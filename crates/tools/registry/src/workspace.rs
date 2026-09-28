//! Loading the workspace and deciding what is publishable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<RawPackage>,
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    /// `cargo metadata` reports `publish` as `Some([])` for `publish = false`,
    /// `Some([registry, ..])` for an allow-list, and `None` for "anywhere".
    publish: Option<Vec<String>>,
    dependencies: Vec<RawDep>,
}

#[derive(Debug, Deserialize)]
struct RawDep {
    name: String,
    kind: Option<String>,
    /// The version requirement. `cargo metadata` reports `"*"` for a
    /// dependency that names no `version`.
    #[serde(default)]
    req: String,
    /// The registry index URL for a dependency that names `registry = …`;
    /// `None` for crates.io and for a path-only dependency.
    #[serde(default)]
    registry: Option<String>,
}

impl RawDep {
    /// Does `cargo package` resolve this dependency from a registry?
    ///
    /// A normal or build dependency always is: cargo refuses to package one
    /// that is path-only, and the release floors every internal one. A
    /// dev-dependency is resolved only when it carries a version (or names a
    /// registry, which the workspace deps always pair with a version). A
    /// path-only dev-dep is stripped from the packaged manifest and never
    /// touches the registry at all.
    ///
    /// `cargo metadata` has already folded `x.workspace = true` into the
    /// member's own entry and flattened every `[target.'…'.*]` table into the
    /// same list (with a `target` field), so both need no special handling.
    fn resolved_from_registry(&self) -> bool {
        self.kind.as_deref() != Some("dev") || self.registry.is_some() || self.req != "*"
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: semver::Version,
    pub manifest_path: PathBuf,
    /// Directory the crate owns, relative to the workspace root. This is the
    /// path we ask git "did anything here change?" about.
    pub rel_dir: String,
    pub publish: bool,
    /// Directories of other workspace members nested INSIDE this crate's
    /// directory — `crates/sdk/client/dnd/examples/kanban-demo` under
    /// `crates/sdk/client/dnd`, and so on.
    ///
    /// "Did anything change here?" is asked of git by path, and a nested
    /// member's files sit under its parent's path. Without excluding them, an
    /// edit to a demo republishes the SDK it demonstrates — and if that SDK is
    /// something like `runtime-shared`, every consumer rebuilds most of the
    /// framework for a change that is not in the crate at all.
    pub nested: Vec<String>,
    /// Workspace-internal dependencies, dev-dependencies excluded.
    ///
    /// This is the set that decides what gets RELEASED (a major bump here
    /// drags this crate into the plan). Dev-deps never do: a published
    /// crate's dev-dependencies are not built by any consumer, so a sibling's
    /// new major changes nothing a consumer sees.
    pub deps: BTreeSet<String>,
    /// Workspace-internal dev-dependencies that `cargo package` resolves from
    /// the registry — the ones that carry a version, usually inherited via
    /// `x = { workspace = true }`. These constrain publish ORDER only.
    ///
    /// Path-only dev-deps are left out: cargo strips them when packaging, so
    /// they neither constrain order nor force a republish. Including them
    /// would create false cycles — `wire` dev-depends on `dev-client`, which
    /// depends on `wire`, and `idea-ui` dev-depends on `premint-dump`, which
    /// is not published at all.
    pub dev_deps: BTreeSet<String>,
}

pub struct Workspace {
    pub root: PathBuf,
    pub packages: BTreeMap<String, Package>,
}

impl Workspace {
    pub fn load(manifest_dir: &Path) -> Result<Self> {
        let out = Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(manifest_dir)
            .output()
            .context("running `cargo metadata`")?;
        if !out.status.success() {
            bail!(
                "`cargo metadata` failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let meta: Metadata =
            serde_json::from_slice(&out.stdout).context("parsing `cargo metadata` output")?;

        let members: BTreeSet<&str> = meta.workspace_members.iter().map(|s| s.as_str()).collect();
        let member_names: BTreeSet<String> = meta
            .packages
            .iter()
            .filter(|p| members.contains(p.id.as_str()))
            .map(|p| p.name.clone())
            .collect();

        let mut packages = BTreeMap::new();
        for p in &meta.packages {
            if !members.contains(p.id.as_str()) {
                continue;
            }
            let dir = p
                .manifest_path
                .parent()
                .context("manifest with no parent directory")?;
            let rel_dir = dir
                .strip_prefix(&meta.workspace_root)
                .unwrap_or(dir)
                .to_string_lossy()
                .replace('\\', "/");
            let internal = |d: &&RawDep| member_names.contains(&d.name) && d.name != p.name;
            let deps = p
                .dependencies
                .iter()
                .filter(internal)
                .filter(|d| d.kind.as_deref() != Some("dev"))
                .map(|d| d.name.clone())
                .collect();
            let dev_deps = p
                .dependencies
                .iter()
                .filter(internal)
                .filter(|d| d.kind.as_deref() == Some("dev") && d.resolved_from_registry())
                .map(|d| d.name.clone())
                .collect();
            packages.insert(
                p.name.clone(),
                Package {
                    name: p.name.clone(),
                    nested: Vec::new(),
                    version: semver::Version::parse(&p.version)
                        .with_context(|| format!("{} has a non-semver version", p.name))?,
                    manifest_path: p.manifest_path.clone(),
                    rel_dir,
                    // `Some([])` is `publish = false`. Anything else is publishable.
                    publish: p.publish.as_ref().map(|v| !v.is_empty()).unwrap_or(true),
                    deps,
                    dev_deps,
                },
            );
        }

        // Second pass: a crate's nested members are only knowable once every
        // member's directory is in hand.
        let dirs: Vec<(String, String)> = packages
            .values()
            .map(|p| (p.name.clone(), p.rel_dir.clone()))
            .collect();
        for p in packages.values_mut() {
            let prefix = format!("{}/", p.rel_dir);
            p.nested = dirs
                .iter()
                .filter(|(name, dir)| name != &p.name && dir.starts_with(&prefix))
                .map(|(_, dir)| dir.clone())
                .collect();
        }

        Ok(Workspace {
            root: meta.workspace_root,
            packages,
        })
    }

    pub fn publishable(&self) -> impl Iterator<Item = &Package> {
        self.packages.values().filter(|p| p.publish)
    }

    /// Publishable crates in the order `cargo package` can package them —
    /// a crate always appears after everything it resolves from the registry,
    /// because packaging runs against the live registry and a missing
    /// requirement kills the run mid-publish.
    ///
    /// That is every normal and build dependency, and every VERSIONED
    /// dev-dependency whose target is part of this release (`releasing`).
    /// `cargo package` resolves a versioned dev-dep like any other; during
    /// the 2026-09-24 release `wire` (dev-dep `runtime-macros`, versioned via
    /// the workspace dep) was packaged before the new `runtime-macros` was
    /// uploaded and the run died.
    ///
    /// A dev edge to a crate NOT being released is dropped: its current
    /// version is already in the registry and the floor still names it, so it
    /// resolves no matter when the dependent is packaged. That matters for
    /// cycles. Cargo permits a dev-dependency cycle (`A` dev-depends on `B`,
    /// `B` depends on `A`), and when both carry versions and both are being
    /// released, neither can be packaged first — each needs the other's new
    /// version in the registry. No order solves that, so it is refused here,
    /// before anything is packaged or uploaded, naming the dev edge to make
    /// path-only. A cycle through a crate this release leaves alone is not a
    /// cycle for the purposes of this release and is ignored.
    pub fn publish_order(&self, releasing: &dyn Fn(&str) -> bool) -> Result<Vec<&Package>> {
        let mut indeg: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for p in self.publishable() {
            let n = indeg.entry(p.name.as_str()).or_insert(0);
            let before: BTreeSet<&str> = p
                .deps
                .iter()
                .filter(|d| self.is_publishable(d))
                .chain(
                    p.dev_deps
                        .iter()
                        .filter(|d| self.is_publishable(d) && releasing(d)),
                )
                .map(String::as_str)
                .collect();
            for d in before {
                *n += 1;
                dependents.entry(d).or_default().push(&p.name);
            }
        }
        let mut ready: Vec<&str> = indeg
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();
        ready.sort_unstable();

        let mut order = Vec::new();
        while let Some(n) = ready.pop() {
            order.push(&self.packages[n]);
            for &dep in dependents.get(n).into_iter().flatten() {
                let e = indeg.get_mut(dep).expect("dependent is publishable");
                *e -= 1;
                if *e == 0 {
                    ready.push(dep);
                }
            }
        }
        if order.len() != indeg.len() {
            let mut stuck: Vec<&str> = indeg
                .iter()
                .filter(|(_, &d)| d > 0)
                .map(|(&n, _)| n)
                .collect();
            stuck.sort_unstable();
            // Normal/build cycles are rejected by cargo itself, so a cycle
            // here runs through a versioned dev-dep. Name those edges.
            let dev_edges: Vec<String> = stuck
                .iter()
                .flat_map(|&n| {
                    self.packages[n]
                        .dev_deps
                        .iter()
                        .filter(|d| stuck.contains(&d.as_str()) && releasing(d))
                        .map(move |d| format!("{n} -> {d}"))
                })
                .collect();
            bail!(
                "dependency cycle among crates in this release, cannot order a publish: {}\n\
                 versioned dev-dependencies in the cycle: {}\n\
                 `cargo package` resolves a dev-dependency that carries a version from the \
                 registry, so neither side can be packaged first. Make the dev-dependency \
                 path-only (`{{ path = \"…\" }}`, no version, no `workspace = true`) — cargo \
                 strips those when packaging.",
                stuck.join(", "),
                if dev_edges.is_empty() { "none".to_string() } else { dev_edges.join(", ") }
            );
        }
        Ok(order)
    }

    pub fn is_publishable(&self, name: &str) -> bool {
        self.packages.get(name).is_some_and(|p| p.publish)
    }

    /// Every publishable crate that depends, transitively, on any of `seeds`.
    pub fn dependents_of<'a>(&'a self, seeds: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
        let mut rev: HashMap<&str, Vec<&str>> = HashMap::new();
        for p in self.publishable() {
            for d in &p.deps {
                rev.entry(d.as_str()).or_default().push(&p.name);
            }
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<&str> = seeds.into_iter().collect();
        while let Some(c) = stack.pop() {
            for &r in rev.get(c).into_iter().flatten() {
                if seen.insert(r.to_string()) {
                    stack.push(r);
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out a throwaway workspace and load it through `cargo metadata`,
    /// so the test exercises the same classification a release does —
    /// workspace inheritance and target tables included.
    fn fixture(name: &str, root_manifest: &str, members: &[(&str, &str)]) -> (PathBuf, Workspace) {
        let d = std::env::temp_dir().join(format!("registry-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".cargo")).unwrap();
        // `registry = "idealyst"` must name a known registry for cargo to
        // parse the manifests. `--no-deps` never contacts it.
        std::fs::write(
            d.join(".cargo/config.toml"),
            "[registries.idealyst]\nindex = \"sparse+https://crates.example.invalid/index/\"\n",
        )
        .unwrap();
        std::fs::write(d.join("Cargo.toml"), root_manifest).unwrap();
        for (dir, manifest) in members {
            std::fs::create_dir_all(d.join(dir).join("src")).unwrap();
            std::fs::write(d.join(dir).join("Cargo.toml"), manifest).unwrap();
            std::fs::write(d.join(dir).join("src/lib.rs"), "").unwrap();
        }
        let ws = Workspace::load(&d).unwrap();
        (d, ws)
    }

    fn member(name: &str, rest: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n{rest}")
    }

    fn position(order: &[&Package], name: &str) -> usize {
        order.iter().position(|p| p.name == name).unwrap_or_else(|| panic!("{name} not ordered"))
    }

    /// Regression: the 2026-09-24 release packaged `wire` before
    /// `runtime-macros`. `wire` dev-depends on `runtime-macros` through
    /// `{ workspace = true }`, which carries a version and `registry =
    /// "idealyst"`, so `cargo package -p wire` resolved it from the live
    /// registry — where the new `runtime-macros` did not exist yet — and the
    /// run died mid-publish. Ordering looked only at `[dependencies]`.
    ///
    /// The fixture also carries the two dev-dep shapes that must NOT order:
    /// a path-only dev-dep closing a cycle (`wire` -> `dev-client` ->
    /// `wire`, which is how the real tree looks), and a target-scoped
    /// versioned one, which must.
    #[test]
    fn regression_versioned_dev_dependency_orders_before_dependent() {
        let root = r#"
[workspace]
resolver = "2"
members = ["runtime-macros", "runtime-template", "wire", "dev-client", "dev-overlay"]

[workspace.dependencies]
runtime-macros = { path = "runtime-macros", version = "1.0.0", registry = "idealyst" }
runtime-template = { path = "runtime-template", version = "1.0.0", registry = "idealyst" }
wire = { path = "wire", version = "1.0.0", registry = "idealyst" }
"#;
        let wire = member(
            "wire",
            r#"
[dev-dependencies]
runtime-macros = { workspace = true }
dev-client = { path = "../dev-client" }

[target.'cfg(unix)'.dev-dependencies]
runtime-template = { workspace = true }
"#,
        );
        let dev_client = member("dev-client", "[dependencies]\nwire = { workspace = true }\n");
        let dev_overlay = member("dev-overlay", "[dev-dependencies]\nwire = { workspace = true }\n");
        let (d, ws) = fixture(
            "devdep-order",
            root,
            &[
                ("runtime-macros", &member("runtime-macros", "")),
                ("runtime-template", &member("runtime-template", "")),
                ("wire", &wire),
                ("dev-client", &dev_client),
                ("dev-overlay", &dev_overlay),
            ],
        );

        assert_eq!(
            ws.packages["wire"].dev_deps,
            BTreeSet::from(["runtime-macros".to_string(), "runtime-template".to_string()]),
            "the path-only dev-dep on dev-client must not count"
        );
        // Selection is untouched: `deps` still drives the plan and still
        // holds normal/build deps only.
        assert!(ws.packages["wire"].deps.is_empty());
        assert!(ws.packages["dev-overlay"].deps.is_empty());

        let order = ws.publish_order(&|_| true).unwrap();
        assert!(position(&order, "runtime-macros") < position(&order, "wire"));
        assert!(position(&order, "runtime-template") < position(&order, "wire"));
        assert!(position(&order, "wire") < position(&order, "dev-overlay"));
        assert!(position(&order, "wire") < position(&order, "dev-client"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn bare(pkgs: &[(&str, &[&str], &[&str])]) -> Workspace {
        let set = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        Workspace {
            root: PathBuf::from("/w"),
            packages: pkgs
                .iter()
                .map(|(n, deps, dev)| {
                    (
                        n.to_string(),
                        Package {
                            name: n.to_string(),
                            version: semver::Version::new(1, 0, 0),
                            manifest_path: PathBuf::from(format!("/w/{n}/Cargo.toml")),
                            rel_dir: n.to_string(),
                            publish: true,
                            nested: vec![],
                            deps: set(deps),
                            dev_deps: set(dev),
                        },
                    )
                })
                .collect(),
        }
    }

    /// `a` dev-depends (versioned) on `b`, `b` depends on `a` — legal in
    /// cargo. Released together, neither can be packaged first, so the
    /// order refuses up front and names the dev edge to make path-only.
    #[test]
    fn a_versioned_dev_dependency_cycle_in_one_release_is_refused() {
        let ws = bare(&[("a", &[], &["b"]), ("b", &["a"], &[])]);
        let err = ws.publish_order(&|_| true).unwrap_err().to_string();
        assert!(err.contains("a -> b"), "error must name the dev edge: {err}");
        assert!(err.contains("path-only"), "error must say how to fix it: {err}");
    }

    /// The same cycle when `b` is not being released: `b`'s published
    /// version already satisfies `a`'s dev-dep, so there is nothing to wait
    /// for and the edge is dropped.
    #[test]
    fn a_dev_edge_to_a_crate_outside_the_release_does_not_order() {
        let ws = bare(&[("a", &[], &["b"]), ("b", &["a"], &[])]);
        let order = ws.publish_order(&|n| n == "a").unwrap();
        let names: Vec<&str> = order.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    /// Dev edges order the publish; they never select what is published.
    /// A major bump on `b` drags in `c` (normal dep) but not `a` (dev dep).
    #[test]
    fn dev_dependencies_do_not_drag_dependents_into_a_release() {
        let ws = bare(&[("a", &[], &["b"]), ("b", &[], &[]), ("c", &["b"], &[])]);
        assert_eq!(ws.dependents_of(["b"]), BTreeSet::from(["c".to_string()]));
    }
}
