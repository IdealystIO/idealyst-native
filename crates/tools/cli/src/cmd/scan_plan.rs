//! What a source scan of a workspace's catalog reads, from one
//! `cargo metadata`.
//!
//! The catalog splits along the workspace boundary (see
//! [`super::catalog_wrapper::generate_deps_only`]): every crate outside
//! the workspace comes from the compiled dependency extractor, which only
//! has to rebuild when the dependency graph changes; every workspace
//! member is read from source by `catalog-scan`. This module decides,
//! for a set of project roots:
//!
//! - **which members to scan**: those the projects reach through normal
//!   dependencies and that touch the framework at all (they are, or
//!   reach, one of [`FRAMEWORK_PACKAGES`]), each with its root file and the features a
//!   catalog build compiles it with — what its `#[cfg(feature = …)]`s
//!   are evaluated against (see [`catalog_features`]);
//! - **whose `macro_rules!` the scan may expand**: the non-member crates
//!   in the same position (idea-theme's `tone!` is one an app invokes);
//! - **what to watch** for changes: the scanned members' directories.
//!
//! `--filter-platform <host>` makes the resolve match the host build the
//! compiled extractor is, so a wasm-only dependency is not in the graph.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use catalog_scan::{Cfg, MacroCrate, ScanCrate};
use serde_json::Value;

/// The input of one workspace scan. See the module docs.
pub struct ScanPlan {
    pub crates: Vec<ScanCrate>,
    /// Each scanned member's package name, in `crates` order.
    pub packages: Vec<String>,
    pub macro_deps: Vec<MacroCrate>,
    /// Each scanned member's crate directory (its `Cargo.toml` and
    /// sources), in `crates` order.
    pub member_dirs: Vec<PathBuf>,
    /// The workspace's cargo target dir (`cargo metadata`'s
    /// `target_directory`).
    pub target_dir: PathBuf,
    /// The files the dependency catalog is a function of, when it is a
    /// function of files at all: the lockfile, every workspace manifest,
    /// and the `.cargo/config.toml`s cargo reads. `None` when some
    /// dependency outside the workspace is a path crate, whose source can
    /// change under an unchanged lockfile (a `[patch]` to a local
    /// framework checkout).
    pub deps_inputs: Option<Vec<PathBuf>>,
}

/// The framework packages a crate must be, or reach, to register catalog
/// entries or define a macro that does: the catalog macros themselves,
/// the vocabulary their output targets (and where `register_style_token!`
/// lives), and the catalog crate (hand registrations name it). A crate
/// reaching none of them — a server crate, `serde` — is neither scanned
/// nor read for macros.
const FRAMEWORK_PACKAGES: &[&str] = &["runtime-macros", "runtime-vocabulary", "mcp-catalog"];

/// The proc-macro crate (named by the test fixtures).
#[cfg(test)]
const MACROS_PACKAGE: &str = "runtime-macros";

/// Plan the scan for `project_roots` (canonical project directories, as
/// [`super::catalog_wrapper::resolve_project_roots`] returns them).
pub fn plan(project_roots: &[PathBuf]) -> Result<ScanPlan> {
    let Some(anchor) = project_roots.first() else {
        bail!("no project roots to scan");
    };
    let cfg = Cfg::host_in(anchor).context("query the host configuration (rustc --print cfg)")?;
    let host = host_triple(anchor)?;
    let meta = metadata(anchor, &host)?;
    let roots: Vec<PathBuf> = project_roots.iter().map(|r| r.join("Cargo.toml")).collect();
    plan_from_metadata(&meta, &roots, &cfg)
}

