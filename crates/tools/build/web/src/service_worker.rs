//! The app's service worker, composed from what its dependency graph
//! contributes.
//!
//! A browser lets one service worker control a scope, so SDKs that each
//! need one (notification clicks, push, background sync, offline caching)
//! cannot each register their own: they would replace each other, and a
//! notification click or push subscription belongs to the worker that
//! created it. Instead every package declares the handler scripts it
//! contributes, and the build composes them into one file served at the
//! bundle root:
//!
//! ```toml
//! [package.metadata.idealyst.web]
//! service_worker = ["sw/notifications.js"]
//! ```
//!
//! Paths resolve against the declaring package's directory, like the
//! Android `runtime_kotlin` / `manifest_application` contributions. The app
//! crate contributes its own handlers the same way. Contributions are plain
//! JavaScript (a worker has no DOM, and loading the app's wasm to answer
//! one event would cost megabytes per wake-up); each runs in its own
//! function scope, labelled with its package, after the framework
//! [`PRELUDE`].
//!
//! Only packages the app actually depends on contribute: the closure is
//! walked from the app's node in `cargo metadata`'s resolve graph,
//! filtered to wasm32 and to normal dependencies, so a workspace sibling or
//! a dev-dependency never ships handlers into the app.
//!
//! The file is NOT fingerprinted: the browser checks a registered worker
//! for updates by re-fetching the same URL and comparing bytes, so the URL
//! must stay stable. The page registers it at boot when the build marks the
//! page ([`marker_script_tag`]); an app that ships its own service worker
//! sets
//!
//! ```toml
//! [package.metadata.idealyst.app.web]
//! service_worker_registration = "import"
//! ```
//!
//! and calls `importScripts("idealyst-sw.js")` from its worker instead —
//! the file is still written, the page just doesn't register it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// The composed worker's file name, at the bundle root. Part of the
/// contract with `idealyst::boot::web` (through [`MARKER_GLOBAL`]) and with
/// an app's own worker that `importScripts` it.
pub const FILE_NAME: &str = "idealyst-sw.js";

/// The page global the build sets to the worker's URL when the page should
/// register it. Read by `idealyst::boot::web`; pinned by
/// `the_marker_names_the_global_the_boot_path_reads`.
pub const MARKER_GLOBAL: &str = "IDEALYST_SERVICE_WORKER";

/// The framework's part of every composed worker, ahead of the
/// contributions.
///
/// - `self.idealyst` is the namespace contributions share (the page/worker
///   channel lands here).
/// - A new version takes over at once (`skipWaiting` + `clients.claim`):
///   the contributions are event handlers, so there is no cache of an old
///   version for an open page to depend on, and a fixed click handler
///   should not wait for every tab to close.
pub const PRELUDE: &str = r#"// Composed by the idealyst web build from the service-worker handlers the
// app's dependencies declare under [package.metadata.idealyst.web]. Do not
// edit: it is rewritten on every build.
"use strict";
self.idealyst = self.idealyst || {};
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
"#;

/// Whether the page registers the composed worker itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Registration {
    /// The page registers `idealyst-sw.js` at boot.
    #[default]
    Auto,
    /// The app's own worker `importScripts` it; the page registers nothing.
    Import,
}

impl Registration {
    /// Parse `[package.metadata.idealyst.app.web].service_worker_registration`.
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value {
            None | Some("auto") => Ok(Registration::Auto),
            Some("import") => Ok(Registration::Import),
            Some(other) => bail!(
                "[package.metadata.idealyst.app.web].service_worker_registration = {other:?}: \
                 expected \"auto\" (the page registers idealyst-sw.js) or \"import\" (your own \
                 service worker calls importScripts(\"idealyst-sw.js\"))"
            ),
        }
    }
}

/// One handler script a package contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    /// The crate that declared it.
    pub package: String,
    /// The path as declared, relative to the package.
    pub rel: String,
    /// Where it was read from (what a dev loop watches).
    pub path: PathBuf,
    pub js: String,
}

/// Run `cargo metadata` for the app at `manifest_path` and collect its
/// dependency closure's contributions.
pub fn discover(manifest_path: &Path) -> Result<Vec<Contribution>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--filter-platform", "wasm32-unknown-unknown"])
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .with_context(|| format!("run cargo metadata for {}", manifest_path.display()))?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed for {}: {}",
            manifest_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout).context("parse cargo metadata json")?;
    collect(&json, manifest_path)
}

