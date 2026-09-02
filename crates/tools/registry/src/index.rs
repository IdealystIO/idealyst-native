//! Building cargo's sparse-index entries.
//!
//! A sparse registry is exactly three kinds of static file:
//!
//!   config.json                          — where the tarballs live
//!   <index-path>                          — JSON-lines, one line per version
//!   <dl>/<name>/<vers>/download           — the `cargo package` tarball
//!
//! Nothing here is bespoke: the line schema and the index path layout are
//! cargo's, and are shared with the older git-backed index format.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const CRATES_IO: &str = "https://github.com/rust-lang/crates.io-index";

#[derive(Debug, Serialize)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    pub deps: Vec<IndexDep>,
    pub cksum: String,
    pub features: BTreeMap<String, Vec<String>>,
    pub yanked: bool,
    pub links: Option<String>,
    /// Schema version. 2 tells cargo that `features2` may be present and that
    /// `dep:`/`?/` feature syntax is in play.
    pub v: u32,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub features2: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct IndexDep {
    pub name: String,
    pub req: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub default_features: bool,
    pub target: Option<String>,
    pub kind: String,
    /// `None` means "the registry this index belongs to". Dependencies that
    /// come from crates.io MUST name it explicitly, or cargo will look for
    /// them here and fail to resolve.
    pub registry: Option<String>,
    /// Set only when the dependency was renamed; carries the original package
    /// name while `name` carries the rename.
    pub package: Option<String>,
}

/// Cargo's index path layout, which the sparse protocol reuses verbatim.
///
///   1 char  -> `1/{name}`
///   2 chars -> `2/{name}`
///   3 chars -> `3/{first}/{name}`
///   4+      -> `{c0}{c1}/{c2}{c3}/{name}`
///
/// Names are lowercased; cargo lowercases when it looks them up, so an
/// uppercase path would simply 404.
pub fn index_path(name: &str) -> String {
    let n = name.to_lowercase();
    match n.chars().count() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{}", &n[..1], n),
        _ => format!("{}/{}/{}", &n[..2], &n[2..4], n),
    }
}