/// The pure half of [`plan`], over a `cargo metadata` document.
pub fn plan_from_metadata(meta: &Value, root_manifests: &[PathBuf], cfg: &Cfg) -> Result<ScanPlan> {
    let packages: HashMap<&str, &Value> = meta["packages"]
        .as_array()
        .context("cargo metadata has no packages")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let nodes: HashMap<&str, &Value> = meta["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata has no resolve graph")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let normal_deps = |id: &str| -> Vec<&str> {
        nodes
            .get(id)
            .and_then(|n| n["deps"].as_array())
            .map(|deps| {
                deps.iter()
                    .filter(|d| {
                        d["dep_kinds"].as_array().is_some_and(|ks| ks.iter().any(|k| k["kind"].is_null()))
                    })
                    .filter_map(|d| d["pkg"].as_str())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Whether each package touches the framework (memoized DFS).
    let mut reaches: HashMap<&str, bool> = HashMap::new();
    fn reach<'a>(
        id: &'a str,
        packages: &HashMap<&'a str, &'a Value>,
        deps: &dyn Fn(&'a str) -> Vec<&'a str>,
        memo: &mut HashMap<&'a str, bool>,
    ) -> bool {
        if let Some(&r) = memo.get(id) {
            return r;
        }
        memo.insert(id, false); // cycle guard (dev-dep cycles are filtered, but be safe)
        let r = packages.get(id).is_some_and(|p| FRAMEWORK_PACKAGES.iter().any(|f| p["name"] == *f))
            || deps(id).into_iter().any(|d| reach(d, packages, deps, memo));
        memo.insert(id, r);
        r
    }

    let root_ids: Vec<&str> = packages
        .iter()
        .filter(|(_, p)| {
            p["manifest_path"].as_str().is_some_and(|mp| root_manifests.iter().any(|r| same_file(Path::new(mp), r)))
        })
        .map(|(id, _)| *id)
        .collect();
    if root_ids.is_empty() {
        bail!("none of the project roots is a package in its cargo workspace");
    }

    // The projects' normal-dependency closure.
    let mut closure: BTreeSet<&str> = BTreeSet::new();
    let mut stack = root_ids.clone();
    while let Some(id) = stack.pop() {
        if closure.insert(id) {
            stack.extend(normal_deps(id));
        }
    }

    let workspace_root = PathBuf::from(meta["workspace_root"].as_str().unwrap_or_default());
    let deps_immutable = closure.iter().all(|id| members.contains(id) || !packages[id]["source"].is_null());
    let deps_inputs = deps_immutable.then(|| {
        let mut files = vec![workspace_root.join("Cargo.lock"), workspace_root.join("Cargo.toml")];
        for id in &members {
            if let Some(mp) = packages.get(id).and_then(|p| p["manifest_path"].as_str()) {
                files.push(PathBuf::from(mp));
            }
        }
        // cargo reads `.cargo/config.toml` from the workspace up.
        let mut dir = Some(workspace_root.as_path());
        while let Some(d) = dir {
            files.push(d.join(".cargo/config.toml"));
            files.push(d.join(".cargo/config"));
            dir = d.parent();
        }
        files
    });
    let mut plan = ScanPlan {
        crates: Vec::new(),
        packages: Vec::new(),
        macro_deps: Vec::new(),
        member_dirs: Vec::new(),
        target_dir: PathBuf::from(meta["target_directory"].as_str().unwrap_or_default()),
        deps_inputs,
    };
    // Keyed by crate name: a stable scan order.
    let mut scanned: BTreeMap<String, (&str, PathBuf)> = BTreeMap::new();
    for &id in &closure {
        if !reach(id, &packages, &normal_deps, &mut reaches) {
            continue;
        }
        let pkg = packages[id];
        let Some((lib_name, src_path)) = lib_target(pkg) else { continue };
        if members.contains(id) {
            scanned.insert(lib_name, (id, src_path));
        } else if let Some(src_dir) = src_path.parent() {
            plan.macro_deps.push(MacroCrate { name: lib_name, src_dir: src_dir.to_path_buf() });
        }
    }
    let scanned_ids: Vec<&str> = scanned.values().map(|(id, _)| *id).collect();
    let features = catalog_features(&scanned_ids, &packages, &nodes);
    for (lib_name, (id, src_path)) in scanned {
        let pkg = packages[id];
        let dir = Path::new(pkg["manifest_path"].as_str().unwrap_or_default()).parent().map(Path::to_path_buf).unwrap_or_default();
        plan.crates.push(ScanCrate { name: lib_name, root: src_path, cfg: cfg.with_features(features[id].iter().cloned()) });
        plan.packages.push(pkg["name"].as_str().unwrap_or_default().to_string());
        plan.member_dirs.push(dir);
    }
    plan.macro_deps.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(plan)
}

/// The features each of `ids` is compiled with in a catalog build.
///
/// The compiled extractor is not the app's own build: it turns on
/// `runtime-core/catalog` and every linked library's own `catalog`
/// feature (the wrapper's force-link), and those switch on more —
/// `runtime-core/catalog` enables `runtime-vocabulary/catalog`, idea-ui's
/// enables idea-theme's. Gates like `#[cfg(feature = "catalog")] mod
/// recipes;` read exactly that set, so the scan evaluates against it: the
/// crate's resolved features, plus `catalog` on every scanned crate that
/// declares one, closed over the `feature = [..]` lists — a crate's own
/// features, and `dep/feature` / `dep?/feature` into another scanned
/// crate.
///
/// Every scanned crate gets its `catalog`, not only those the full
/// extractor happens to force-link directly: what a crate registers
/// under its catalog feature is part of its catalog wherever it sits in
/// the graph.
fn catalog_features<'a>(
    ids: &[&'a str],
    packages: &HashMap<&'a str, &'a Value>,
    nodes: &HashMap<&'a str, &'a Value>,
) -> HashMap<&'a str, BTreeSet<String>> {
    let mut on: HashMap<&str, BTreeSet<String>> = ids
        .iter()
        .map(|&id| {
            let mut set: BTreeSet<String> = nodes
                .get(id)
                .and_then(|n| n["features"].as_array())
                .map(|fs| fs.iter().filter_map(|f| f.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if packages[id]["features"].get("catalog").is_some() {
                set.insert("catalog".to_string());
            }
            (id, set)
        })
        .collect();
    // A dependency key (as written in `features`, renames included) →
    // the scanned package it names, per scanned package.
    let dep_targets = |id: &str| -> HashMap<String, &str> {
        let mut out = HashMap::new();
        for dep in packages[id]["dependencies"].as_array().into_iter().flatten() {
            let (Some(name), key) = (dep["name"].as_str(), dep["rename"].as_str().or(dep["name"].as_str())) else { continue };
            if let Some(&target) = ids.iter().find(|&&t| packages[t]["name"] == name) {
                out.insert(key.unwrap_or(name).to_string(), target);
            }
        }
        out
    };
    loop {
        let mut added: Vec<(&str, String)> = Vec::new();
        for &id in ids {
            let declared = &packages[id]["features"];
            let targets = dep_targets(id);
            for feature in &on[id] {
                for implied in declared[feature.as_str()].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if let Some((dep, feat)) = implied.split_once('/') {
                        if let Some(&target) = targets.get(dep.trim_end_matches('?')) {
                            added.push((target, feat.to_string()));
                        }
                    } else if !implied.starts_with("dep:") && declared.get(implied).is_some() {
                        added.push((id, implied.to_string()));
                    }
                }
            }
        }
        let mut changed = false;
        for (id, feature) in added {
            changed |= on.get_mut(id).expect("a scanned id").insert(feature);
        }
        if !changed {
            return on;
        }
    }
}

/// A package's library target: its crate name (`-` as `_`) and root file.
/// Proc-macro crates register nothing and are skipped.
fn lib_target(pkg: &Value) -> Option<(String, PathBuf)> {
    pkg["targets"].as_array()?.iter().find_map(|t| {
        let kinds: Vec<&str> = t["kind"].as_array()?.iter().filter_map(Value::as_str).collect();
        let is_lib = kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "cdylib" | "staticlib"));
        if !is_lib || kinds.contains(&"proc-macro") {
            return None;
        }
        Some((t["name"].as_str()?.replace('-', "_"), PathBuf::from(t["src_path"].as_str()?)))
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// The host target triple of the toolchain `dir` builds with.
pub(crate) fn host_triple(dir: &Path) -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = std::process::Command::new(rustc).arg("-vV").current_dir(dir).output().context("run rustc -vV")?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(|h| h.trim().to_string()))
        .context("rustc -vV reported no host triple")
}

/// `cargo metadata` of the workspace `dir` is in, resolved for `host`.
fn metadata(dir: &Path, host: &str) -> Result<Value> {
    let manifest = dir.join("Cargo.toml");
    let out = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--filter-platform", host, "--manifest-path"])
        .arg(&manifest)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("run cargo metadata for {}", manifest.display()))?;
    if !out.status.success() {
        bail!("cargo metadata failed for {}: {}", manifest.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("parse cargo metadata for {}", manifest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pkg(name: &str, dir: &str) -> Value {
        // The macros crate is what it is in reality: a proc-macro crate,
        // which registers nothing itself.
        let kind = if name == MACROS_PACKAGE { json!(["proc-macro"]) } else { json!(["cdylib", "rlib"]) };
        json!({
            "id": format!("{name} 0.1.0"),
            "name": name,
            "manifest_path": format!("{dir}/Cargo.toml"),
            "targets": [{ "kind": kind, "name": name, "src_path": format!("{dir}/src/lib.rs") }],
        })
    }

    fn node(name: &str, deps: &[&str], features: &[&str]) -> Value {
        json!({
            "id": format!("{name} 0.1.0"),
            "features": features,
            "deps": deps.iter().map(|d| json!({ "pkg": format!("{d} 0.1.0"), "dep_kinds": [{ "kind": null }] })).collect::<Vec<_>>(),
        })
    }

    /// The split the catalog relies on: members that can use the macros
    /// are scanned (with their features); non-members that can are macro
    /// sources only; crates that can't reach the macros (a server crate,
    /// `serde`) are neither; a member outside the projects' closure is
    /// not scanned.
    #[test]
    fn scans_reachable_members_and_reads_macros_from_dependencies() {
        let meta = json!({
            "packages": [
                pkg("app", "/w/app"), pkg("ui-shared", "/w/ui-shared"), pkg("api", "/w/api"),
                pkg("other-app", "/w/other"), pkg("idea-theme", "/reg/idea-theme"),
                pkg("runtime-macros", "/reg/runtime-macros"), pkg("serde", "/reg/serde"),
            ],
            "resolve": { "nodes": [
                node("app", &["ui-shared", "api", "idea-theme"], &["default", "web"]),
                node("ui-shared", &["idea-theme"], &[]),
                node("api", &["serde"], &[]),
                node("other-app", &["idea-theme"], &[]),
                node("idea-theme", &["runtime-macros"], &[]),
                node("runtime-macros", &[], &[]),
                node("serde", &[], &[]),
            ]},
            "workspace_members": ["app 0.1.0", "ui-shared 0.1.0", "api 0.1.0", "other-app 0.1.0"],
        });
        let plan = plan_from_metadata(&meta, &[PathBuf::from("/w/app/Cargo.toml")], &Cfg::default()).unwrap();
        let names: Vec<&str> = plan.crates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["app", "ui_shared"]);
        // `idea-theme` here is a path crate outside the workspace: its
        // source can change under the same lockfile, so the dependency
        // catalog cannot be cached.
        assert!(plan.deps_inputs.is_none());
        assert_eq!(plan.packages, ["app", "ui-shared"]);
        assert_eq!(plan.crates[0].root, PathBuf::from("/w/app/src/lib.rs"));
        assert_eq!(plan.member_dirs, [PathBuf::from("/w/app"), PathBuf::from("/w/ui-shared")]);
        let deps: Vec<&str> = plan.macro_deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(deps, ["idea_theme"]);
        assert_eq!(plan.macro_deps[0].src_dir, PathBuf::from("/reg/idea-theme/src"));
        // `app`'s `web` feature is what its cfgs see.
        let gated: syn::Meta = syn::parse_quote!(feature = "web");
        assert!(plan.crates[0].cfg.eval(&gated));
        assert!(!plan.crates[1].cfg.eval(&gated));
    }

    /// With every dependency outside the workspace from a registry, the
    /// dependency catalog is a function of the lockfile, the workspace
    /// manifests and cargo's config — the inputs the cache keys on.
    #[test]
    fn registry_dependencies_make_the_dependency_catalog_cacheable() {
        let mut theme = pkg("idea-theme", "/reg/idea-theme");
        theme["source"] = json!("sparse+https://crates.idealyst.io/index/");
        let mut macros = pkg("runtime-macros", "/reg/runtime-macros");
        macros["source"] = json!("sparse+https://crates.idealyst.io/index/");
        let meta = json!({
            "packages": [pkg("app", "/w/app"), theme, macros],
            "resolve": { "nodes": [
                node("app", &["idea-theme"], &[]),
                node("idea-theme", &["runtime-macros"], &[]),
                node("runtime-macros", &[], &[]),
            ]},
            "workspace_members": ["app 0.1.0"],
            "workspace_root": "/w",
            "target_directory": "/w/target",
        });
        let plan = plan_from_metadata(&meta, &[PathBuf::from("/w/app/Cargo.toml")], &Cfg::default()).unwrap();
        let inputs = plan.deps_inputs.expect("cacheable");
        for f in ["/w/Cargo.lock", "/w/Cargo.toml", "/w/app/Cargo.toml", "/w/.cargo/config.toml", "/.cargo/config.toml"] {
            assert!(inputs.contains(&PathBuf::from(f)), "{f} missing from {inputs:?}");
        }
        assert_eq!(plan.target_dir, PathBuf::from("/w/target"));
    }

    /// A catalog build turns on each scanned crate's `catalog` and what
    /// it implies, own features and other scanned crates' alike — the
    /// set the compiled extractor builds them with.
    #[test]
    fn scanned_crates_see_the_catalog_builds_features() {
        let mut ui = pkg("idea-ui", "/w/idea-ui");
        ui["features"] = json!({ "default": ["table"], "table": [], "catalog": ["theme/catalog", "extras"], "extras": [] });
        ui["dependencies"] = json!([{ "name": "idea-theme", "rename": "theme" }]);
        let mut theme = pkg("idea-theme", "/w/idea-theme");
        theme["features"] = json!({ "catalog": ["recipes"], "recipes": [] });
        let meta = json!({
            "packages": [ui, theme, pkg("runtime-macros", "/reg/runtime-macros")],
            "resolve": { "nodes": [
                node("idea-ui", &["idea-theme"], &["default", "table"]),
                node("idea-theme", &["runtime-macros"], &[]),
                node("runtime-macros", &[], &[]),
            ]},
            "workspace_members": ["idea-ui 0.1.0", "idea-theme 0.1.0"],
        });
        let plan = plan_from_metadata(&meta, &[PathBuf::from("/w/idea-ui/Cargo.toml")], &Cfg::default()).unwrap();
        let on = |krate: usize, f: &str| plan.crates[krate].cfg.eval(&syn::parse_str(&format!("feature = {f:?}")).unwrap());
        let (theme, ui) = (0, 1); // sorted by crate name
        assert_eq!(plan.crates[ui].name, "idea_ui");
        assert!(on(ui, "table") && on(ui, "catalog") && on(ui, "extras"));
        assert!(on(theme, "catalog") && on(theme, "recipes"), "reached through the renamed dep");
    }
}