/// The pure core of [`discover`]: the contributions of every package the
/// app at `root_manifest` depends on (normal dependencies, transitively),
/// SDKs first in package-name order, the app's own last.
pub fn collect(metadata: &Value, root_manifest: &Path) -> Result<Vec<Contribution>> {
    let packages: BTreeMap<&str, &Value> = metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get("id")?.as_str()?, p)))
        .collect();
    // Compared canonicalized: cargo reports the path it resolved, which can
    // differ from the caller's through a symlink (`/tmp` → `/private/tmp`).
    let canonical = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = canonical(root_manifest);
    let root_id = packages
        .iter()
        .find(|(_, p)| p.get("manifest_path").and_then(Value::as_str).map(|m| canonical(Path::new(m))) == Some(root.clone()))
        .map(|(id, _)| *id)
        .ok_or_else(|| anyhow!("{} is not a package in its cargo metadata", root_manifest.display()))?;

    let mut reachable = BTreeSet::new();
    let nodes: BTreeMap<&str, &Value> = metadata
        .pointer("/resolve/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| Some((n.get("id")?.as_str()?, n)))
        .collect();
    let mut queue = VecDeque::from([root_id]);
    while let Some(id) = queue.pop_front() {
        if !reachable.insert(id) {
            continue;
        }
        let deps = nodes.get(id).and_then(|n| n.get("deps")).and_then(Value::as_array);
        for dep in deps.into_iter().flatten() {
            if is_normal_dependency(dep) {
                if let Some(pkg) = dep.get("pkg").and_then(Value::as_str) {
                    queue.push_back(pkg);
                }
            }
        }
    }

    let mut sdks: Vec<(&str, Vec<Contribution>)> = Vec::new();
    let mut app = Vec::new();
    for id in reachable {
        let Some(pkg) = packages.get(id) else { continue };
        let contributions = package_contributions(pkg)?;
        if contributions.is_empty() {
            continue;
        }
        if id == root_id {
            app = contributions;
        } else {
            let name = pkg.get("name").and_then(Value::as_str).unwrap_or("");
            sdks.push((name, contributions));
        }
    }
    sdks.sort_by(|a, b| a.0.cmp(b.0));
    Ok(sdks.into_iter().flat_map(|(_, c)| c).chain(app).collect())
}

/// A resolve-graph edge the app's build links: a normal (or build)
/// dependency, not a dev-dependency.
fn is_normal_dependency(dep: &Value) -> bool {
    match dep.get("dep_kinds").and_then(Value::as_array) {
        // Pre-1.41 metadata carries no kinds: every edge is a dependency.
        None => true,
        Some(kinds) => kinds.iter().any(|k| matches!(k.get("kind"), None | Some(Value::Null))),
    }
}

fn package_contributions(pkg: &Value) -> Result<Vec<Contribution>> {
    let name = pkg.get("name").and_then(Value::as_str).unwrap_or("<unknown>");
    let Some(list) = pkg.pointer("/metadata/idealyst/web/service_worker") else {
        return Ok(Vec::new());
    };
    let entries = list
        .as_array()
        .ok_or_else(|| anyhow!("{name}: metadata.idealyst.web.service_worker must be an array of paths"))?;
    let dir: PathBuf = pkg
        .get("manifest_path")
        .and_then(Value::as_str)
        .and_then(|p| Path::new(p).parent())
        .ok_or_else(|| anyhow!("package {name} has no manifest_path"))?
        .to_path_buf();
    entries
        .iter()
        .map(|e| {
            let rel = e
                .as_str()
                .ok_or_else(|| anyhow!("{name}: metadata.idealyst.web.service_worker entries must be strings"))?;
            let p = Path::new(rel);
            if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                bail!(
                    "{name}: metadata.idealyst.web.service_worker entry {rel:?} must be relative to \
                     the package and not use `..`"
                );
            }
            let path = dir.join(rel);
            let js = fs::read_to_string(&path).with_context(|| {
                format!(
                    "{name} declares {} (via metadata.idealyst.web.service_worker) but it cannot be read",
                    path.display()
                )
            })?;
            Ok(Contribution { package: name.to_string(), rel: rel.to_string(), path, js })
        })
        .collect()
}

/// The composed worker: the [`PRELUDE`], then each contribution in its own
/// function scope, labelled with its package so a stack trace or a syntax
/// error points at the SDK that shipped it.
pub fn compose(contributions: &[Contribution]) -> String {
    let mut out = String::from(PRELUDE);
    for c in contributions {
        out.push_str(&format!("\n// --- {} : {} ---\n(() => {{\n", c.package, c.rel));
        out.push_str(&c.js);
        if !c.js.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("})();\n");
    }
    out
}

