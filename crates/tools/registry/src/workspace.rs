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
    /// Workspace-internal dependencies, dev-dependencies excluded.
    ///
    /// Dev-deps are excluded deliberately: cargo strips a path-only dev-dep
    /// when packaging, so it neither constrains publish order nor forces a
    /// republish. Including them would create false cycles — `idea-ui`
    /// dev-depends on `premint-dump`, which is not published at all.
    pub deps: BTreeSet<String>,
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
            let deps = p
                .dependencies
                .iter()
                .filter(|d| {
                    member_names.contains(&d.name) && d.kind.as_deref() != Some("dev")
                })
                .map(|d| d.name.clone())
                .collect();
            packages.insert(
                p.name.clone(),
                Package {
                    name: p.name.clone(),
                    version: semver::Version::parse(&p.version)
                        .with_context(|| format!("{} has a non-semver version", p.name))?,
                    manifest_path: p.manifest_path.clone(),
                    rel_dir,
                    // `Some([])` is `publish = false`. Anything else is publishable.
                    publish: p.publish.as_ref().map(|v| !v.is_empty()).unwrap_or(true),
                    deps,
                },
            );
        }

        Ok(Workspace {
            root: meta.workspace_root,
            packages,
        })
    }

    pub fn publishable(&self) -> impl Iterator<Item = &Package> {
        self.packages.values().filter(|p| p.publish)
    }

    /// Publishable crates in dependency order — a crate always appears after
    /// everything it depends on, because a registry rejects an index entry
    /// whose dependencies are not yet resolvable.
    pub fn publish_order(&self) -> Result<Vec<&Package>> {
        let mut indeg: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for p in self.publishable() {
            let n = indeg.entry(p.name.as_str()).or_insert(0);
            for d in p.deps.iter().filter(|d| self.is_publishable(d)) {
                *n += 1;
                dependents.entry(d.as_str()).or_default().push(&p.name);
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
            let stuck: Vec<&str> = indeg
                .iter()
                .filter(|(_, &d)| d > 0)
                .map(|(&n, _)| n)
                .collect();
            bail!(
                "dependency cycle among publishable crates, cannot order a publish: {}",
                stuck.join(", ")
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
