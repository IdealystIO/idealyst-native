//! Ephemeral "catalog wrapper" crate generator.
//!
//! The MCP server lists a project's `#[component]`s by running a binary
//! that (a) links the project's library so its `inventory::submit!`
//! registrations are present, and (b) is built with `runtime-core/catalog`
//! on so the `#[component]` macro actually *emits* those registrations.
//!
//! Rather than make every project carry that binary + an `catalog` feature
//! (the old scaffold did), the CLI generates a throwaway wrapper crate
//! under `target/idealyst/<project>/catalog/` — the same place and
//! shape as the per-platform `{web,ios,android}` wrappers. The wrapper
//! path-deps the project and turns on `runtime-core/catalog`; Cargo's
//! feature unification then builds `runtime-macros` with emission on for
//! the *entire* graph, so the project's components register even though
//! the project declares no `catalog` feature itself.
//!
//! This is the mechanism that lets `idealyst mcp` work against any
//! project — old or new — with zero per-project boilerplate.
//!
//! ## Force-linking dependency component crates
//!
//! `use {lib} as _;` only pins the project's OWN `inventory::submit!`
//! registrations. A component library the project depends on (e.g.
//! `idea-ui`) registers its components via the same mechanism, but
//! `inventory`'s linker-section ctors only survive linking if the
//! linker actually pulls that crate's object code in — `inventory`'s
//! known cross-rlib caveat (framework-mcp-spec §9.3). Merely depending
//! on a component library doesn't guarantee that.
//!
//! So the wrapper walks the project's `cargo metadata`, finds every
//! direct dependency that itself depends on `runtime-core` (the signal
//! for "this crate may declare `#[component]`s"), declares each as a
//! direct wrapper dependency, and emits a `use <dep> as _;` for it. That
//! forces their object code — and thus their `inventory::submit!`
//! registrations — into the catalog binary, so a freshly-added
//! component-library dependency surfaces in the catalog even before the
//! project references any of its components.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;
use serde_json::Value;

use crate::framework_source;

/// Generate (or refresh) the catalog wrapper crate for `project_root`
/// and return its directory. The returned crate exposes a `catalog`
/// bin that prints the project's MCP catalog JSON to stdout — run it
/// with `cargo run -q --bin catalog` from the returned directory.
///
/// Idempotent: files are only rewritten when their contents change, so
/// repeated calls don't invalidate cargo's fingerprints and trigger
/// needless rebuilds.
pub fn generate(project_root: &Path) -> Result<PathBuf> {
    generate_with(project_root, "catalog", "catalog", "dump_catalog_json")
}

/// Parameterized core of [`generate`]. Builds an ephemeral wrapper that
/// links the project (+ force-links its component-library deps) with
/// `runtime-core/catalog` on, exposing a `<bin_name>` binary whose
/// `main()` calls `runtime_core::__mcp::<dump_call>()`.
///
/// `subdir` names the staging dir under `target/idealyst/<project>/`, so
/// distinct extractors (the MCP catalog vs. the external-export manifest)
/// don't clobber each other's build fingerprints.
pub fn generate_with(
    project_root: &Path,
    subdir: &str,
    bin_name: &str,
    dump_call: &str,
) -> Result<PathBuf> {
    // Absolute project path — the wrapper lives elsewhere on disk and
    // references the project by path, so a relative `.` / cwd would
    // resolve against the wrapper dir, not here.
    let project_root = fs::canonicalize(project_root)
        .with_context(|| format!("canonicalize project dir {}", project_root.display()))?;

    let manifest = build_ios::parse_manifest(&project_root)
        .context("read the project's Cargo.toml to build a catalog wrapper")?;
    let source = framework_source::resolve(&project_root)?;

    let wrapper_dir = source
        .wrapper_root(&project_root)
        .join(&manifest.name)
        .join(subdir);
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.join("src").display()))?;

    // `runtime-core` with `catalog` on is the lever: enabling it anywhere
    // in the graph flips the `#[component]` emission gate for every crate,
    // including the project lib.
    let fcore_dep = source.dep("crates/runtime/core", &["catalog"]);
    // Redirect any git-pinned framework crates the project uses to the
    // same physical paths the wrapper uses, so the two halves share ONE
    // `runtime_core` instance (otherwise feature unification can't merge
    // the `catalog` feature, and the wrapper→project type bridge fails).
    // Empty in git mode — there both sides already use the same rev.
    let patch_block = source.patch_block();

    // Direct dependencies that themselves depend on `runtime-core` —
    // i.e. crates that may declare `#[component]`s. We force-link each
    // so its catalog registrations survive linking even before the
    // project references any of its components (see module docs).
    // Non-fatal: a metadata failure just means we link the project lib
    // only, the same behaviour as before this was added.
    let forced = discover_forced_deps(&project_root, &source, &manifest.name);

    let forced_dep_lines = forced
        .iter()
        .map(|d| format!("{} = {}\n", d.pkg_name, d.dep_line))
        .collect::<String>();
    let forced_use_lines = forced
        .iter()
        .map(|d| format!("use {} as _;\n", d.lib_ident))
        .collect::<String>();

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst mcp`. Do not edit — rewritten on demand.
#
# Ephemeral catalog-extraction wrapper. Links the project's library and
# turns on `runtime-core/catalog` so every `#[component]` in the project (and
# its component-library deps) registers in the MCP catalog. The project
# itself needs no `[[bin]] catalog` and no `catalog` feature.
#
# Empty `[workspace]` declares this wrapper standalone even though it
# lives under the framework workspace's `target/idealyst/...`; without it
# cargo would try to claim it as a member of the parent workspace.
[workspace]

