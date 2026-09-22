//! The dev-time overlay's BUILD-TIME half: a crate's `ui!` sites, as
//! descriptors, written next to the build.
//!
//! The compiled app carries only numbers — a site key and a node index
//! on each `Element` a `ui!` site builds. What those numbers MEAN is
//! this file. Producing it here rather than emitting it into the binary
//! is a measured decision: a `static` descriptor per site cost +1.4 s on
//! every one-edit rebuild of a real app, while the tags alone cost
//! nothing measurable (`runtime_template`'s README has the table).
//!
//! # What is produced
//!
//! One JSON document per build, under
//! `target/idealyst/<app>/overlay/<build>.json`, where `<build>` is a
//! digest of every scanned file's contents plus
//! [`runtime_template::SPLIT_VERSION`]. Keying by content means two
//! builds of the same sources share a file and a rebuild that changed
//! nothing writes nothing new; keying by the split version means a
//! change to node numbering cannot be mistaken for a change to the app.
//!
//! A differ's job is then: take the previous build's document and this
//! one, and turn the difference into patches. That differ is NOT built
//! here.
//!
//! # Why the same library as the macro
//!
//! `runtime_macros_parse` is the `ui!` parser, split pass and node
//! numbering — the exact code the proc macro runs. A second
//! implementation that disagreed by one node would mis-address every
//! patch after it, silently, because both halves would stay internally
//! consistent. `crates/dev/ui-lowering-parity` holds the two tests that
//! keep them honest: one compiles a real `ui!` and checks its tag
//! against this scanner's site key, the other checks node-for-node
//! agreement across the whole fixture corpus.
//!
//! # Failure is per-file
//!
//! A file that does not parse is reported and skipped; every other file
//! still contributes. The author is mid-edit most of the time this runs,
//! and one broken file must not cost the descriptor set for the rest of
//! the crate. Same forgiveness `catalog-scan` has, for the same reason.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use runtime_template::{Descriptor, SPLIT_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One build's descriptor set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescriptorSet {
    /// Format version of this document. Bumped when the JSON shape
    /// changes, independently of the numbering.
    pub overlay_version: u32,
    /// The node-numbering version the scanning library used. A differ
    /// must refuse a document whose value disagrees with the binary's
    /// `IDEALYST_UI_SPLIT_VERSION`.
    pub split_version: u32,
    /// `[package] name` of the scanned crate, exactly as it feeds the
    /// site key.
    pub package: String,
    /// Content digest of every scanned file, by package-relative path.
    /// What makes the build identifiable, and what a differ compares
    /// first to find which files could possibly have changed.
    pub files: BTreeMap<String, String>,
    /// Every site found, in file then source order.
    pub sites: Vec<Descriptor>,
}

impl DescriptorSet {
    /// The build key: a digest over the split version and every file's
    /// path and content hash. Also the document's file name.
    pub fn build_key(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.split_version.to_le_bytes());
        for (path, digest) in &self.files {
            h.update(path.as_bytes());
            h.update([0u8]);
            h.update(digest.as_bytes());
            h.update([0u8]);
        }
        hex(&h.finalize())
    }
}

/// Scan one crate directory into a [`DescriptorSet`].
///
/// `dir` is the directory holding `Cargo.toml` and `src/`.
pub fn scan_crate(dir: &Path) -> Result<DescriptorSet> {
    let package = package_name(dir)?;
    let src = dir.join("src");
    let mut files = Vec::new();
    super::catalog_scan::collect_rs_files(&src, &mut files);
    // Also scan `tests/` and `examples/`? No: only what the app's
    // binary is built from can carry tags, and a descriptor for a site
    // that is never compiled into the running program is noise a differ
    // would have to filter.
    files.sort();

    let mut set = DescriptorSet {
        overlay_version: 1,
        split_version: SPLIT_VERSION,
        package: package.clone(),
        files: BTreeMap::new(),
        sites: Vec::new(),
    };

    for file in &files {
        let Some(relative) = relative_to(dir, file) else { continue };
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[overlay] skipping {}: {e}", file.display());
                continue;
            }
        };
        set.files.insert(relative.clone(), hex(&Sha256::digest(text.as_bytes())));

        let sites = match runtime_macros_parse::sites_in_file(&package, &relative, &text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[overlay] skipping {} (does not parse: {e}); other files still scanned",
                    file.display()
                );
                continue;
            }
        };
        for mut site in sites {
            match runtime_macros_parse::describe(site.id.clone(), &mut site.ui) {
                Ok(d) => set.sites.push(d),
                // A stamp mismatch is a bug in this crate's own two
                // walks, not in the author's code. Loud on stderr,
                // skipped in the output: a wrong descriptor is worse
                // than a missing one.
                Err(e) => eprintln!("[overlay] {}: {e}", site.id),
            }
        }
    }

    Ok(set)
}

