//! Comment-preserving manifest edits.
//!
//! Every write here goes through `toml_edit` rather than a text substitution.
//! These manifests carry a lot of load-bearing prose — the rationale for a
//! pinned tract version, why a dep is declared per-target, why there is no
//! allocator feature — and a release must not disturb one byte of it.

use std::path::Path;

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table};

pub fn read(path: &Path) -> Result<DocumentMut> {
    std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))
}

pub fn write(path: &Path, doc: &DocumentMut) -> Result<()> {
    std::fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))
}

/// Pin a crate's own `version`, replacing `version.workspace = true`.
///
/// Per-crate versions are the entire point of the migration: as long as every
/// crate inherits one workspace version, they all move together and a consumer
/// re-resolves — and so rebuilds — the whole graph on every release.
pub fn set_package_version(doc: &mut DocumentMut, v: &semver::Version) -> Result<()> {
    let pkg = doc
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .context("manifest has no [package] table")?;
    pkg.insert("version", value(v.to_string()));
    Ok(())
}

/// Set `version` and `registry` on a `[workspace.dependencies]` entry, adding
/// the entry if it is missing.
///
/// The requirement is a caret on major.minor (`1.5`), not the exact version.
/// An exact pin would force every dependent to be republished for a patch it
/// does not care about — which is the git-tag behaviour this whole exercise
/// exists to escape.
///
/// `registry` is NOT optional decoration. Without it cargo resolves these
/// names against crates.io, where most of them are taken by unrelated crates —
/// `wasm-split-macro`, `css`, `wire`, `net`, `table` and a dozen more all
/// exist there. A published crate would then depend on a stranger's package
/// that merely shares a name. Packaging fails loudly in the lucky cases (the
/// versions do not match) and silently succeeds in the unlucky ones, so this
/// key is what makes the whole scheme safe.
pub fn set_workspace_dep_version(
    doc: &mut DocumentMut,
    name: &str,
    v: &semver::Version,
    rel_path: &str,
    registry: &str,
) -> Result<bool> {
    let deps = doc
        .get_mut("workspace")
        .and_then(|w| w.get_mut("dependencies"))
        .and_then(Item::as_table_like_mut)
        .context("root manifest has no [workspace.dependencies] table")?;

    let req = format!("{}.{}", v.major, v.minor);

    // A dependency may be RENAMED: `wasm-split = { path = "…", package =
    // "wasm-splitter" }`. The table key is then the alias, not the package
    // name, so a plain lookup misses it — and the "not found" branch would
    // add a SECOND entry for the same path while leaving the real one
    // unversioned. Resolve the alias first.
    let key = deps
        .iter()
        .find(|(k, item)| {
            *k != name
                && item
                    .as_table_like()
                    .and_then(|t| t.get("package"))
                    .and_then(|p| p.as_str())
                    == Some(name)
        })
        .map(|(k, _)| k.to_string())
        .unwrap_or_else(|| name.to_string());
    let name = key.as_str();

    match deps.get_mut(name) {
        Some(item) => {
            let t = item.as_table_like_mut().with_context(|| {
                format!("[workspace.dependencies] {name} is not a table — cannot add a version")
            })?;
            if t.get("path").is_none() {
                bail!("[workspace.dependencies] {name} has no `path`; refusing to touch it");
            }
            let existing = t.get("version").and_then(|i| i.as_str()).map(str::to_owned);
            let has_registry =
                t.get("registry").and_then(|i| i.as_str()) == Some(registry);
            if existing.as_deref() == Some(req.as_str()) && has_registry {
                return Ok(false);
            }
            t.insert("version", value(req));
            t.insert("registry", value(registry));
            // Inserting into an inline table appends after the previous
            // value's trailing decor, which renders as `"path" , version`.
            // `fmt` normalizes the whole entry to `{ k = v, k = v }`.
            if let Some(inline) = item.as_inline_table_mut() {
                inline.fmt();
            }
            Ok(true)
        }
        None => {
            let mut t = Table::new().into_inline_table();
            t.insert("path", rel_path.into());
            t.insert("version", req.into());
            t.insert("registry", registry.into());
            deps.insert(name, value(t));
            Ok(true)
        }
    }
}

/// Give a literal `path = "…"` dependency inside a member manifest a version
/// and a registry.
///
/// Cargo refuses to package a crate whose non-dev dependencies are path-only:
/// the published artifact would have no way to name them. Most of this
/// workspace already routes internal deps through `{ workspace = true }`, so
/// this covers the handful that spell the path directly — and they need the
/// `registry` key for exactly the reason described on
/// [`set_workspace_dep_version`]. `offload` -> `offload-macro` was the case
/// that caught this: with a version but no registry, cargo went looking on
/// crates.io.
pub fn version_literal_path_deps(
    doc: &mut DocumentMut,
    versions: &dyn Fn(&str) -> Option<semver::Version>,
    registry: &str,
) -> Result<Vec<String>> {
    let mut touched = Vec::new();
    for section in ["dependencies", "build-dependencies"] {
        collect(doc.as_table_mut(), section, versions, registry, &mut touched)?;
    }
    Ok(touched)
}

