//! The SDK table is hand-curated, so it drifts whenever a crate lands
//! without an entry: `qr`, `gpu-surface` and `idea-theme-editor` all
//! shipped invisible to `list_sdks` / `describe_sdk` / `search` until the
//! mcp-catalog-drift audit caught them. This walks the crate directories
//! the audit names and fails on the first crate with no `sdk!` entry, and
//! on any entry whose crate no longer exists.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Crates under the roster roots that are not author-facing deps:
/// proc-macro helpers re-exported through their parent crate, and the
/// per-platform internals `stack-navigator` pulls in itself.
const NOT_AUTHOR_FACING: &[&str] = &[
    "offload-macro",
    "server-macros",
    "idea-ui-docs-derive",
    "android-navigator-helpers",
    "ios-navigator-helpers",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Every package directly under `root`, or one level deeper when a
/// directory only groups crates (`crates/sdk/client/navigators/{stack,swap}`).
/// Nested crates inside a package (`canvas/core`, `canvas3d/wgpu`, examples)
/// are that package's implementation, so they are not collected.
fn packages_under(root: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if let Some(name) = package_name(&manifest) {
                out.insert(name);
            }
        } else {
            packages_under(&dir, out);
        }
    }
}

fn roster() -> BTreeSet<String> {
    let root = repo_root();
    let mut names = BTreeSet::new();
    for sub in ["crates/sdk/client", "crates/sdk/server", "crates/api", "crates/ui"] {
        packages_under(&root.join(sub), &mut names);
    }
    assert!(names.contains("net") && names.contains("idea-ui"), "roster walk found the crate tree: {names:?}");
    for helper in NOT_AUTHOR_FACING {
        names.remove(*helper);
    }
    names
}

#[test]
fn every_author_facing_crate_has_an_sdk_entry() {
    let entries: BTreeSet<&str> = mcp_catalog::sdks().map(|e| e.name).collect();
    let missing: Vec<_> = roster().into_iter().filter(|n| !entries.contains(n.as_str())).collect();
    assert!(
        missing.is_empty(),
        "crates with no `sdk!` entry in crates/mcp/catalog/src/sdks.rs (add one, or list a non-author-facing helper in NOT_AUTHOR_FACING): {missing:?}"
    );
}

#[test]
fn every_sdk_entry_names_a_crate_that_exists() {
    let roster = roster();
    let stale: Vec<_> = mcp_catalog::sdks().map(|e| e.name).filter(|n| !roster.contains(*n)).collect();
    assert!(stale.is_empty(), "`sdk!` entries with no crate under crates/{{sdk,api,ui}}: {stale:?}");
}
