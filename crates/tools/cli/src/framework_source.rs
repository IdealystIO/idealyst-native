//! CLI-side glue around `build_ios::FrameworkSource`.
//!
//! The CLI bakes the framework's git URL + refspec at compile time
//! (`crates/cli/build.rs`) and is the only place that knows them.
//! When dispatching a command, we hand `FrameworkSource::detect` the
//! defaults so it can fall back to git when no workspace is found.
//!
//! Refspec is either a tag (preferred — `tag = "v0.1.0"`) or a
//! commit hash. `build.rs` picks based on whether HEAD is tagged.
//! Either form can be overridden at runtime via env vars:
//! `IDEALYST_FRAMEWORK_GIT_TAG`, `IDEALYST_FRAMEWORK_GIT_REV`, or
//! `IDEALYST_FRAMEWORK_GIT_URL`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use build_ios::{FrameworkSource, FrameworkVersions, GitDefaults, GitRef, RegistryDefaults};

/// Absolutize a project dir for framework-source ancestor walking,
/// tolerating VM-mapped shares.
///
/// The commands normally `fs::canonicalize` the project dir so
/// `find_framework_workspace` has real ancestor components to inspect
/// (a bare `.` has none). But `canonicalize` issues a volume /
/// final-path query that fails with "the volume does not contain a
/// recognized file system" (os error 1005) on some virtio-fs / 9p VM
/// mounts — including the `Z:` share the framework's own Windows dev
/// VM uses. When canonicalize fails, fall back to a purely lexical
/// absolute path (`current_dir` + `join`): it still carries the
/// ancestor components detection walks; it just doesn't resolve
/// symlinks / `..`. On normal filesystems canonicalize succeeds and
/// behavior is unchanged.
pub fn abs_project_dir(dir: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(dir) {
        Ok(p) => Ok(p),
        Err(_) if dir.is_absolute() => Ok(dir.to_path_buf()),
        Err(_) => Ok(std::env::current_dir()
            .with_context(|| format!("cannot resolve project dir {}", dir.display()))?
            .join(dir)),
    }
}

fn git_defaults() -> GitDefaults {
    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| env!("IDEALYST_FRAMEWORK_GIT_URL_DEFAULT").to_string());

    // Runtime overrides win — tag beats rev when both are set,
    // matching the build.rs ordering.
    let refspec = if let Ok(tag) = std::env::var("IDEALYST_FRAMEWORK_GIT_TAG") {
        if !tag.is_empty() {
            GitRef::Tag(tag)
        } else {
            compile_time_refspec()
        }
    } else if let Ok(rev) = std::env::var("IDEALYST_FRAMEWORK_GIT_REV") {
        if !rev.is_empty() {
            GitRef::Rev(rev)
        } else {
            compile_time_refspec()
        }
    } else {
        compile_time_refspec()
    };

    GitDefaults { url, refspec }
}

/// The refspec `build.rs` captured into the binary. `KIND` is the
/// cargo dep-table key (`rev`, `tag`, `branch`); `VALUE` is the
/// corresponding string.
fn compile_time_refspec() -> GitRef {
    let kind = env!("IDEALYST_FRAMEWORK_GIT_REF_KIND_DEFAULT");
    let value = env!("IDEALYST_FRAMEWORK_GIT_REF_VALUE_DEFAULT").to_string();
    match kind {
        "tag" => GitRef::Tag(value),
        "branch" => GitRef::Branch(value),
        // Default + "rev" both land here so an unknown KIND (forward-
        // compat / future variants) degrades to commit-pinning.
        _ => GitRef::Rev(value),
    }
}