/// Walk `[dependencies]` and every `[target.'…'.dependencies]` table.
fn collect(
    table: &mut Table,
    section: &str,
    versions: &dyn Fn(&str) -> Option<semver::Version>,
    registry: &str,
    touched: &mut Vec<String>,
) -> Result<()> {
    if let Some(deps) = table.get_mut(section).and_then(Item::as_table_like_mut) {
        let names: Vec<String> = deps.iter().map(|(k, _)| k.to_string()).collect();
        for name in names {
            let Some(t) = deps.get_mut(&name).and_then(Item::as_table_like_mut) else {
                continue;
            };
            if t.get("path").is_none() {
                continue;
            }
            let Some(v) = versions(&name) else { continue };
            // A version without a registry is worse than neither: cargo
            // resolves the name against crates.io.
            if t.get("version").is_some() && t.get("registry").is_some() {
                continue;
            }
            t.insert("version", value(format!("{}.{}", v.major, v.minor)));
            t.insert("registry", value(registry));
            // Normalize `{ path = "x" , version = … }` back to one space per
            // separator; inserting appends after the previous value's decor.
            if let Some(inline) = deps.get_mut(&name).and_then(Item::as_inline_table_mut) {
                inline.fmt();
            }
            touched.push(name);
        }
    }
    if let Some(targets) = table.get_mut("target").and_then(Item::as_table_like_mut) {
        let keys: Vec<String> = targets.iter().map(|(k, _)| k.to_string()).collect();
        for k in keys {
            if let Some(t) = targets.get_mut(&k).and_then(Item::as_table_mut) {
                collect(t, section, versions, registry, touched)?;
            }
        }
    }
    Ok(())
}

/// What `[workspace.dependencies]` says about an internal crate, minus the
/// version — everything a member needs to spell the dependency itself.
pub struct WsDep {
    pub rel_path: String,
    pub extras: Vec<(String, toml_edit::Value)>,
}

/// Rewrite internal `[dev-dependencies]` from `{ workspace = true }` to a
/// literal, version-less `{ path = "…" }`.
///
/// Cargo drops a path-only dev-dependency when it packages a crate, because
/// the published artifact never builds anyone's tests. A dev-dep that carries
/// a version requirement is NOT dropped — cargo tries to resolve it from the
/// registry instead, and that turns a legal dev-dependency CYCLE into an
/// unpublishable workspace. `wire` dev-depends on `dev-client`, which depends
/// on `wire`; with versions inherited from `[workspace.dependencies]`, neither
/// can ever be packaged first.
///
/// Versions on internal dev-deps buy nothing in exchange, so the rule is
/// simply that they do not have any.
pub fn delink_internal_dev_deps(
    doc: &mut DocumentMut,
    lookup: &dyn Fn(&str) -> Option<WsDep>,
) -> Result<Vec<String>> {
    let mut touched = Vec::new();
    delink_in(doc.as_table_mut(), lookup, &mut touched);
    if let Some(targets) = doc
        .as_table_mut()
        .get_mut("target")
        .and_then(Item::as_table_like_mut)
    {
        let keys: Vec<String> = targets.iter().map(|(k, _)| k.to_string()).collect();
        for k in keys {
            if let Some(t) = targets.get_mut(&k).and_then(Item::as_table_mut) {
                delink_in(t, lookup, &mut touched);
            }
        }
    }
    Ok(touched)
}