/// Scan `crate_dir` and write its descriptor set under
/// `target/idealyst/<app>/overlay/`, returning the file written.
///
/// `<app>` is the scanned crate's own package name, so a project with
/// several app crates gets one directory each.
///
/// Idempotent: the name is the build key, so re-running on unchanged
/// sources rewrites the same bytes to the same path.
pub fn write_for(project_root: &Path, crate_dir: &Path) -> Result<PathBuf> {
    let set = scan_crate(crate_dir)?;
    let dir = overlay_dir(project_root, &set.package);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{}.json", set.build_key()));
    let json = serde_json::to_string(&set)?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// `target/idealyst/<app>/overlay` — alongside the other per-app CLI
/// staging dirs, so `idealyst clean` already sweeps it.
pub fn overlay_dir(project_root: &Path, app: &str) -> PathBuf {
    project_root.join("target").join("idealyst").join(app).join("overlay")
}

/// `[package] name` of the crate, spelled as cargo spells it — hyphens
/// intact, because that is what `CARGO_PKG_NAME` gives the proc macro
/// and therefore what feeds the site key.
fn package_name(dir: &Path) -> Result<String> {
    let manifest_path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
        .with_context(|| format!("{} has no [package] name", manifest_path.display()))
}

/// A file's path relative to the package root, `/`-separated — the
/// spelling the proc macro normalizes its span's file to.
fn relative_to(dir: &Path, file: &Path) -> Option<String> {
    Some(file.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway crate on disk and scan it.
    ///
    /// A real directory rather than a string: the scan's job includes
    /// walking `src/`, reading `Cargo.toml`, and turning absolute paths
    /// back into package-relative ones, and none of that is exercised by
    /// handing it text.
    fn fixture_crate() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"overlay-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/screens")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            r#"
mod screens;

#[component]
fn Root() -> Element {
    ui! {
        view(style = sheet()) {
            text { "Hello" }
        }
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/screens/login.rs"),
            r#"
#[component]
fn Login(count: i32) -> Element {
    ui! {
        view() {
            text { "Sign in" }
            if count > 0 {
                text { "again" }
            }
        }
    }
}
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_scan_finds_every_site_and_keys_it_by_where_it_is_written() {
        let dir = fixture_crate();
        let set = scan_crate(dir.path()).expect("scan");

        assert_eq!(set.package, "overlay-fixture");
        assert_eq!(set.split_version, SPLIT_VERSION);
        assert_eq!(set.sites.len(), 2, "one `ui!` per file");

        let mut files: Vec<&str> = set.sites.iter().map(|s| s.site.file.as_ref()).collect();
        files.sort();
        assert_eq!(files, vec!["src/lib.rs", "src/screens/login.rs"]);
        // Package-relative and `/`-separated, never the tempdir's path:
        // the key must not depend on where the crate was checked out.
        assert!(set.files.keys().all(|f| !f.starts_with('/')), "{:?}", set.files);
    }

    /// The `if` occupies a node index between its siblings, and its body
    /// hangs under it. That is the property a differ addresses children
    /// by, so it is pinned on real scanned output rather than assumed.
    #[test]
    fn control_flow_is_a_node_with_its_body_underneath() {
        let dir = fixture_crate();
        let set = scan_crate(dir.path()).expect("scan");
        let login = set
            .sites
            .iter()
            .find(|s| s.site.file == "src/screens/login.rs")
            .expect("login site");

        let opaque = login
            .nodes
            .iter()
            .enumerate()
            .find_map(|(i, n)| match n {
                // The `if` is the Opaque node that HAS children; a bare
                // literal child of `text` is also Opaque (it is an
                // expression the descriptor cannot patch in place — the
                // patchable form is the parent's `content` prop) but it
                // is a leaf.
                runtime_template::Node::Opaque { expr: Some(e), children }
                    if !children.is_empty() =>
                {
                    Some((i as u32, e.to_string(), children.to_vec()))
                }
                _ => None,
            })
            .expect("the `if` is an Opaque node");
        assert_eq!(opaque.1, "count>0", "the condition is recorded as source text");
        assert_eq!(opaque.2.len(), 1, "its body is one node, hanging under it");
        assert!(opaque.2[0] > opaque.0, "a child is numbered after its parent");
        assert_eq!(runtime_template::check_well_formed(login), Ok(()));
    }

    /// Two scans of the same sources must produce the same build key,
    /// and any edit must change it. That is the whole contract of the
    /// file name.
    #[test]
    fn the_build_key_is_the_content_and_the_split_version() {
        let dir = fixture_crate();
        let first = scan_crate(dir.path()).expect("scan").build_key();
        assert_eq!(first, scan_crate(dir.path()).expect("scan").build_key());

        let login = dir.path().join("src/screens/login.rs");
        let text = std::fs::read_to_string(&login).unwrap().replace("Sign in", "Sign in.");
        std::fs::write(&login, text).unwrap();
        assert_ne!(first, scan_crate(dir.path()).expect("scan").build_key());
    }

    /// One unparseable file costs its own descriptors and nothing else.
    #[test]
    fn a_file_that_does_not_parse_does_not_take_the_crate_down() {
        let dir = fixture_crate();
        std::fs::write(dir.path().join("src/broken.rs"), "fn oops( {").unwrap();
        let set = scan_crate(dir.path()).expect("scan");
        assert_eq!(set.sites.len(), 2, "the other two files still contributed");
        assert!(set.files.contains_key("src/broken.rs"), "and it is still hashed");
    }

    #[test]
    fn writing_puts_the_set_under_the_app_staging_dir() {
        let dir = fixture_crate();
        let project = tempfile::tempdir().expect("tempdir");
        let path = write_for(project.path(), dir.path()).expect("write");
        assert!(
            path.starts_with(project.path().join("target/idealyst/overlay-fixture/overlay")),
            "{path:?}"
        );
        let read: DescriptorSet =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, scan_crate(dir.path()).unwrap());
        assert_eq!(path.file_stem().unwrap().to_string_lossy(), read.build_key());
    }
}