/// Registry defaults for scaffolding, overridable at runtime.
///
/// A fresh project pins the framework by VERSION, not by git rev. A git pin
/// makes every framework release change each package's source id, which
/// invalidates every fingerprint and rebuilds the consumer's whole graph —
/// the problem the registry exists to solve.
pub fn registry_defaults() -> RegistryDefaults {
    RegistryDefaults {
        name: std::env::var("IDEALYST_REGISTRY_NAME")
            .unwrap_or_else(|_| env!("IDEALYST_REGISTRY_NAME_DEFAULT").to_string()),
        index: std::env::var("IDEALYST_REGISTRY_INDEX")
            .unwrap_or_else(|_| env!("IDEALYST_REGISTRY_INDEX_DEFAULT").to_string()),
        versions: framework_versions(FRAMEWORK_MANIFEST),
    }
}

/// The framework's root `Cargo.toml` as of this build: the workspace's own
/// in a dev build or `--git` install, the copy the release staged in a
/// registry install (see build.rs, "Package assets").
const FRAMEWORK_MANIFEST: &str = include_str!(env!("IDEALYST_FRAMEWORK_MANIFEST"));

/// Every framework crate's version, from `[workspace.dependencies]`.
///
/// An entry with both `path` and `version` is a published crate — the
/// release writes its floor there for every bump — and the path is the
/// subpath wrapper generators name it by. Path-only entries are
/// unpublished and stay out, so a registry-mode generator that names one
/// fails at the call instead of in a manifest the user never wrote.
fn framework_versions(manifest: &str) -> FrameworkVersions {
    let doc: toml::Table = manifest.parse().expect("the embedded framework manifest is valid TOML");
    let Some(deps) = doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
    else {
        return FrameworkVersions::default();
    };
    let entries: Vec<(&str, &str, &str)> = deps
        .iter()
        .filter_map(|(key, dep)| {
            let t = dep.as_table()?;
            let path = t.get("path")?.as_str()?;
            let version = t.get("version")?.as_str()?;
            let name = t.get("package").and_then(|p| p.as_str()).unwrap_or(key);
            Some((path, name, version))
        })
        .collect();
    FrameworkVersions::new(entries)
}

/// Resolve the framework source for `project_dir`. Single entry point
/// used by every command handler that produces wrapper Cargo.tomls.
pub fn resolve(project_dir: &Path) -> Result<FrameworkSource> {
    FrameworkSource::detect(project_dir, git_defaults(), registry_defaults())
}

/// The cargo target root the CLI's own side builds live under — the
/// dev server's `idealyst-dev-server/`, the catalog's `idealyst-mcp/`.
///
/// - Framework as a local path ([`FrameworkSource::Workspace`]): the
///   checkout's `target/`, shared by every project built against it so
///   the framework graph compiles once.
/// - Otherwise: the target dir cargo reports for the project's
///   workspace. `known` is that value when the caller already ran a full
///   `cargo metadata`; without it a `--no-deps` metadata call asks. Falls
///   back to `<project>/target` only when cargo can't answer at all.
///
/// NOT [`FrameworkSource::cargo_target_dir`] for a registry/git project:
/// that is `<crate dir>/target`, so two apps of one workspace each got a
/// private copy. Measured in CrewForge (2026-10-08): `app-main` and
/// `app-checkin` declare the SAME server package, and its dev build sat
/// in both `crates/<app>/target/idealyst-dev-server/` (27 + 22 GB, ~11 GB
/// live) — identical units, built twice.
pub fn cli_target_root(source: &FrameworkSource, project_dir: &Path, known: Option<PathBuf>) -> PathBuf {
    if source.is_workspace() {
        return source.cargo_target_dir(project_dir);
    }
    known
        .or_else(|| cargo_workspace_target_dir(project_dir))
        .unwrap_or_else(|| source.cargo_target_dir(project_dir))
}