/// The `<script>` that tells the page to register the worker. The URL is
/// relative, so an app served under a sub-path registers the worker beside
/// its own `index.html` (and the worker's default scope is that path).
pub fn marker_script_tag() -> String {
    format!("\n    <script>window.{MARKER_GLOBAL}=\"{FILE_NAME}\";</script>")
}

/// Write (or remove) the composed worker in the staged bundle `staged`.
/// Returns whether a worker was written. A bundle whose graph contributes
/// nothing gets no file — and loses one a previous build staged, so
/// removing the last contributing SDK unregisters nothing stale on the next
/// deploy.
pub fn stage(staged: &Path, contributions: &[Contribution]) -> Result<bool> {
    let path = staged.join(FILE_NAME);
    if contributions.is_empty() {
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove stale {}", path.display()))?;
        }
        return Ok(false);
    }
    fs::write(&path, compose(contributions)).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// A metadata document: `app` depends on `sdk-b` and `sdk-a` (normal),
    /// `sdk-a` on `sdk-deep`, and `test-only` is a dev-dependency; `sibling`
    /// is a workspace member nobody depends on.
    fn metadata(root: &Path) -> Value {
        let pkg = |name: &str, web: Value| {
            json!({
                "id": format!("{name} 0.1.0"),
                "name": name,
                "manifest_path": root.join(name).join("Cargo.toml").to_str().unwrap(),
                "metadata": web,
            })
        };
        let sw = |paths: &[&str]| json!({ "idealyst": { "web": { "service_worker": paths } } });
        let dep = |name: &str, kind: Value| json!({ "pkg": format!("{name} 0.1.0"), "dep_kinds": [{ "kind": kind }] });
        json!({
            "packages": [
                pkg("app", sw(&["sw/app.js"])),
                pkg("sdk-b", sw(&["sw/b.js"])),
                pkg("sdk-a", sw(&["sw/a1.js", "sw/a2.js"])),
                pkg("sdk-deep", sw(&["deep.js"])),
                pkg("test-only", sw(&["t.js"])),
                pkg("sibling", sw(&["s.js"])),
                pkg("plain", Value::Null),
            ],
            "resolve": { "nodes": [
                { "id": "app 0.1.0", "deps": [dep("sdk-b", Value::Null), dep("sdk-a", Value::Null),
                                              dep("test-only", json!("dev")), dep("plain", Value::Null)] },
                { "id": "sdk-a 0.1.0", "deps": [dep("sdk-deep", Value::Null)] },
                { "id": "sdk-b 0.1.0", "deps": [] },
                { "id": "sdk-deep 0.1.0", "deps": [] },
                { "id": "test-only 0.1.0", "deps": [] },
                { "id": "sibling 0.1.0", "deps": [] },
                { "id": "plain 0.1.0", "deps": [] },
            ] },
        })
    }

    fn fixture() -> (tempfile::TempDir, Value) {
        let tmp = tempfile::tempdir().unwrap();
        for (pkg, rel) in [
            ("app", "sw/app.js"),
            ("sdk-b", "sw/b.js"),
            ("sdk-a", "sw/a1.js"),
            ("sdk-a", "sw/a2.js"),
            ("sdk-deep", "deep.js"),
            ("test-only", "t.js"),
            ("sibling", "s.js"),
        ] {
            write(&tmp.path().join(pkg), rel, &format!("// {pkg} {rel}"));
        }
        let meta = metadata(tmp.path());
        (tmp, meta)
    }

    #[test]
    fn the_app_closure_contributes_sdks_by_name_then_the_app() {
        let (tmp, meta) = fixture();
        let got = collect(&meta, &tmp.path().join("app/Cargo.toml")).unwrap();
        let order: Vec<_> = got.iter().map(|c| format!("{}:{}", c.package, c.rel)).collect();
        assert_eq!(order, ["sdk-a:sw/a1.js", "sdk-a:sw/a2.js", "sdk-b:sw/b.js", "sdk-deep:deep.js", "app:sw/app.js"]);
        assert_eq!(got[0].js, "// sdk-a sw/a1.js");
    }

    /// The bug class: a workspace's `cargo metadata` lists every member, so
    /// collecting from `packages` alone would ship a sibling app's (or a
    /// test-only crate's) handlers into this app's worker.
    #[test]
    fn regression_siblings_and_dev_dependencies_contribute_nothing() {
        let (tmp, meta) = fixture();
        let got = collect(&meta, &tmp.path().join("app/Cargo.toml")).unwrap();
        assert!(got.iter().all(|c| c.package != "sibling" && c.package != "test-only"), "{got:?}");
    }

    #[test]
    fn a_missing_file_names_the_package_that_declared_it() {
        let (tmp, meta) = fixture();
        fs::remove_file(tmp.path().join("sdk-b/sw/b.js")).unwrap();
        let err = collect(&meta, &tmp.path().join("app/Cargo.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("sdk-b declares") && msg.contains("b.js"), "{msg}");
    }

    #[test]
    fn paths_must_stay_inside_the_package() {
        let (tmp, mut meta) = fixture();
        meta["packages"][1]["metadata"]["idealyst"]["web"]["service_worker"] = json!(["../escape.js"]);
        let msg = format!("{:#}", collect(&meta, &tmp.path().join("app/Cargo.toml")).unwrap_err());
        assert!(msg.contains("sdk-b") && msg.contains("must be relative"), "{msg}");
    }

    #[test]
    fn compose_scopes_and_labels_each_contribution_after_the_prelude() {
        let js = compose(&[
            Contribution { package: "notifications".into(), rel: "sw/n.js".into(), path: "n.js".into(), js: "const x = 1;".into() },
            Contribution { package: "app".into(), rel: "sw/app.js".into(), path: "app.js".into(), js: "const x = 2;\n".into() },
        ]);
        assert!(js.starts_with(PRELUDE));
        assert!(js.ends_with(
            "\n// --- notifications : sw/n.js ---\n(() => {\nconst x = 1;\n})();\n\n// --- app : sw/app.js ---\n(() => {\nconst x = 2;\n})();\n"
        ), "{js}");
    }

    #[test]
    fn stage_writes_the_worker_and_removes_a_stale_one() {
        let tmp = tempfile::tempdir().unwrap();
        let c = vec![Contribution { package: "p".into(), rel: "a.js".into(), path: "a.js".into(), js: "1;".into() }];
        assert!(stage(tmp.path(), &c).unwrap());
        assert!(fs::read_to_string(tmp.path().join(FILE_NAME)).unwrap().contains("// --- p : a.js ---"));
        assert!(!stage(tmp.path(), &[]).unwrap());
        assert!(!tmp.path().join(FILE_NAME).exists(), "the last contribution removed → no worker");
    }

    #[test]
    fn registration_modes_parse_and_reject_typos() {
        assert_eq!(Registration::parse(None).unwrap(), Registration::Auto);
        assert_eq!(Registration::parse(Some("auto")).unwrap(), Registration::Auto);
        assert_eq!(Registration::parse(Some("import")).unwrap(), Registration::Import);
        assert!(format!("{:#}", Registration::parse(Some("manual")).unwrap_err()).contains("\"manual\""));
    }

    /// The walk against a real `cargo metadata` document (its `dep_kinds`
    /// and id shapes are cargo's, not the synthetic ones above): an app
    /// with a path SDK, a dev-only SDK, and a workspace sibling.
    #[test]
    fn discover_walks_a_real_cargo_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "Cargo.toml", "[workspace]\nmembers = [\"app\", \"sdk\", \"devsdk\", \"sibling\"]\nresolver = \"2\"\n");
        let member = |name: &str, deps: &str, sw: bool| {
            let meta = if sw {
                format!("\n[package.metadata.idealyst.web]\nservice_worker = [\"sw/{name}.js\"]\n")
            } else {
                String::new()
            };
            write(
                root,
                &format!("{name}/Cargo.toml"),
                &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}{meta}"),
            );
            write(root, &format!("{name}/src/lib.rs"), "");
            if sw {
                write(root, &format!("{name}/sw/{name}.js"), &format!("// {name}"));
            }
        };
        member("app", "\n[dependencies]\nsdk = { path = \"../sdk\" }\n\n[dev-dependencies]\ndevsdk = { path = \"../devsdk\" }\n", true);
        member("sdk", "", true);
        member("devsdk", "", true);
        member("sibling", "", true);
        let got = discover(&root.join("app/Cargo.toml")).unwrap();
        let order: Vec<_> = got.iter().map(|c| c.package.as_str()).collect();
        assert_eq!(order, ["sdk", "app"]);
    }

    /// The global's name is a contract with `idealyst::boot::web`, which
    /// reads it to decide whether to register (a test there pins the same
    /// literal).
    #[test]
    fn the_marker_names_the_global_the_boot_path_reads() {
        assert_eq!(marker_script_tag(), "\n    <script>window.IDEALYST_SERVICE_WORKER=\"idealyst-sw.js\";</script>");
    }
}
