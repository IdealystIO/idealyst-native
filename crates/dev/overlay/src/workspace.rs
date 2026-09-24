//! The decision across every crate of the app's workspace.
//!
//! [`crate::decide`] answers "patch or rebuild?" for ONE crate against
//! that crate's descriptor set. An app of any size is several crates: the
//! app crate (the TIP — the one `cargo build --bin` names) plus the
//! library crates of its workspace it depends on (CrewForge's
//! `crewforge-ui-shared`: 39k lines, 57 files with `ui!`). A save in one
//! of those is as patchable as a save in the tip, and this module is what
//! lets the dev loop say so.
//!
//! # What a hot patch of a library crate has to re-emit
//!
//! A patch is linked from freshly compiled objects and spliced into the
//! running program; everything it does not define it borrows from the
//! program. The jump table then redirects the functions the running
//! program reaches INDIRECTLY — `#[component]` bodies, through
//! `__hot::call`. A DIRECT call in already-linked code cannot be
//! redirected.
//!
//! For the tip that is enough: nothing depends on it, so every way into
//! its new code runs through a redirected component. A library crate is
//! called directly by its dependents — the app calls a shared `fn`, a
//! stylesheet's `<name>_style()`, a builder method. Re-emitting only the
//! library would leave all of those calls in the dependents' base code,
//! pointing at the OLD copies. So a patch re-emits the edited crates AND
//! every crate of the workspace that depends on one of them (up to the
//! tip), and links all of their objects into ONE module: a dependent's
//! call to the library resolves inside the patch, to the new body, and
//! the dependent's own components are redirected as usual.
//! [`Workspace::replay_set`] computes that set.
//!
//! # What still cannot be patched in a library crate
//!
//! A replay emits objects only. A dependent's replay therefore reads the
//! library's METADATA from the base build, and for some functions rustc
//! compiles the body out of that metadata into the dependent — generic
//! ones, `#[inline]`, `const fn`, `async fn`, `impl Trait`, trait default
//! bodies (see `runtime_macros_parse::downstream_bodies`). An edit to one
//! of those in a library crate would reach the library's own callers and
//! miss every dependent's copy, silently. The archive records a digest per
//! such function ([`crate::archive::FileDigest::downstream`]) and a save
//! that moves one rebuilds, naming it ([`Reason::DownstreamBody`]).
//!
//! # What is in the workspace
//!
//! Local packages that are members of the tip's cargo workspace and are
//! in the tip's dependency closure. A local package OUTSIDE the workspace
//! — a `[patch]` pointing the framework at a checkout — is watched (see
//! `dev-reload`'s `watch_roots`) but not patchable: cargo builds those
//! with the `package."*"` profile (opt-level 3 in `--dev-opt optimized`),
//! and a save there rebuilds ([`Reason::OutsideWorkspace`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::archive::DescriptorSet;
use crate::decide::{decide_with, ChangedFile, Decision, Reason, SitePatch};

/// One crate of the workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceCrate {
    /// `[package] name`, as `CARGO_PKG_NAME` spells it.
    pub package: String,
    /// The directory holding its `Cargo.toml`.
    pub dir: PathBuf,
    /// Its library target's crate name as rustc sees it (`--crate-name`),
    /// which is what a captured invocation is keyed by. The tip's LIBRARY
    /// too: that is where an app's components live.
    pub crate_name: String,
    /// The workspace crates it depends on directly (normal dependencies
    /// only; a dev- or build-dependency is not in the wasm program).
    pub deps: BTreeSet<String>,
    /// The descriptor set its last scan produced. `None` means "cannot
    /// tell", and any save in the crate rebuilds.
    pub archive: Option<DescriptorSet>,
}

/// The app's workspace, as the dev loop decides saves against it.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The app crate's package name.
    pub tip: String,
    /// Every patchable crate, the tip included, by package name.
    pub crates: BTreeMap<String, WorkspaceCrate>,
    /// Local packages in the tip's closure that are NOT workspace
    /// members, with their directories. Watched, never patched.
    pub outside: BTreeMap<String, PathBuf>,
}