pub fn checksum(crate_file: &Path) -> Result<String> {
    let bytes = std::fs::read(crate_file)
        .with_context(|| format!("reading {}", crate_file.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

/// Read the normalized `Cargo.toml` that cargo itself wrote into the `.crate`
/// tarball.
///
/// Deriving the index entry from the packaged manifest rather than the source
/// one avoids having to re-implement cargo's normalization — dev-deps without
/// versions dropped, workspace inheritance resolved, `default-features`
/// spelled out. Whatever cargo put in the tarball is by definition what the
/// published crate declares.
pub fn packaged_manifest(crate_file: &Path) -> Result<toml_edit::DocumentMut> {
    let f = std::fs::File::open(crate_file)
        .with_context(|| format!("opening {}", crate_file.display()))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for entry in ar.entries().context("reading .crate tarball")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        // `<name>-<version>/Cargo.toml`, and *not* Cargo.toml.orig, which is
        // the unnormalized copy cargo keeps for provenance.
        if path.file_name().is_some_and(|f| f == "Cargo.toml")
            && path.components().count() == 2
        {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut entry, &mut s)?;
            return s.parse().context("parsing packaged Cargo.toml");
        }
    }
    anyhow::bail!("{} contains no Cargo.toml", crate_file.display())
}

/// Turn a packaged manifest into the registry's dependency list.
pub fn deps_from_manifest(doc: &toml_edit::DocumentMut, internal: &dyn Fn(&str) -> bool) -> Vec<IndexDep> {
    let mut out = Vec::new();
    let root = doc.as_table();
    for (section, kind) in [
        ("dependencies", "normal"),
        ("dev-dependencies", "dev"),
        ("build-dependencies", "build"),
    ] {
        gather(root, section, kind, None, internal, &mut out);
    }
    if let Some(targets) = root.get("target").and_then(|i| i.as_table_like()) {
        for (cfg, item) in targets.iter() {
            let Some(t) = item.as_table_like() else { continue };
            for (section, kind) in [
                ("dependencies", "normal"),
                ("dev-dependencies", "dev"),
                ("build-dependencies", "build"),
            ] {
                gather_like(t, section, kind, Some(cfg.to_string()), internal, &mut out);
            }
        }
    }
    out
}

fn gather(
    table: &toml_edit::Table,
    section: &str,
    kind: &str,
    target: Option<String>,
    internal: &dyn Fn(&str) -> bool,
    out: &mut Vec<IndexDep>,
) {
    if let Some(t) = table.get(section).and_then(|i| i.as_table_like()) {
        push_all(t, kind, target, internal, out);
    }
}

fn gather_like(
    table: &dyn toml_edit::TableLike,
    section: &str,
    kind: &str,
    target: Option<String>,
    internal: &dyn Fn(&str) -> bool,
    out: &mut Vec<IndexDep>,
) {
    if let Some(t) = table.get(section).and_then(|i| i.as_table_like()) {
        push_all(t, kind, target, internal, out);
    }
}

fn push_all(
    deps: &dyn toml_edit::TableLike,
    kind: &str,
    target: Option<String>,
    internal: &dyn Fn(&str) -> bool,
    out: &mut Vec<IndexDep>,
) {
    for (key, item) in deps.iter() {
        let (req, features, optional, default_features, package) = match item {
            // `foo = "1.2"`
            i if i.as_str().is_some() => (
                i.as_str().unwrap().to_string(),
                vec![],
                false,
                true,
                None,
            ),
            i => {
                let Some(t) = i.as_table_like() else { continue };
                let req = t
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*")
                    .to_string();
                let features = t
                    .get("features")
                    .and_then(|f| f.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let optional = t.get("optional").and_then(|v| v.as_bool()).unwrap_or(false);
                let default_features = t
                    .get("default-features")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let package = t
                    .get("package")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                (req, features, optional, default_features, package)
            }
        };
        // The real package this entry resolves to, for deciding which
        // registry it lives in.
        let resolved = package.as_deref().unwrap_or(key);
        out.push(IndexDep {
            name: key.to_string(),
            req,
            features,
            optional,
            default_features,
            target: target.clone(),
            kind: kind.to_string(),
            registry: if internal(resolved) {
                None
            } else {
                Some(CRATES_IO.to_string())
            },
            package,
        });
    }
}

/// Split `[features]` the way the v2 index expects: anything using the newer
/// `dep:` / `pkg?/feat` syntax goes in `features2` so that a cargo too old to
/// understand it still parses the entry.
pub fn split_features(
    doc: &toml_edit::DocumentMut,
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let mut v1 = BTreeMap::new();
    let mut v2 = BTreeMap::new();
    let Some(t) = doc.as_table().get("features").and_then(|i| i.as_table_like()) else {
        return (v1, v2);
    };
    for (name, item) in t.iter() {
        let vals: Vec<String> = item
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if vals.iter().any(|v| v.starts_with("dep:") || v.contains("?/")) {
            v2.insert(name.to_string(), vals);
        } else {
            v1.insert(name.to_string(), vals);
        }
    }
    (v1, v2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_paths_follow_cargos_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("wire"), "wi/re/wire");
        assert_eq!(index_path("runtime-world"), "ru/nt/runtime-world");
        assert_eq!(index_path("idea-ui"), "id/ea/idea-ui");
    }

    /// Cargo lowercases a crate name before looking up its index path, so an
    /// entry written under the original casing would simply 404.
    #[test]
    fn index_paths_are_lowercased() {
        assert_eq!(index_path("Idea-UI"), "id/ea/idea-ui");
    }

    #[test]
    fn features_split_on_the_v2_syntax() {
        let doc: toml_edit::DocumentMut = r#"
[features]
plain = ["other"]
gated = ["dep:serde"]
weak = ["serde?/derive"]
"#
        .parse()
        .unwrap();
        let (v1, v2) = split_features(&doc);
        assert_eq!(v1.keys().collect::<Vec<_>>(), vec!["plain"]);
        assert_eq!(v2.keys().collect::<Vec<_>>(), vec!["gated", "weak"]);
    }

    /// A dependency on one of our own crates carries `registry: null`, meaning
    /// "this index"; anything else must name crates.io explicitly or cargo
    /// looks for it here and fails to resolve.
    #[test]
    fn dependency_registries_are_attributed() {
        let doc: toml_edit::DocumentMut = r#"
[dependencies]
runtime-shared = { version = "1.5" }
serde = { version = "1", features = ["derive"] }

[target.'cfg(target_arch = "wasm32")'.dependencies]
backend-web = { version = "1.5" }
"#
        .parse()
        .unwrap();
        let internal = |n: &str| matches!(n, "runtime-shared" | "backend-web");
        let deps = deps_from_manifest(&doc, &internal);
        let by = |n: &str| deps.iter().find(|d| d.name == n).unwrap();
        assert_eq!(by("runtime-shared").registry, None);
        assert_eq!(by("serde").registry.as_deref(), Some(CRATES_IO));
        assert_eq!(
            by("backend-web").target.as_deref(),
            Some("cfg(target_arch = \"wasm32\")")
        );
        assert_eq!(by("backend-web").registry, None);
    }

    #[test]
    fn renamed_dependencies_keep_both_names() {
        let doc: toml_edit::DocumentMut = r#"
[dependencies]
runtime_core = { version = "1.5", package = "runtime-core" }
"#
        .parse()
        .unwrap();
        let deps = deps_from_manifest(&doc, &|n| n == "runtime-core");
        assert_eq!(deps[0].name, "runtime_core");
        assert_eq!(deps[0].package.as_deref(), Some("runtime-core"));
        // Attribution follows the real package, not the local alias.
        assert_eq!(deps[0].registry, None);
    }
}