[package]
name = "{name}-{subdir}-wrapper"
version = "0.0.1"
edition = "2021"
publish = false

[[bin]]
name = "{bin_name}"
path = "src/main.rs"

[dependencies]
runtime-core = {fcore_dep}
{user_name} = {{ path = "{user_path}" }}
{forced_dep_lines}{patch_block}"#,
        name = manifest.name,
        subdir = subdir,
        bin_name = bin_name,
        fcore_dep = fcore_dep,
        user_name = manifest.name,
        user_path = project_root.display(),
        forced_dep_lines = forced_dep_lines,
        patch_block = patch_block,
    );

    let main_rs = format!(
        r#"//! GENERATED by `idealyst mcp` — ephemeral catalog extractor.
//!
//! `use {lib} as _;` links the project's library so its
//! `inventory::submit!` component registrations are present; the wrapper
//! is built with `runtime-core/catalog`, so those registrations were
//! emitted. Each `use <dep> as _;` below force-links a component-library
//! dependency so its registrations survive linking too (see the wrapper
//! generator's module docs). `dump_catalog_json` serializes the
//! collected catalog to stdout.

use {lib} as _;
{forced_use_lines}
fn main() {{
    runtime_core::__mcp::{dump_call}();
}}
"#,
        lib = manifest.lib_name,
        forced_use_lines = forced_use_lines,
        dump_call = dump_call,
    );

    // Share the project/workspace target dir so common dependencies stay
    // warm across builds and the produced binary lives at a predictable
    // path. (The `catalog`-feature build of runtime-core is a distinct cargo
    // fingerprint from the project's normal build, so they coexist
    // rather than clobber each other.)
    let cargo_config = format!(
        "# GENERATED. Redirect this wrapper's build output to the shared\n\
         # target dir so subsequent extractions reuse the cache.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        source.cargo_target_dir(&project_root).display(),
    );

    write_if_changed(&wrapper_dir.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&wrapper_dir.join("src/main.rs"), &main_rs)?;
    fs::create_dir_all(wrapper_dir.join(".cargo"))
        .with_context(|| format!("create {}", wrapper_dir.join(".cargo").display()))?;
    write_if_changed(&wrapper_dir.join(".cargo/config.toml"), &cargo_config)?;

    Ok(wrapper_dir)
}

/// A dependency crate the wrapper force-links so its component
/// registrations survive linking.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForcedDep {
    /// Cargo package name — the `[dependencies]` table key.
    pkg_name: String,
    /// Lib crate identifier for `use <ident> as _;` (the dep's lib
    /// target name, e.g. `idea_ui`).
    lib_ident: String,
    /// The right-hand side of the `[dependencies]` entry — a cargo dep
    /// table sourced to match how the project resolves the same crate
    /// (path in workspace mode, git in git mode), so cargo unifies them
    /// into a single instance rather than a parallel `runtime_core`.
    /// Carries `features = ["catalog"]` when the crate declares a `catalog`
    /// feature (the MCP self-registration gate — see [`dep_line_for`]).
    dep_line: String,
}