/// Where a saved path belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A file of a workspace crate, with its package-relative path.
    Crate { package: String, relative: String },
    /// A file of a local package outside the workspace.
    Outside { package: String, relative: String },
    /// Neither: not a `.rs` file, or not under any known crate.
    Unknown,
}

/// A saved file, routed to its crate.
pub struct SavedFile {
    pub package: String,
    pub file: ChangedFile,
}

/// What to do with a save, across the workspace.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceDecision {
    Unchanged,
    /// Overlay patches only. Site keys carry the package, so one list
    /// serves every crate.
    Patch(Vec<SitePatch>),
    HotPatch(HotPatchPlan),
    Rebuild(Reason),
}

/// A hot patch, planned.
#[derive(Debug, Clone, PartialEq)]
pub struct HotPatchPlan {
    /// Packages whose sources the save moved.
    pub edited: Vec<String>,
    /// Every package the patch re-emits, dependencies first: the edited
    /// crates and every workspace crate depending on one of them. See the
    /// module docs.
    pub replay: Vec<String>,
    /// The saved files, for the log (`<package>/<path>` outside the tip).
    pub files: Vec<String>,
}

impl Workspace {
    /// Build the workspace from a `cargo metadata --format-version 1`
    /// document for the tip's manifest. Archives start empty; see
    /// [`Self::rescan`].
    ///
    /// The tip is the package whose manifest sits in `tip_dir`. Its
    /// closure follows NORMAL dependency edges from the resolve graph,
    /// so a crate only a dev- or build-dependency pulls in is left out.
    /// `None` when the document names no package in `tip_dir`.
    pub fn from_metadata(meta: &serde_json::Value, tip_dir: &Path) -> Option<Workspace> {
        let tip_dir = canonical(tip_dir);
        let members: BTreeSet<&str> = meta["workspace_members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m.as_str())
            .collect();

        struct Pkg<'a> {
            name: String,
            dir: PathBuf,
            local: bool,
            crate_name: Option<String>,
            deps: Vec<&'a str>,
        }
        let mut pkgs: BTreeMap<&str, Pkg<'_>> = BTreeMap::new();
        for p in meta["packages"].as_array().into_iter().flatten() {
            let (Some(id), Some(name), Some(manifest)) =
                (p["id"].as_str(), p["name"].as_str(), p["manifest_path"].as_str())
            else {
                continue;
            };
            let dir = canonical(Path::new(manifest).parent().unwrap_or(Path::new(".")));
            let crate_name = p["targets"].as_array().into_iter().flatten().find_map(|t| {
                let kinds: Vec<&str> =
                    t["kind"].as_array().into_iter().flatten().filter_map(|k| k.as_str()).collect();
                let is_lib = kinds
                    .iter()
                    .any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "cdylib" | "staticlib"));
                is_lib.then(|| t["name"].as_str().map(|n| n.replace('-', "_"))).flatten()
            });
            pkgs.insert(
                id,
                Pkg {
                    name: name.to_string(),
                    dir,
                    local: p.get("source").is_none_or(|s| s.is_null()),
                    crate_name,
                    deps: Vec::new(),
                },
            );
        }
        for node in meta.pointer("/resolve/nodes").and_then(|n| n.as_array()).into_iter().flatten() {
            let Some(id) = node["id"].as_str() else { continue };
            let normal: Vec<&str> = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| {
                    // `kind: null` is a normal dependency; "dev" and
                    // "build" are not part of the program.
                    d["dep_kinds"].as_array().is_none_or(|kinds| {
                        kinds.is_empty() || kinds.iter().any(|k| k["kind"].is_null())
                    })
                })
                .filter_map(|d| d["pkg"].as_str())
                .collect();
            if let Some(p) = pkgs.get_mut(id) {
                p.deps = normal;
            }
        }

        let tip_id = pkgs.iter().find(|(_, p)| p.local && p.dir == tip_dir).map(|(id, _)| *id)?;

        // The tip's closure, local packages only.
        let mut closure: BTreeSet<&str> = BTreeSet::new();
        let mut stack = vec![tip_id];
        while let Some(id) = stack.pop() {
            if !closure.insert(id) {
                continue;
            }
            if let Some(p) = pkgs.get(id) {
                stack.extend(p.deps.iter().copied().filter(|d| pkgs.get(d).is_some_and(|p| p.local)));
            }
        }

        let patchable: BTreeSet<&str> = closure
            .iter()
            .copied()
            .filter(|id| {
                *id == tip_id || (members.contains(id) && pkgs[id].crate_name.is_some())
            })
            .collect();

        let mut crates = BTreeMap::new();
        let mut outside = BTreeMap::new();
        for id in &closure {
            let p = &pkgs[id];
            if !p.local {
                continue;
            }
            if !patchable.contains(id) {
                outside.insert(p.name.clone(), p.dir.clone());
                continue;
            }
            let deps = p
                .deps
                .iter()
                .filter(|d| patchable.contains(*d))
                .map(|d| pkgs[d].name.clone())
                .collect();
            crates.insert(
                p.name.clone(),
                WorkspaceCrate {
                    package: p.name.clone(),
                    dir: p.dir.clone(),
                    crate_name: p
                        .crate_name
                        .clone()
                        .unwrap_or_else(|| p.name.replace('-', "_")),
                    deps,
                    archive: None,
                },
            );
        }
        Some(Workspace { tip: pkgs[tip_id].name.clone(), crates, outside })
    }

    /// A workspace of the tip alone — what a session gets when `cargo
    /// metadata` could not be read. Exactly the old single-crate
    /// behavior: a save in any other crate is `Route::Unknown` and
    /// rebuilds.
    pub fn single(package: &str, dir: &Path, crate_name: &str) -> Workspace {
        let mut crates = BTreeMap::new();
        crates.insert(
            package.to_string(),
            WorkspaceCrate {
                package: package.to_string(),
                dir: canonical(dir),
                crate_name: crate_name.to_string(),
                deps: BTreeSet::new(),
                archive: None,
            },
        );
        Workspace { tip: package.to_string(), crates, outside: BTreeMap::new() }
    }

    /// Which crate a saved path belongs to.
    ///
    /// Longest directory wins: a workspace root is often itself a crate
    /// (the tip) with library crates in subdirectories, and a file in
    /// `lab-shared/src/` is lab-shared's, not the root crate's
    /// `lab-shared/src/…`.
    pub fn route(&self, path: &Path) -> Route {
        if path.extension().is_none_or(|e| e != "rs") {
            return Route::Unknown;
        }
        let path = canonical(path);
        let mut best: Option<(usize, &str, bool, &Path)> = None;
        let candidates = self
            .crates
            .values()
            .map(|c| (c.package.as_str(), true, c.dir.as_path()))
            .chain(self.outside.iter().map(|(p, d)| (p.as_str(), false, d.as_path())));
        for (package, patchable, dir) in candidates {
            if path.starts_with(dir) {
                let depth = dir.components().count();
                if best.is_none_or(|(d, ..)| depth > d) {
                    best = Some((depth, package, patchable, dir));
                }
            }
        }
        let Some((_, package, patchable, dir)) = best else { return Route::Unknown };
        let relative = path
            .strip_prefix(dir)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if patchable {
            Route::Crate { package: package.to_string(), relative }
        } else {
            Route::Outside { package: package.to_string(), relative }
        }
    }

    /// Decide a save across the workspace.
    ///
    /// Each crate's files go through [`decide_with`] against that crate's
    /// own archive, then the answers combine with the most expensive
    /// winning, as within one crate. On top of that, a LIBRARY crate's
    /// body edit must not move a function its dependents compile from
    /// metadata (see the module docs).
    ///
    /// A save that is a hot patch anywhere re-emits every crate that
    /// changed at all, including one whose only edit was a literal the
    /// overlay could have carried: the overlay patch would be addressed to
    /// the tree the hot patch is about to rebuild.
    pub fn decide(&self, saved: &[SavedFile], premint: bool) -> WorkspaceDecision {
        let mut by_package: BTreeMap<&str, Vec<&SavedFile>> = BTreeMap::new();
        for s in saved {
            by_package.entry(s.package.as_str()).or_default().push(s);
        }

        let mut patches = Vec::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut body_edited = false;
        let mut files = Vec::new();
        for (package, saved) in &by_package {
            let Some(krate) = self.crates.get(*package) else {
                let file = saved.first().map(|s| s.file.path.clone()).unwrap_or_default();
                return WorkspaceDecision::Rebuild(Reason::OutsideWorkspace {
                    file: format!("{package}/{file}"),
                });
            };
            let Some(archive) = krate.archive.as_ref() else {
                return WorkspaceDecision::Rebuild(Reason::NoArchive);
            };
            let changed: Vec<ChangedFile> = saved
                .iter()
                .map(|s| ChangedFile { path: s.file.path.clone(), text: s.file.text.clone() })
                .collect();
            match decide_with(Some(archive), &changed, premint) {
                Decision::Unchanged => {}
                Decision::Patch(mut p) => {
                    patches.append(&mut p);
                    touched.insert(package.to_string());
                }
                Decision::HotPatch(_) => {
                    body_edited = true;
                    touched.insert(package.to_string());
                }
                Decision::Rebuild(why) => {
                    return WorkspaceDecision::Rebuild(self.qualify(package, why));
                }
            }
            for s in saved {
                files.push(self.display(package, &s.file.path));
            }
        }

        if !body_edited {
            return if patches.is_empty() {
                WorkspaceDecision::Unchanged
            } else {
                WorkspaceDecision::Patch(patches)
            };
        }

        // A library crate's edit must leave every body its dependents
        // compile from metadata exactly as the base build saw it.
        for package in &touched {
            if *package == self.tip {
                continue;
            }
            let archive = self.crates[package].archive.as_ref().expect("checked above");
            for s in &by_package[package.as_str()] {
                let recorded = archive.files.get(&s.file.path);
                let now = crate::archive::downstream_digests(&s.file.text);
                let empty = BTreeMap::new();
                let before = recorded.map(|r| &r.downstream).unwrap_or(&empty);
                if let Some(item) = first_difference(before, &now) {
                    return WorkspaceDecision::Rebuild(Reason::DownstreamBody {
                        file: self.display(package, &s.file.path),
                        item,
                    });
                }
            }
        }

        let replay = self.replay_set(&touched);
        files.sort();
        WorkspaceDecision::HotPatch(HotPatchPlan { edited: touched.into_iter().collect(), replay, files })
    }

    /// The edited crates plus every workspace crate that depends on one
    /// of them, transitively, dependencies before dependents (ties by
    /// name, so the order is stable). See the module docs for why the
    /// dependents are in it.
    pub fn replay_set(&self, edited: &BTreeSet<String>) -> Vec<String> {
        let mut set: BTreeSet<&str> =
            edited.iter().map(String::as_str).filter(|p| self.crates.contains_key(*p)).collect();
        loop {
            let before = set.len();
            for c in self.crates.values() {
                if c.deps.iter().any(|d| set.contains(d.as_str())) {
                    set.insert(c.package.as_str());
                }
            }
            if set.len() == before {
                break;
            }
        }
        // Kahn's algorithm over the induced subgraph.
        let mut order = Vec::with_capacity(set.len());
        let mut placed: BTreeSet<&str> = BTreeSet::new();
        while placed.len() < set.len() {
            let ready: Vec<&str> = set
                .iter()
                .copied()
                .filter(|p| !placed.contains(p))
                .filter(|p| {
                    self.crates[*p]
                        .deps
                        .iter()
                        .all(|d| !set.contains(d.as_str()) || placed.contains(d.as_str()))
                })
                .collect();
            if ready.is_empty() {
                // A cycle cargo would not have resolved; place the rest
                // by name rather than loop forever.
                order.extend(set.iter().filter(|p| !placed.contains(*p)).map(|p| p.to_string()));
                break;
            }
            for p in ready {
                placed.insert(p);
                order.push(p.to_string());
            }
        }
        order
    }

    /// Re-scan these crates' sources into fresh archives, writing each
    /// descriptor set under `project_root` (see
    /// [`crate::archive::crate_overlay_dir`]). A crate whose scan fails is
    /// left with no archive, so its next save rebuilds.
    pub fn rescan<'a>(&mut self, project_root: &Path, packages: impl IntoIterator<Item = &'a str>) {
        let packages: Vec<String> = packages.into_iter().map(str::to_string).collect();
        for package in packages {
            let dir = crate::archive::crate_overlay_dir(project_root, &self.tip, &package);
            let Some(krate) = self.crates.get_mut(&package) else { continue };
            krate.archive = match crate::archive::write_into(&dir, &krate.dir) {
                Ok(set) => Some(set),
                Err(e) => {
                    eprintln!("[dev-reload] no descriptor set for {package}: {e}");
                    None
                }
            };
        }
    }

    /// Re-scan every crate. After a rebuild, and at session start.
    pub fn rescan_all(&mut self, project_root: &Path) {
        let all: Vec<String> = self.crates.keys().cloned().collect();
        self.rescan(project_root, all.iter().map(String::as_str));
    }

    /// A digest of the crate's sources as its archive last saw them
    /// ([`DescriptorSet::build_key`]): every file's content. `None` when
    /// the crate has no archive.
    ///
    /// The patch builder keys the objects of a crate it re-emitted only
    /// to carry a dependency's edit by this: a crate whose sources have
    /// not moved since its last replay, against the same base, compiles
    /// to the same objects, so the replay can be skipped. An overlay
    /// patch advances the archive, and with it this key, so a literal
    /// edited since is never lost to a reused object.
    pub fn source_key(&self, package: &str) -> Option<String> {
        self.crates.get(package)?.archive.as_ref().map(DescriptorSet::build_key)
    }

    /// Fold an applied OVERLAY patch into each touched crate's archive.
    /// See [`crate::decide::advance_archive`].
    pub fn advance(&mut self, saved: &[SavedFile]) {
        let mut by_package: BTreeMap<&str, Vec<ChangedFile>> = BTreeMap::new();
        for s in saved {
            by_package.entry(s.package.as_str()).or_default().push(ChangedFile {
                path: s.file.path.clone(),
                text: s.file.text.clone(),
            });
        }
        for (package, changed) in by_package {
            if let Some(archive) = self.crates.get_mut(package).and_then(|c| c.archive.as_mut()) {
                crate::decide::advance_archive(archive, &changed);
            }
        }
    }

    /// The rustc crate name a captured invocation is keyed by.
    pub fn crate_name(&self, package: &str) -> Option<&str> {
        self.crates.get(package).map(|c| c.crate_name.as_str())
    }

    fn display(&self, package: &str, path: &str) -> String {
        if package == self.tip {
            path.to_string()
        } else {
            format!("{package}/{path}")
        }
    }

    /// A reason decided within one crate, with its file named so a log
    /// line says which crate it is in.
    fn qualify(&self, package: &str, why: Reason) -> Reason {
        if package == self.tip {
            return why;
        }
        let q = |file: String| format!("{package}/{file}");
        match why {
            Reason::UnknownFile { file } => Reason::UnknownFile { file: q(file) },
            Reason::DoesNotParse { file } => Reason::DoesNotParse { file: q(file) },
            Reason::CodeChanged { file } => Reason::CodeChanged { file: q(file) },
            Reason::ShapeChanged { file } => Reason::ShapeChanged { file: q(file) },
            Reason::PremintStylesheet { file } => Reason::PremintStylesheet { file: q(file) },
            other => other,
        }
    }
}