fn delink_in(
    table: &mut Table,
    lookup: &dyn Fn(&str) -> Option<WsDep>,
    touched: &mut Vec<String>,
) {
    let Some(deps) = table
        .get_mut("dev-dependencies")
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    let names: Vec<String> = deps.iter().map(|(k, _)| k.to_string()).collect();
    for name in names {
        let Some(item) = deps.get_mut(&name) else { continue };
        let Some(t) = item.as_table_like() else { continue };
        if t.get("workspace").and_then(|w| w.as_bool()) != Some(true) {
            continue;
        }
        let Some(ws) = lookup(&name) else { continue };

        let mut out = toml_edit::InlineTable::new();
        out.insert("path", ws.rel_path.clone().into());
        for (k, v) in &ws.extras {
            out.insert(k, v.clone());
        }
        // The member's own overrides win over the workspace-level ones.
        for (k, v) in t.iter() {
            if k == "workspace" {
                continue;
            }
            if let Some(v) = v.as_value() {
                out.insert(k, v.clone());
            }
        }
        out.fmt();
        *item = value(out);
        touched.push(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_dep(path: &str) -> WsDep {
        WsDep { rel_path: path.into(), extras: vec![] }
    }

    /// Regression: once `[workspace.dependencies]` carried versions, an
    /// internal dev-dep inherited one — and cargo stopped stripping it at
    /// package time, so `wire` (dev-depends on `dev-client`) and `dev-client`
    /// (depends on `wire`) could neither be published first. Internal
    /// dev-deps must stay version-less.
    #[test]
    fn regression_internal_dev_deps_lose_their_version() {
        let mut doc: DocumentMut = r#"
[dev-dependencies]
# keep this comment
dev-client = { workspace = true, features = ["dev-hot-reload"] }
tungstenite = { version = "0.24" }
"#
        .parse()
        .unwrap();
        let touched =
            delink_internal_dev_deps(&mut doc, &|n| (n == "dev-client").then(|| ws_dep("../client")))
                .unwrap();
        assert_eq!(touched, vec!["dev-client"]);
        let out = doc.to_string();
        assert!(out.contains(r#"dev-client = { path = "../client", features = ["dev-hot-reload"] }"#), "{out}");
        assert!(!out.contains("workspace = true"));
        // An external dev-dep is untouched, and comments survive.
        assert!(out.contains(r#"tungstenite = { version = "0.24" }"#));
        assert!(out.contains("# keep this comment"));
    }

    #[test]
    fn delinking_reaches_target_scoped_dev_deps() {
        let mut doc: DocumentMut = r#"
[target.'cfg(unix)'.dev-dependencies]
host-mock = { workspace = true }
"#
        .parse()
        .unwrap();
        let touched =
            delink_internal_dev_deps(&mut doc, &|n| (n == "host-mock").then(|| ws_dep("../mock")))
                .unwrap();
        assert_eq!(touched, vec!["host-mock"]);
        assert!(doc.to_string().contains(r#"host-mock = { path = "../mock" }"#));
    }

    /// The requirement is a caret on major.minor, never the exact version —
    /// an exact pin would force every dependent to be republished for a patch
    /// it does not care about, which is the git-tag behaviour being escaped.
    #[test]
    fn workspace_dep_requirements_are_caret_major_minor() {
        let mut doc: DocumentMut = r#"
[workspace.dependencies]
runtime-world = { path = "crates/runtime/world" }
"#
        .parse()
        .unwrap();
        let v = semver::Version::new(1, 5, 2);
        assert!(set_workspace_dep_version(&mut doc, "runtime-world", &v, "crates/runtime/world", "idealyst").unwrap());
        assert!(doc.to_string().contains(
            r#"runtime-world = { path = "crates/runtime/world", version = "1.5", registry = "idealyst" }"#
        ));
        // Idempotent: a second pass changes nothing.
        assert!(!set_workspace_dep_version(&mut doc, "runtime-world", &v, "crates/runtime/world", "idealyst").unwrap());
    }

    /// Regression: `wasm-split = { path = "…", package = "wasm-splitter" }` is
    /// keyed by its ALIAS. Looking the package name up directly missed it and
    /// appended a second entry for the same path, leaving the real one without
    /// a version — so `cargo package` then refused runtime-shared for a
    /// path-only dependency.
    #[test]
    fn regression_renamed_workspace_deps_are_updated_in_place() {
        let mut doc: DocumentMut = r#"
[workspace.dependencies]
wasm-split = { path = "crates/tools/wasm-split/wasm-split", package = "wasm-splitter" }
"#
        .parse()
        .unwrap();
        let v = semver::Version::new(1, 5, 2);
        assert!(set_workspace_dep_version(
            &mut doc,
            "wasm-splitter",
            &v,
            "crates/tools/wasm-split/wasm-split",
            "idealyst"
        )
        .unwrap());
        let out = doc.to_string();
        assert!(
            out.contains(r#"package = "wasm-splitter", version = "1.5", registry = "idealyst""#),
            "{out}"
        );
        // Exactly one entry — no duplicate keyed by the package name.
        assert_eq!(out.matches("crates/tools/wasm-split/wasm-split\"").count(), 1, "{out}");
    }

    #[test]
    fn a_missing_workspace_dep_entry_is_added() {
        let mut doc: DocumentMut = "[workspace.dependencies]\n".parse().unwrap();
        let v = semver::Version::new(1, 5, 2);
        assert!(set_workspace_dep_version(&mut doc, "table", &v, "crates/sdk/client/table", "idealyst").unwrap());
        assert!(doc.to_string().contains(
            r#"table = { path = "crates/sdk/client/table", version = "1.5", registry = "idealyst" }"#
        ));
    }

    #[test]
    fn package_version_replaces_workspace_inheritance() {
        let mut doc: DocumentMut = "[package]\nname = \"x\"\nversion.workspace = true\n"
            .parse()
            .unwrap();
        set_package_version(&mut doc, &semver::Version::new(1, 5, 2)).unwrap();
        let out = doc.to_string();
        assert!(out.contains(r#"version = "1.5.2""#), "{out}");
        assert!(!out.contains("workspace = true"));
    }
}