/// Run `cargo metadata` for `project_root` and collect the component-
/// library dependencies to force-link. Non-fatal: on any failure we log
/// to stderr and return an empty list — the wrapper still links the
/// project's own library, so the project's own components appear.
fn discover_forced_deps(
    project_root: &Path,
    source: &FrameworkSource,
    project_pkg_name: &str,
) -> Vec<ForcedDep> {
    let manifest_path = project_root.join("Cargo.toml");
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "[idealyst mcp] cargo metadata failed; dependency components may not \
                 appear in the catalog: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return Vec::new();
        }
        Err(e) => {
            eprintln!(
                "[idealyst mcp] could not run cargo metadata ({e}); dependency \
                 components may not appear in the catalog"
            );
            return Vec::new();
        }
    };
    let json: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[idealyst mcp] cargo metadata produced invalid JSON: {e}");
            return Vec::new();
        }
    };
    collect_forced_deps(&json, source, &manifest_path, project_pkg_name)
}

/// Pure core of [`discover_forced_deps`], split out so it's unit-testable
/// against a synthetic `cargo metadata` document without invoking cargo.
///
/// A dependency qualifies when it is (a) a direct, normal (non dev/build)
/// dependency of the project, (b) itself depends on `runtime-core` — the
/// signal that it may declare `#[component]`s — and (c) exposes a normal
/// `lib`/`rlib` target (not a proc-macro). In git mode we additionally
/// require the dependency to originate from the framework's own git repo:
/// a third-party crate resolved from a different source can't be
/// re-declared by the wrapper without risking a duplicate crate instance,
/// so we skip it rather than corrupt the build.
fn collect_forced_deps(
    metadata: &Value,
    source: &FrameworkSource,
    project_manifest_path: &Path,
    project_pkg_name: &str,
) -> Vec<ForcedDep> {
    let packages = match metadata.get("packages").and_then(|p| p.as_array()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // Resolve the root package id. Prefer `resolve.root`; fall back to
    // the package whose manifest_path matches the project (cargo leaves
    // `resolve.root` null for a virtual-workspace manifest).
    let root_id = metadata
        .pointer("/resolve/root")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            packages.iter().find_map(|p| {
                let mp = p.get("manifest_path").and_then(|m| m.as_str())?;
                (Path::new(mp) == project_manifest_path)
                    .then(|| p.get("id").and_then(|i| i.as_str()).map(String::from))
                    .flatten()
            })
        });
    let Some(root_id) = root_id else {
        return Vec::new();
    };

    // Direct, normal dependency package ids of the root, from the
    // resolve graph (which already applied platform/feature resolution).
    let direct_ids: Vec<String> = metadata
        .pointer("/resolve/nodes")
        .and_then(|n| n.as_array())
        .and_then(|nodes| nodes.iter().find(|n| n.get("id").and_then(|i| i.as_str()) == Some(root_id.as_str())))
        .and_then(|root_node| root_node.get("deps").and_then(|d| d.as_array()))
        .map(|deps| {
            deps.iter()
                .filter(|dep| dep_has_normal_kind(dep))
                .filter_map(|dep| dep.get("pkg").and_then(|p| p.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut out: Vec<ForcedDep> = Vec::new();
    for id in &direct_ids {
        let Some(pkg) = packages
            .iter()
            .find(|p| p.get("id").and_then(|i| i.as_str()) == Some(id.as_str()))
        else {
            continue;
        };
        let name = pkg.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        // Never re-declare runtime-core (already a wrapper dep) or the
        // project itself (already linked via `use {lib} as _;`).
        if name.is_empty() || name == "runtime-core" || name == project_pkg_name {
            continue;
        }
        // Only crates that depend on runtime-core can host components.
        if !pkg_depends_on_runtime_core(pkg) {
            continue;
        }
        let Some(lib_ident) = pkg_lib_target_name(pkg) else {
            continue;
        };
        let Some(manifest) = pkg.get("manifest_path").and_then(|m| m.as_str()) else {
            continue;
        };
        let Some(dir) = Path::new(manifest).parent() else {
            continue;
        };
        let pkg_source = pkg.get("source").and_then(|s| s.as_str());
        // Some component-library crates gate their MCP self-registration
        // behind their OWN `catalog` feature (a separate inventory slice
        // from `runtime-core/catalog`'s `#[component]` emission) — e.g.
        // `icons-lucide`'s `IconSetEntry` and the recipe system. Enabling
        // `runtime-core/catalog` does NOT transitively turn those on, so we
        // must enable each crate's `catalog` feature explicitly or its
        // registrations compile out and never reach the catalog.
        let features: &[&str] = if pkg_has_catalog_feature(pkg) {
            &["catalog"]
        } else {
            &[]
        };
        let Some(dep_line) = dep_line_for(source, dir, pkg_source, features) else {
            // git mode + third-party source: skip (see fn docs).
            eprintln!(
                "[idealyst mcp] skipping force-link of `{name}` — its source isn't the \
                 framework git repo, so it can't be safely re-declared in git mode. Its \
                 components will appear once your code references the crate."
            );
            continue;
        };
        out.push(ForcedDep {
            pkg_name: name.to_string(),
            lib_ident,
            dep_line,
        });
    }

    // Deterministic order so `write_if_changed` stays idempotent.
    out.sort_by(|a, b| a.pkg_name.cmp(&b.pkg_name));
    out.dedup_by(|a, b| a.pkg_name == b.pkg_name);
    out
}

/// True if a `resolve.nodes[].deps[]` entry includes a normal (non
/// dev/build) dependency kind. Cargo encodes the normal kind as a
/// `null` `kind`; older metadata without `dep_kinds` is treated as
/// normal too.
fn dep_has_normal_kind(dep: &Value) -> bool {
    match dep.get("dep_kinds").and_then(|k| k.as_array()) {
        None => true,
        Some(kinds) => kinds
            .iter()
            .any(|k| k.get("kind").map(|v| v.is_null()).unwrap_or(true)),
    }
}

/// True if a package's manifest declares a dependency named
/// `runtime-core` (any kind) — the marker that it may host components.
fn pkg_depends_on_runtime_core(pkg: &Value) -> bool {
    pkg.get("dependencies")
        .and_then(|d| d.as_array())
        .map(|deps| {
            deps.iter()
                .any(|d| d.get("name").and_then(|n| n.as_str()) == Some("runtime-core"))
        })
        .unwrap_or(false)
}

/// True if a package declares a `catalog` Cargo feature — the framework's
/// convention for gating `mcp_catalog::inventory::submit!` self-registration
/// that `#[component]` emission doesn't cover (icon packs' `IconSetEntry`,
/// the recipe system, …). `cargo metadata` exposes the feature map on every
/// package, so this is a pure lookup.
fn pkg_has_catalog_feature(pkg: &Value) -> bool {
    pkg.get("features")
        .and_then(|f| f.as_object())
        .map(|m| m.contains_key("catalog"))
        .unwrap_or(false)
}

/// The package's importable lib target name (`idea_ui`), or `None` if it
/// has no normal library target (e.g. a binary-only or proc-macro crate,
/// which can't be `use`d as a linked library).
fn pkg_lib_target_name(pkg: &Value) -> Option<String> {
    let targets = pkg.get("targets").and_then(|t| t.as_array())?;
    targets.iter().find_map(|t| {
        let kinds = t.get("kind").and_then(|k| k.as_array())?;
        let is_lib = kinds.iter().any(|k| {
            matches!(k.as_str(), Some("lib") | Some("rlib") | Some("dylib"))
        });
        let is_proc_macro = kinds.iter().any(|k| k.as_str() == Some("proc-macro"));
        (is_lib && !is_proc_macro)
            .then(|| t.get("name").and_then(|n| n.as_str()).map(String::from))
            .flatten()
    })
}

/// Build the `[dependencies]` RHS for a force-linked crate, sourced to
/// match how the project resolves it so cargo unifies them:
///
/// - **Workspace mode**: a `path` dep to the resolved manifest directory.
///   The project's own dep (whether `{ workspace = true }` or `path`)
///   resolves to the same directory, so cargo sees one package instance.
/// - **Git mode**: a `git` dep pinned to the same url + refspec as the
///   framework. Returns `None` for a dependency whose source isn't that
///   git repo — re-declaring a foreign source would fork the crate graph.
///
/// `features` are appended verbatim (e.g. `["catalog"]`) so a crate whose
/// MCP registration is feature-gated is built with that gate on. Feature
/// unification then enables it on the single shared crate instance.
fn dep_line_for(
    source: &FrameworkSource,
    manifest_dir: &Path,
    pkg_source: Option<&str>,
    features: &[&str],
) -> Option<String> {
    // `, features = ["a", "b"]` or empty.
    let feat = if features.is_empty() {
        String::new()
    } else {
        let list = features
            .iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(", ");
        format!(", features = [{}]", list)
    };
    match source {
        FrameworkSource::Workspace { .. } => {
            Some(format!("{{ path = \"{}\"{} }}", manifest_dir.display(), feat))
        }
        FrameworkSource::Git { url, refspec } => {
            // Only force-link crates that come from the framework's git
            // repo; their git source already matches the project's, so
            // cargo merges them into one instance.
            let src = pkg_source?;
            if !src.contains(url.as_str()) {
                return None;
            }
            let (key, value) = refspec.as_pair();
            Some(format!("{{ git = \"{}\", {} = \"{}\"{} }}", url, key, value, feat))
        }
    }
}

/// Write `contents` to `path` only if it differs from what's already
/// there. Avoids bumping mtimes (and thus cargo fingerprints) on
/// no-op regenerations.
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal project: `[package]` + a hyphenated name (so we exercise
    /// the `name → lib_name` `-`→`_` conversion) and a `runtime-core`
    /// dep. No `[[bin]] catalog`, no `catalog` feature — the whole point is
    /// that the wrapper supplies both.
    ///
    /// `runtime-core` is pinned to a non-existent local path so the
    /// `cargo metadata` call inside `generate` (forced-dep discovery)
    /// fails fast and OFFLINE — these tests assert file emission, not
    /// dependency resolution, and must not touch the network. Discovery
    /// degrades gracefully to "no forced deps", which is what we want
    /// here. The forced-dep selection itself is covered by the pure
    /// `collect_forced_deps` tests below.
    fn fake_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-catwrap-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-app\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
             [dependencies]\nruntime-core = { path = \"./does-not-exist/crates/runtime/core\" }\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn generate_emits_a_runnable_catalog_wrapper() {
        let project = fake_project("emit");
        let wrapper = generate(&project).expect("generate wrapper");

        // Lives under the project's `target/idealyst/<name>/catalog`.
        assert!(wrapper.ends_with("target/idealyst/my-app/catalog"), "{:?}", wrapper);

        let cargo = fs::read_to_string(wrapper.join("Cargo.toml")).unwrap();
        // A `catalog` bin the MCP server can run.
        assert!(cargo.contains("[[bin]]"));
        assert!(cargo.contains("name = \"catalog\""));
        // runtime-core with the `catalog` feature on — the emission lever.
        assert!(cargo.contains("runtime-core ="));
        assert!(cargo.contains("features = [\"catalog\"]"), "cargo: {cargo}");
        // Path-deps the project under its package name.
        assert!(cargo.contains("my-app = { path ="), "cargo: {cargo}");
        // Standalone so the parent workspace doesn't claim it.
        assert!(cargo.contains("[workspace]"));

        let main_rs = fs::read_to_string(wrapper.join("src/main.rs")).unwrap();
        // Imports by LIB name (hyphen → underscore) and dumps the catalog.
        assert!(main_rs.contains("use my_app as _;"), "main: {main_rs}");
        assert!(main_rs.contains("dump_catalog_json"));

        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn generate_is_idempotent() {
        let project = fake_project("idem");
        let wrapper = generate(&project).expect("first generate");
        let main_path = wrapper.join("src/main.rs");
        let mtime1 = fs::metadata(&main_path).unwrap().modified().unwrap();
        // Second call with identical inputs must not rewrite the files
        // (would bump mtime and invalidate cargo's fingerprint).
        let wrapper2 = generate(&project).expect("second generate");
        assert_eq!(wrapper, wrapper2);
        let mtime2 = fs::metadata(&main_path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "idempotent regenerate must not rewrite files");

        let _ = fs::remove_dir_all(&project);
    }

    use build_ios::{FrameworkSource, GitRef};
    use serde_json::json;

    /// A `cargo metadata`-shaped document: the project `my-app` directly
    /// depends on `idea-ui` (a component lib — depends on runtime-core,
    /// has a lib target), `serde` (no runtime-core dep), a dev-dep
    /// `dev-tool` (component-ish but dev kind), `proc-mac` (proc-macro
    /// only) and `runtime-core` itself.
    fn sample_metadata() -> Value {
        json!({
            "packages": [
                {
                    "id": "app",
                    "name": "my-app",
                    "manifest_path": "/proj/Cargo.toml",
                    "source": null,
                    "dependencies": [
                        {"name": "runtime-core", "kind": null},
                        {"name": "idea-ui", "kind": null},
                        {"name": "serde", "kind": null}
                    ],
                    "targets": [{"name": "my_app", "kind": ["lib"]}]
                },
                {
                    "id": "ui",
                    "name": "idea-ui",
                    "manifest_path": "/ws/crates/ui/idea-ui/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "idea_ui", "kind": ["lib"]}]
                },
                {
                    "id": "serde",
                    "name": "serde",
                    "manifest_path": "/reg/serde/Cargo.toml",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "dependencies": [],
                    "targets": [{"name": "serde", "kind": ["lib"]}]
                },
                {
                    "id": "devtool",
                    "name": "dev-tool",
                    "manifest_path": "/ws/crates/dev-tool/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "dev_tool", "kind": ["lib"]}]
                },
                {
                    "id": "pm",
                    "name": "proc-mac",
                    "manifest_path": "/ws/crates/proc-mac/Cargo.toml",
                    "source": null,
                    "dependencies": [{"name": "runtime-core", "kind": null}],
                    "targets": [{"name": "proc_mac", "kind": ["proc-macro"]}]
                },
                {
                    "id": "rc",
                    "name": "runtime-core",
                    "manifest_path": "/ws/crates/runtime/core/Cargo.toml",
                    "source": null,
                    "dependencies": [],
                    "targets": [{"name": "runtime_core", "kind": ["lib"]}]
                }
            ],
            "resolve": {
                "root": "app",
                "nodes": [
                    {
                        "id": "app",
                        "deps": [
                            {"pkg": "ui", "dep_kinds": [{"kind": null}]},
                            {"pkg": "serde", "dep_kinds": [{"kind": null}]},
                            {"pkg": "devtool", "dep_kinds": [{"kind": "dev"}]},
                            {"pkg": "pm", "dep_kinds": [{"kind": null}]},
                            {"pkg": "rc", "dep_kinds": [{"kind": null}]}
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn collect_force_links_only_component_library_deps_in_workspace_mode() {
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(
            &sample_metadata(),
            &src,
            Path::new("/proj/Cargo.toml"),
            "my-app",
        );
        // Only idea-ui qualifies: serde lacks a runtime-core dep,
        // dev-tool is a dev-dependency, proc-mac is proc-macro-only,
        // runtime-core and the project itself are excluded by name.
        assert_eq!(deps.len(), 1, "got: {deps:?}");
        let d = &deps[0];
        assert_eq!(d.pkg_name, "idea-ui");
        assert_eq!(d.lib_ident, "idea_ui");
        // Workspace mode → path dep to the resolved manifest dir, which
        // is exactly what the project's own dep resolves to (unifies).
        assert_eq!(d.dep_line, "{ path = \"/ws/crates/ui/idea-ui\" }");
    }

    #[test]
    fn collect_enables_catalog_feature_on_deps_that_declare_one() {
        // A crate that gates its MCP self-registration behind a `catalog`
        // feature (like `icons-lucide`'s IconSetEntry) must be force-linked
        // WITH that feature on — otherwise the submission compiles out and
        // its slice is empty. Crates without a `catalog` feature (idea-ui)
        // must stay bare so we don't enable a feature that doesn't exist.
        let mut meta = sample_metadata();
        // Add an icons-lucide-shaped package with a `catalog` feature.
        meta["packages"].as_array_mut().unwrap().push(json!({
            "id": "icons",
            "name": "icons-lucide",
            "manifest_path": "/ws/crates/ui/icons-lucide/Cargo.toml",
            "source": null,
            "dependencies": [{"name": "runtime-core", "kind": null}],
            "targets": [{"name": "icons_lucide", "kind": ["lib"]}],
            "features": {"registry": [], "catalog": ["dep:mcp-catalog"]}
        }));
        meta["resolve"]["nodes"][0]["deps"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pkg": "icons", "dep_kinds": [{"kind": null}]}));

        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 2, "got: {deps:?}");

        let icons = deps.iter().find(|d| d.pkg_name == "icons-lucide").expect("icons-lucide forced");
        assert_eq!(
            icons.dep_line,
            "{ path = \"/ws/crates/ui/icons-lucide\", features = [\"catalog\"] }",
            "icons-lucide must be force-linked with its catalog feature on",
        );
        // idea-ui declares no `catalog` feature → stays bare (regression
        // guard: we don't enable a non-existent feature).
        let ui = deps.iter().find(|d| d.pkg_name == "idea-ui").expect("idea-ui forced");
        assert_eq!(ui.dep_line, "{ path = \"/ws/crates/ui/idea-ui\" }");
    }

    #[test]
    fn collect_enables_catalog_feature_in_git_mode_too() {
        let url = "https://github.com/IdealystIO/idealyst-native";
        let src = FrameworkSource::Git {
            url: url.to_string(),
            refspec: GitRef::Rev("abc123".to_string()),
        };
        let mut meta = sample_metadata();
        meta["packages"].as_array_mut().unwrap().push(json!({
            "id": "icons",
            "name": "icons-lucide",
            "manifest_path": "/ws/crates/ui/icons-lucide/Cargo.toml",
            "source": format!("git+{url}?rev=abc123#abc123"),
            "dependencies": [{"name": "runtime-core", "kind": null}],
            "targets": [{"name": "icons_lucide", "kind": ["lib"]}],
            "features": {"catalog": ["dep:mcp-catalog"]}
        }));
        // idea-ui must also be sourced from the framework git repo to be kept.
        meta["packages"][1]["source"] = json!(format!("git+{url}?rev=abc123#abc123"));
        meta["resolve"]["nodes"][0]["deps"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pkg": "icons", "dep_kinds": [{"kind": null}]}));

        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        let icons = deps.iter().find(|d| d.pkg_name == "icons-lucide").expect("icons-lucide forced");
        assert_eq!(
            icons.dep_line,
            format!("{{ git = \"{url}\", rev = \"abc123\", features = [\"catalog\"] }}"),
        );
    }

    #[test]
    fn collect_emits_git_deps_for_framework_crates_and_skips_foreign_in_git_mode() {
        let url = "https://github.com/IdealystIO/idealyst-native";
        let src = FrameworkSource::Git {
            url: url.to_string(),
            refspec: GitRef::Tag("v0.1.0".to_string()),
        };
        // idea-ui resolved from the framework git repo → force-linked
        // via a matching git dep.
        let mut meta = sample_metadata();
        meta["packages"][1]["source"] =
            json!(format!("git+{url}?tag=v0.1.0#abc123"));
        // A third-party component lib resolved from crates.io: depends on
        // runtime-core but its source isn't the framework repo, so we
        // can't re-declare it safely in git mode — must be skipped.
        meta["packages"][2]["dependencies"] = json!([{"name": "runtime-core", "kind": null}]);

        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 1, "got: {deps:?}");
        assert_eq!(deps[0].pkg_name, "idea-ui");
        assert_eq!(
            deps[0].dep_line,
            format!("{{ git = \"{url}\", tag = \"v0.1.0\" }}")
        );
    }

    #[test]
    fn collect_falls_back_to_manifest_path_when_resolve_root_missing() {
        // A virtual-workspace manifest leaves resolve.root null; we must
        // still find the root by matching the project manifest path.
        let mut meta = sample_metadata();
        meta["resolve"]["root"] = Value::Null;
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        let deps = collect_forced_deps(&meta, &src, Path::new("/proj/Cargo.toml"), "my-app");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].pkg_name, "idea-ui");
    }

    #[test]
    fn collect_returns_empty_on_malformed_metadata() {
        let src = FrameworkSource::Workspace { root: PathBuf::from("/ws") };
        assert!(collect_forced_deps(&json!({}), &src, Path::new("/proj/Cargo.toml"), "my-app").is_empty());
        assert!(collect_forced_deps(&json!({"packages": []}), &src, Path::new("/proj/Cargo.toml"), "my-app").is_empty());
    }

    #[test]
    fn generate_rejects_a_workspace_root() {
        // A bare `[workspace]` with no `[package]` is not a project; the
        // caller turns this Err into a graceful "no catalog" warning.
        let dir = std::env::temp_dir().join(format!(
            "idealyst-catwrap-ws-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert!(generate(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