/// The first label whose digest differs between two maps (present in one
/// only, or different in both).
fn first_difference(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Option<String> {
    a.keys()
        .chain(b.keys())
        .find(|k| a.get(*k) != b.get(*k))
        .cloned()
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-crate workspace on disk: `app` (the tip) depends on
    /// `shared`, and `shared` lives in a subdirectory of the tip — the
    /// layout of the lab, where routing by longest prefix matters.
    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        ws: Workspace,
    }

    const SHARED: &str = r#"
use runtime_core::*;

#[component]
pub fn SharedCard(title: String) -> Element {
    ui! { view() { text { "shared {title}" } } }
}

pub fn shared_line(n: i32) -> String {
    format!("v1 {}", n)
}

pub fn wrap<T: Clone>(t: T) -> Vec<T> {
    vec![t]
}

#[inline]
pub fn quick() -> u32 {
    1
}
"#;

    const APP: &str = r#"
use runtime_core::*;

#[component]
fn Root() -> Element {
    ui! { view() { text { "root" } } }
}

pub fn wrap_here<T: Clone>(t: T) -> Vec<T> {
    vec![t]
}
"#;

    fn write_crate(dir: &Path, name: &str, lib: &str) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), lib).unwrap();
    }

    /// A `cargo metadata` document for the fixture: the tip, the shared
    /// crate (a member), an OUTSIDE local package (a `[patch]`ed
    /// framework crate), a dev-dependency-only member, and a registry
    /// crate.
    fn metadata(root: &Path) -> serde_json::Value {
        let m = |p: &Path| p.join("Cargo.toml").display().to_string();
        serde_json::json!({
            "packages": [
                {"id": "app", "name": "app", "manifest_path": m(root), "source": null,
                 "targets": [{"kind": ["rlib"], "name": "app"}, {"kind": ["bin"], "name": "app"}]},
                {"id": "shared", "name": "lab-shared", "manifest_path": m(&root.join("lab-shared")), "source": null,
                 "targets": [{"kind": ["lib"], "name": "lab_shared"}]},
                {"id": "testkit", "name": "testkit", "manifest_path": m(&root.join("testkit")), "source": null,
                 "targets": [{"kind": ["lib"], "name": "testkit"}]},
                {"id": "fw", "name": "runtime-core", "manifest_path": m(&root.join("fw")), "source": null,
                 "targets": [{"kind": ["lib"], "name": "runtime_core"}]},
                {"id": "serde", "name": "serde", "manifest_path": "/reg/serde/Cargo.toml",
                 "source": "registry+https://github.com/rust-lang/crates.io-index",
                 "targets": [{"kind": ["lib"], "name": "serde"}]}
            ],
            "workspace_members": ["app", "shared", "testkit"],
            "resolve": {
                "root": "app",
                "nodes": [
                    {"id": "app", "deps": [
                        {"pkg": "shared", "dep_kinds": [{"kind": null}]},
                        {"pkg": "fw", "dep_kinds": [{"kind": null}]},
                        {"pkg": "testkit", "dep_kinds": [{"kind": "dev"}]}
                    ]},
                    {"id": "shared", "deps": [
                        {"pkg": "fw", "dep_kinds": [{"kind": null}]},
                        {"pkg": "serde", "dep_kinds": [{"kind": null}]}
                    ]},
                    {"id": "testkit", "deps": []},
                    {"id": "fw", "deps": []},
                    {"id": "serde", "deps": []}
                ]
            }
        })
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write_crate(&root, "app", APP);
        write_crate(&root.join("lab-shared"), "lab-shared", SHARED);
        write_crate(&root.join("fw"), "runtime-core", "pub fn f() {}\n");
        let mut ws = Workspace::from_metadata(&metadata(&root), &root).expect("the tip is found");
        ws.rescan_all(&root);
        Fixture { _tmp: tmp, root, ws }
    }

    fn saved(f: &Fixture, rel: &str, text: String) -> Vec<SavedFile> {
        match f.ws.route(&f.root.join(rel)) {
            Route::Crate { package, relative } | Route::Outside { package, relative } => {
                vec![SavedFile { package, file: ChangedFile { path: relative, text } }]
            }
            Route::Unknown => panic!("{rel} routed nowhere"),
        }
    }

    #[test]
    fn the_workspace_is_the_tips_closure_of_members() {
        let f = fixture();
        assert_eq!(f.ws.tip, "app");
        assert_eq!(f.ws.crates.keys().collect::<Vec<_>>(), vec!["app", "lab-shared"]);
        assert_eq!(f.ws.crates["app"].crate_name, "app");
        assert_eq!(f.ws.crates["lab-shared"].crate_name, "lab_shared");
        assert_eq!(f.ws.crates["app"].deps, BTreeSet::from(["lab-shared".to_string()]));
        // A dev-dependency is not in the program; a local package outside
        // the workspace is watched but never patched.
        assert!(!f.ws.crates.contains_key("testkit"));
        assert_eq!(f.ws.outside.keys().collect::<Vec<_>>(), vec!["runtime-core"]);
    }

    /// Regression: a workspace root that is itself the tip holds its
    /// library crates in subdirectories. Routing by "under the tip's
    /// directory" filed `lab-shared/src/lib.rs` as the tip's file and
    /// every shared save rebuilt as an unknown file.
    #[test]
    fn routing_picks_the_deepest_crate() {
        let f = fixture();
        assert_eq!(
            f.ws.route(&f.root.join("lab-shared/src/lib.rs")),
            Route::Crate { package: "lab-shared".into(), relative: "src/lib.rs".into() }
        );
        assert_eq!(
            f.ws.route(&f.root.join("src/lib.rs")),
            Route::Crate { package: "app".into(), relative: "src/lib.rs".into() }
        );
        assert_eq!(
            f.ws.route(&f.root.join("fw/src/lib.rs")),
            Route::Outside { package: "runtime-core".into(), relative: "src/lib.rs".into() }
        );
        assert_eq!(f.ws.route(&f.root.join("Cargo.toml")), Route::Unknown);
    }

    /// The tier's reason to exist: a body edit in a library crate is a hot
    /// patch, and the plan re-emits the crate AND the tip that calls it.
    #[test]
    fn a_body_edit_in_a_library_crate_hot_patches_it_and_its_dependents() {
        let f = fixture();
        let edit = SHARED.replace("v1 {}", "v2 {}");
        match f.ws.decide(&saved(&f, "lab-shared/src/lib.rs", edit), false) {
            WorkspaceDecision::HotPatch(plan) => {
                assert_eq!(plan.edited, vec!["lab-shared"]);
                assert_eq!(plan.replay, vec!["lab-shared", "app"], "dependencies first");
                assert_eq!(plan.files, vec!["lab-shared/src/lib.rs"]);
            }
            other => panic!("expected a hot patch, got {other:?}"),
        }
    }

    /// A tip-only edit replays the tip alone: nothing depends on it.
    #[test]
    fn a_body_edit_in_the_tip_replays_only_the_tip() {
        let f = fixture();
        let edit = APP.replace("vec![t]", "vec![t.clone(), t]");
        match f.ws.decide(&saved(&f, "src/lib.rs", edit), false) {
            WorkspaceDecision::HotPatch(plan) => {
                assert_eq!(plan.replay, vec!["app"]);
                assert_eq!(plan.files, vec!["src/lib.rs"]);
            }
            other => panic!("expected a hot patch, got {other:?}"),
        }
    }

    #[test]
    fn a_shape_edit_in_a_library_crate_rebuilds_and_names_the_crate() {
        let f = fixture();
        let edit = SHARED.replace("SharedCard(title: String)", "SharedCard(title: String, n: i32)");
        assert_eq!(
            f.ws.decide(&saved(&f, "lab-shared/src/lib.rs", edit), false),
            WorkspaceDecision::Rebuild(Reason::ShapeChanged { file: "lab-shared/src/lib.rs".into() })
        );
    }

    /// A generic body in a library crate is compiled by its dependents
    /// out of the BASE metadata, so re-emitting them would still run the
    /// old body: rebuild, naming the function.
    #[test]
    fn a_generic_body_edit_in_a_library_crate_rebuilds() {
        let f = fixture();
        let edit = SHARED.replace("vec![t]\n", "vec![t.clone(), t]\n");
        assert_eq!(
            f.ws.decide(&saved(&f, "lab-shared/src/lib.rs", edit), false),
            WorkspaceDecision::Rebuild(Reason::DownstreamBody {
                file: "lab-shared/src/lib.rs".into(),
                item: "fn wrap".into(),
            })
        );
    }

    #[test]
    fn an_inline_body_edit_in_a_library_crate_rebuilds() {
        let f = fixture();
        let edit = SHARED.replace("    1\n}", "    2\n}");
        assert_eq!(
            f.ws.decide(&saved(&f, "lab-shared/src/lib.rs", edit), false),
            WorkspaceDecision::Rebuild(Reason::DownstreamBody {
                file: "lab-shared/src/lib.rs".into(),
                item: "fn quick".into(),
            })
        );
    }

    /// The same generic edit in the TIP is fine: nothing downstream.
    #[test]
    fn a_generic_body_edit_in_the_tip_still_hot_patches() {
        let f = fixture();
        let edit = APP.replace("vec![t]", "vec![t.clone(), t]");
        assert!(matches!(
            f.ws.decide(&saved(&f, "src/lib.rs", edit), false),
            WorkspaceDecision::HotPatch(_)
        ));
    }

    /// A literal in a library crate's `ui!` is an overlay patch, keyed by
    /// the LIBRARY's package — the site key the library's compiled tags
    /// carry.
    #[test]
    fn a_literal_edit_in_a_library_crate_is_an_overlay_patch() {
        let f = fixture();
        let edit = SHARED.replace("\"shared {title}\"", "\"common {title}\"");
        match f.ws.decide(&saved(&f, "lab-shared/src/lib.rs", edit), false) {
            WorkspaceDecision::Patch(p) => {
                assert_eq!(p.len(), 1);
                let key = f.ws.crates["lab-shared"].archive.as_ref().unwrap().sites[0].key;
                assert_eq!(p[0].site, key);
            }
            other => panic!("expected an overlay patch, got {other:?}"),
        }
    }

    /// A save that is a literal in one crate and a body in another is one
    /// hot patch that re-emits both.
    #[test]
    fn a_literal_here_and_a_body_there_is_one_hot_patch_over_both() {
        let f = fixture();
        let mut both = saved(&f, "src/lib.rs", APP.replace("\"root\"", "\"root!\""));
        both.extend(saved(&f, "lab-shared/src/lib.rs", SHARED.replace("v1 {}", "v2 {}")));
        match f.ws.decide(&both, false) {
            WorkspaceDecision::HotPatch(plan) => {
                assert_eq!(plan.edited, vec!["app", "lab-shared"]);
                assert_eq!(plan.replay, vec!["lab-shared", "app"]);
            }
            other => panic!("expected a hot patch, got {other:?}"),
        }
    }

    #[test]
    fn a_save_outside_the_workspace_rebuilds() {
        let f = fixture();
        assert_eq!(
            f.ws.decide(&saved(&f, "fw/src/lib.rs", "pub fn f() { }\n".into()), false),
            WorkspaceDecision::Rebuild(Reason::OutsideWorkspace {
                file: "runtime-core/src/lib.rs".into()
            })
        );
    }

    /// Each crate's archive is its own descriptor set, keyed by its own
    /// package, under the project's staging dir.
    #[test]
    fn each_crate_has_its_own_archive() {
        let f = fixture();
        assert_eq!(f.ws.crates["lab-shared"].archive.as_ref().unwrap().package, "lab-shared");
        assert_eq!(f.ws.crates["app"].archive.as_ref().unwrap().package, "app");
        // A library's archives live inside the APP's overlay dir, not in a
        // `target/idealyst/<package>/` of their own: that tree is per app.
        let dir = crate::archive::crate_overlay_dir(&f.root, "app", "lab-shared");
        assert!(dir.starts_with(crate::archive::overlay_dir(&f.root, "app")), "{}", dir.display());
        assert!(crate::decide::load_archive_from(&dir).is_some());
        assert!(!crate::archive::overlay_dir(&f.root, "lab-shared").exists());
    }

    /// A three-level chain replays in dependency order and includes every
    /// transitive dependent, but not a sibling the edit does not reach.
    #[test]
    fn the_replay_set_is_the_transitive_dependents_in_order() {
        let mut ws = Workspace::single("app", Path::new("/w/app"), "app");
        let mut add = |name: &str, deps: &[&str]| {
            ws.crates.insert(
                name.into(),
                WorkspaceCrate {
                    package: name.into(),
                    dir: PathBuf::from(format!("/w/{name}")),
                    crate_name: name.replace('-', "_"),
                    deps: deps.iter().map(|d| d.to_string()).collect(),
                    archive: None,
                },
            );
        };
        add("core", &[]);
        add("ui-shared", &["core"]);
        add("api", &["core"]);
        add("app", &["ui-shared", "api"]);
        assert_eq!(
            ws.replay_set(&BTreeSet::from(["ui-shared".to_string()])),
            vec!["ui-shared", "app"]
        );
        assert_eq!(
            ws.replay_set(&BTreeSet::from(["core".to_string()])),
            vec!["core", "api", "ui-shared", "app"]
        );
    }
}
