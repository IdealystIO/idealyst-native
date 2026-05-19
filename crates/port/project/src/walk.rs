//! Source tree walker.
//!
//! Returns every TSX/JSX/Vue/Svelte file under a root, skipping
//! the obvious noise directories (`node_modules`, `dist`, `.git`,
//! ...) and respecting symlinks via the default `read_dir`
//! behavior (no following).
//!
//! Kept dependency-free — `walkdir` is a small dep but adding it
//! for the handful of lines below isn't worth the build cost.

use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    ".git",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "coverage",
    ".turbo",
    ".cache",
];

const SOURCE_EXTS: &[&str] = &["tsx", "jsx", "vue", "svelte"];

/// Plain `.ts` / `.d.ts` files are *not* ported as components,
/// but the project driver scans them for type definitions to
/// feed its cross-file registry. Includes `.mts`/`.cts` flavors
/// for completeness.
const TYPE_EXTS: &[&str] = &["ts", "mts", "cts"];

pub fn find_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out, |ext| SOURCE_EXTS.contains(&ext));
    out.sort();
    out
}

/// Find `.ts` / `.d.ts` / `.mts` / `.cts` files. These are
/// scanned for type aliases + interfaces only — they don't get
/// ported, but their types feed the cross-file context registry.
/// Excludes files already in [`find_sources`] (i.e. `.tsx` are
/// not double-counted).
pub fn find_type_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out, |ext| TYPE_EXTS.contains(&ext));
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, accept: impl Fn(&str) -> bool + Copy) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if SKIP_DIRS.contains(&name) || name.starts_with('.') {
                continue;
            }
            walk(&path, out, accept);
        } else if file_type.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if accept(ext.to_lowercase().as_str()) {
                    out.push(path);
                }
            }
        }
    }
}