/// `target_directory` from a `--no-deps` `cargo metadata` of the workspace
/// `project_dir` belongs to — honours `CARGO_TARGET_DIR` and
/// `.cargo/config.toml` exactly as every other cargo call in that tree
/// does. `None` when cargo can't run or the manifest doesn't parse.
pub fn cargo_workspace_target_dir(project_dir: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(project_dir.join("Cargo.toml"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    meta.get("target_directory")?.as_str().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every framework crate a generator names with `source.dep("…")` must
    /// have a registry version, or a registry-sourced project hits the
    /// `dep` panic the first time that generator runs. Scans the CLI and
    /// the build/run crates, and checks against the table this binary
    /// embeds. Crates that are `publish = false` are exempt — they cannot
    /// be registry deps at all, and their generators are workspace-only.
    #[test]
    fn every_published_crate_a_generator_names_has_a_version() {
        let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|a| a.join("crates/runtime/core/Cargo.toml").is_file())
        else {
            return; // a packaged crate, outside the workspace
        };
        let versions = registry_defaults().versions;
        let mut subpaths = std::collections::BTreeSet::new();
        let mut stack = vec![root.join("crates/tools")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "target") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap();
                    for rest in src.split(".dep(\"").skip(1) {
                        if let Some(sub) = rest.split('"').next() {
                            subpaths.insert(sub.to_string());
                        }
                    }
                }
            }
        }
        assert!(subpaths.contains("crates/backend/web"), "scan found {subpaths:?}");
        for sub in subpaths {
            let Ok(manifest) = std::fs::read_to_string(root.join(&sub).join("Cargo.toml")) else {
                continue; // a test fixture's made-up path
            };
            let doc: toml::Table = manifest.parse().unwrap();
            if doc["package"].get("publish").and_then(|p| p.as_bool()) == Some(false) {
                continue;
            }
            assert!(
                versions.for_subpath(&sub).is_some(),
                "{sub} is published but has no `version` in [workspace.dependencies]"
            );
        }
    }

    /// The table carries each crate's OWN major.minor, so crates on
    /// different majors are each pinned correctly — the bug a single
    /// shared number had (`backend-web` 2.x scaffolded as `"1.5"`).
    #[test]
    fn each_crate_keeps_its_own_version() {
        let v = framework_versions(
            r#"
[workspace.dependencies]
runtime-core = { path = "crates/runtime/core", version = "1.11.0", registry = "idealyst" }
backend-web = { path = "crates/backend/web", version = "2.5.0", registry = "idealyst" }
wasm-split = { path = "crates/tools/wasm-split/wasm-split", package = "wasm-splitter", version = "1.5.2", registry = "idealyst" }
lint = { path = "crates/tools/lint" }
serde = { version = "1" }
"#,
        );
        assert_eq!(v.for_subpath("crates/runtime/core"), Some("1.11"));
        assert_eq!(v.for_subpath("crates/backend/web"), Some("2.5"));
        assert_eq!(v.for_package("wasm-splitter"), Some("1.5"), "a renamed dep is keyed by its package");
        assert_eq!(v.for_subpath("crates/tools/lint"), None, "path-only = unpublished");
        assert_eq!(v.for_package("serde"), None, "not a framework crate");
    }

    /// The embedded table matches the workspace it was built from: every
    /// crate's requirement admits the version its own manifest declares.
    #[test]
    fn the_embedded_table_admits_each_crates_own_version() {
        let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|a| a.join("crates/runtime/core/Cargo.toml").is_file())
        else {
            return;
        };
        let versions = registry_defaults().versions;
        for sub in ["crates/runtime/core", "crates/idealyst", "crates/backend/web", "crates/runtime/world"] {
            let doc: toml::Table = std::fs::read_to_string(root.join(sub).join("Cargo.toml")).unwrap().parse().unwrap();
            let own = doc["package"]["version"].as_str().unwrap();
            let req = versions.for_subpath(sub).unwrap_or_else(|| panic!("{sub} missing"));
            let (major, minor) = req.split_once('.').unwrap();
            let mut it = own.split('.');
            assert_eq!(it.next(), Some(major), "{sub}: {own} vs ^{req}");
            assert!(it.next().unwrap().parse::<u64>().unwrap() >= minor.parse::<u64>().unwrap(), "{sub}: {own} vs ^{req}");
        }
    }
}
