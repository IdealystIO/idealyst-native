//! `idealyst clean` — reclaim the build output the framework's own
//! pipelines produce.
//!
//! ## Why this exists as a command at all
//!
//! `cargo clean` only knows about the workspace's own `target/`. The
//! framework's build pipelines write to two places cargo has no idea
//! about, and nothing else ever reclaims them:
//!
//! - `target/idealyst-web` — the wasm build's private target dir (see
//!   `build_web::build`). It is keyed on the RUSTFLAGS + profile the web
//!   pipeline adds, so it is a full second copy of the dependency graph.
//! - `target/idealyst/<app>/…` — per-app ephemeral platform projects
//!   (the iOS wrapper, the premint dump crate), each with its own nested
//!   `target/` that shares nothing with anything.
//!
//! ## Why it grows without bound
//!
//! Cargo never garbage-collects superseded compilation units. A unit's
//! artifacts are named `<crate>-<metadata-hash>.rlib`, and the metadata
//! hash folds in the resolved feature set of every dependency. Two apps
//! that pull different `web-sys` features therefore produce two disjoint
//! copies of *everything* downstream of `web-sys`, and both stay on disk
//! forever. Same for toggling `--premint`, `--local`, or editing
//! `Cargo.toml` in a way that moves the resolve.
//!
//! Measured on this repo: a trivial hello-world app's first
//! `dev --web --local` build costs ~286 MB; switching to a second app
//! costs ~1 GB. Repeated rebuilds of one app with unchanged flags do NOT
//! grow — rustc caps its incremental cache at two sessions per unit — so
//! the growth axis is (apps × flag combinations), not (rebuilds).
//!
//! `--stale` targets exactly that: it keeps the newest unit per crate
//! and drops the superseded copies, so the next build is still warm.
//! Deleting a unit that some *other* configuration still wanted is not a
//! correctness problem — cargo just recompiles it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Artifact extensions cargo writes into `deps/` alongside a unit, keyed
/// by the `<crate>-<hash>` stem. Anything not matching a known extension
/// is left alone rather than guessed at.
const DEP_EXTENSIONS: &[&str] = &["rlib", "rmeta", "wasm", "d", "a", "so", "dylib", "o"];

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Also drop the ephemeral platform projects under
    /// `target/idealyst/`. Without this flag, `clean` only removes
    /// Cargo build output and leaves the generated Xcode/Gradle
    /// projects in place so the next `dev` doesn't pay the
    /// regeneration cost.
    #[arg(long)]
    pub deep: bool,

    /// Prune only *superseded* compilation units, keeping the newest
    /// build of each crate. Reclaims the duplicate copies that pile up
    /// as you switch apps or toggle `--premint`/`--local`, without
    /// forcing a cold rebuild of the app you're currently working on.
    /// Mutually exclusive with the wholesale removal above.
    #[arg(long, conflicts_with = "deep")]
    pub stale: bool,

    /// Report what would be removed without deleting anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Project directory. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let dir = crate::framework_source::abs_project_dir(&args.dir)?;
    let source = crate::framework_source::resolve(&dir)?;
    let web_target = source.cargo_target_dir(&dir).join("idealyst-web");
    let platform_root = source.wrapper_root(&dir);

    let mut reclaimed = 0u64;

    if args.stale {
        reclaimed += prune_stale(&web_target, args.dry_run)?;
    } else {
        reclaimed += remove_path(&web_target, args.dry_run)?;
        // The per-app nested cargo target dirs are build output; the
        // generated Xcode/Gradle project around them is not. Without
        // `--deep` we take the former and leave the latter, so the next
        // `dev` skips project regeneration but still gets a clean build.
        if args.deep {
            reclaimed += remove_path(&platform_root, args.dry_run)?;
        } else {
            for nested in nested_target_dirs(&platform_root) {
                reclaimed += remove_path(&nested, args.dry_run)?;
            }
        }
    }

    let verb = if args.dry_run { "would reclaim" } else { "reclaimed" };
    eprintln!("[idealyst clean] {verb} {}", human_bytes(reclaimed));
    if !args.deep && !args.stale {
        eprintln!(
            "[idealyst clean] generated platform projects kept — use `--deep` to drop them too"
        );
    }
    eprintln!(
        "[idealyst clean] the workspace's own `target/` is cargo's — use `cargo clean` for that"
    );
    Ok(())
}

/// Nested cargo target dirs under the per-app platform-project root
/// (`target/idealyst/<app>/<platform>/…/target`). Found by walking for
/// directories literally named `target`, which is what every generated
/// project's `.cargo/config.toml` redirects to.
fn nested_target_dirs(platform_root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_nested_targets(platform_root, &mut found);
    found
}

fn collect_nested_targets(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "target") {
            // Don't descend — everything under a target dir goes.
            out.push(path);
        } else {
            collect_nested_targets(&path, out);
        }
    }
}

/// Drop every compilation unit except the most recently built one per
/// `<crate>` name, across each `<triple>/<profile>` layout in the web
/// target dir.
///
/// Cargo names a unit's fingerprint dir and its `deps/` artifacts with
/// the same `<crate>-<metadata-hash>` stem, so the fingerprint dir's
/// mtime is a reliable "when was this unit last built" and the stem
/// links it to the artifacts to remove.
fn prune_stale(web_target: &Path, dry_run: bool) -> Result<u64> {
    if !web_target.exists() {
        return Ok(0);
    }
    let mut reclaimed = 0u64;
    for layout in build_layouts(web_target) {
        reclaimed += prune_layout(&layout, dry_run)?;
    }
    Ok(reclaimed)
}

/// Every directory under the target dir that has a `.fingerprint`
/// sibling — i.e. `<target>/debug` and `<target>/<triple>/debug`.
fn build_layouts(web_target: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(web_target) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(".fingerprint").is_dir() {
            out.push(path);
            continue;
        }
        // One level deeper for the `<triple>/<profile>` shape.
        if let Ok(inner) = fs::read_dir(&path) {
            for e in inner.flatten() {
                let p = e.path();
                if p.is_dir() && p.join(".fingerprint").is_dir() {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn prune_layout(layout: &Path, dry_run: bool) -> Result<u64> {
    let fingerprint = layout.join(".fingerprint");
    let deps = layout.join("deps");

    // crate name -> (mtime, unit dir name, hash)
    let mut units: HashMap<String, Vec<(std::time::SystemTime, String, String)>> = HashMap::new();
    let Ok(entries) = fs::read_dir(&fingerprint) else {
        return Ok(0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((crate_name, hash)) = split_unit_name(name) else {
            continue;
        };
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        units
            .entry(crate_name.to_string())
            .or_default()
            .push((mtime, name.to_string(), hash.to_string()));
    }

    let mut reclaimed = 0u64;
    for (_crate_name, mut versions) in units {
        if versions.len() < 2 {
            continue;
        }
        // Newest last; everything before it is superseded.
        versions.sort_by_key(|(mtime, _, _)| *mtime);
        versions.pop();
        for (_, unit_dir, hash) in versions {
            reclaimed += remove_path(&fingerprint.join(&unit_dir), dry_run)?;
            // `<crate>-<hash>.<ext>` in deps/. The crate name in deps/
            // is the *underscored* form, and split_unit_name already
            // gave us the fingerprint spelling, so match on the hash —
            // it's a 16-hex-digit metadata hash, unique per unit.
            reclaimed += remove_dep_artifacts(&deps, &hash, dry_run)?;
        }
    }
    Ok(reclaimed)
}

/// Split a cargo fingerprint dir name (`<crate>-<16 hex>`) into its
/// crate name and metadata hash. Returns `None` for anything that
/// doesn't match, so unexpected entries are left untouched.
fn split_unit_name(name: &str) -> Option<(&str, &str)> {
    let (crate_name, hash) = name.rsplit_once('-')?;
    let is_hash = hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    is_hash.then_some((crate_name, hash))
}

/// Remove every `deps/` artifact carrying `hash` as its metadata hash.
fn remove_dep_artifacts(deps: &Path, hash: &str, dry_run: bool) -> Result<u64> {
    let Ok(entries) = fs::read_dir(deps) else {
        return Ok(0);
    };
    let mut reclaimed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dep_artifact_matches(name, hash) {
            reclaimed += remove_path(&path, dry_run)?;
        }
    }
    Ok(reclaimed)
}

/// Whether a `deps/` filename belongs to the unit identified by `hash`.
///
/// Cargo writes both `<crate>-<hash>.<ext>` and, for split codegen
/// units, `<crate>-<hash>.<cgu>.<ext>`. Matching on the `-<hash>`
/// segment covers both without needing the crate name, which differs in
/// spelling (hyphens vs underscores) between `.fingerprint` and `deps`.
fn dep_artifact_matches(file_name: &str, hash: &str) -> bool {
    let Some((stem, rest)) = file_name.split_once(&format!("-{hash}")) else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }
    // `rest` is the remainder after the hash: either empty, or a
    // `.`-prefixed extension chain. Guard against a longer hash-like
    // run (`-<hash><more hex>`) matching a different unit.
    rest.is_empty()
        || rest.strip_prefix('.').is_some_and(|ext| {
            ext.rsplit('.')
                .next()
                .is_some_and(|last| DEP_EXTENSIONS.contains(&last))
        })
}

/// Delete a file or directory, returning the bytes reclaimed. A missing
/// path is not an error — `clean` is idempotent by design.
fn remove_path(path: &Path, dry_run: bool) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let size = dir_size(path);
    if !dry_run {
        if path.is_dir() {
            fs::remove_dir_all(path)
                .with_context(|| format!("remove {}", path.display()))?;
        } else {
            fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    Ok(size)
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        // Symlink — count the link, never follow it out of the tree.
        return 0;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|e| dir_size(&e.path())).sum()
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "MB", "GB", "TB"];
    // Skip KB — this command deals in build trees, not config files.
    let mut value = bytes as f64;
    let mut unit = 0;
    if value >= 1024.0 {
        value /= 1024.0 * 1024.0;
        unit = 1;
    }
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; bytes]).unwrap();
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-clean-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fingerprint dir name is `<crate>-<16 hex>`; anything else is
    /// left alone rather than guessed at.
    #[test]
    fn split_unit_name_requires_a_16_hex_suffix() {
        assert_eq!(
            split_unit_name("web-sys-0dd769c77fe83c22"),
            Some(("web-sys", "0dd769c77fe83c22"))
        );
        assert_eq!(split_unit_name("build-script-build"), None);
        assert_eq!(split_unit_name("no-hash"), None);
        // Right length, not hex.
        assert_eq!(split_unit_name("crate-zzzzzzzzzzzzzzzz"), None);
    }

    /// The `deps/` matcher has to cover both the plain artifact and the
    /// per-codegen-unit `.rcgu.o` spellings, and must not fire on a
    /// different unit whose hash merely starts with the same run.
    #[test]
    fn dep_artifact_matches_plain_and_codegen_unit_spellings() {
        let hash = "b0e192518a57f733";
        assert!(dep_artifact_matches(
            &format!("libruntime_layout-{hash}.rlib"),
            hash
        ));
        assert!(dep_artifact_matches(
            &format!("runtime_layout-{hash}.a94da2mmxi7cz5iitqk1iskby.1mskdzd.rcgu.o"),
            hash
        ));
        assert!(!dep_artifact_matches("libruntime_layout-cafe.rlib", hash));
        // A longer hex run is a different unit, not this one.
        assert!(!dep_artifact_matches(
            &format!("libruntime_layout-{hash}ab.rlib"),
            hash
        ));
    }

    /// The regression this command exists for: superseded units pile up
    /// forever because cargo never GCs them. `--stale` must drop the
    /// older unit's fingerprint dir AND its `deps/` artifacts, while
    /// leaving the newest build completely intact.
    #[test]
    fn regression_stale_prune_keeps_newest_unit_and_drops_superseded() {
        let root = tmpdir("stale");
        let layout = root.join("wasm32-unknown-unknown/debug");
        let old_hash = "1111111111111111";
        let new_hash = "2222222222222222";

        for hash in [old_hash, new_hash] {
            touch(&layout.join(format!(".fingerprint/web-sys-{hash}/lib-web_sys")), 16);
            touch(&layout.join(format!("deps/libweb_sys-{hash}.rlib")), 4096);
            touch(&layout.join(format!("deps/libweb_sys-{hash}.rmeta")), 2048);
        }
        // Make the "old" unit unambiguously older than the new one.
        let old_dir = layout.join(format!(".fingerprint/web-sys-{old_hash}"));
        let long_ago =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        filetime_set(&old_dir, long_ago);

        let reclaimed = prune_stale(&root, false).unwrap();

        assert!(
            !old_dir.exists(),
            "superseded fingerprint dir should be gone"
        );
        assert!(
            !layout.join(format!("deps/libweb_sys-{old_hash}.rlib")).exists(),
            "superseded rlib should be gone"
        );
        assert!(
            !layout.join(format!("deps/libweb_sys-{old_hash}.rmeta")).exists(),
            "superseded rmeta should be gone"
        );
        assert!(
            layout.join(format!(".fingerprint/web-sys-{new_hash}")).exists(),
            "newest unit must survive so the next build stays warm"
        );
        assert!(
            layout.join(format!("deps/libweb_sys-{new_hash}.rlib")).exists(),
            "newest rlib must survive"
        );
        assert_eq!(reclaimed, 16 + 4096 + 2048);

        let _ = fs::remove_dir_all(&root);
    }

    /// A single-unit crate has nothing superseded — `--stale` must be a
    /// no-op rather than deleting the only copy.
    #[test]
    fn stale_prune_leaves_a_sole_unit_alone() {
        let root = tmpdir("sole");
        let layout = root.join("wasm32-unknown-unknown/debug");
        touch(&layout.join(".fingerprint/web-sys-1111111111111111/lib-web_sys"), 16);
        touch(&layout.join("deps/libweb_sys-1111111111111111.rlib"), 4096);

        assert_eq!(prune_stale(&root, false).unwrap(), 0);
        assert!(layout
            .join(".fingerprint/web-sys-1111111111111111")
            .exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// `--dry-run` reports the same byte count it would reclaim, but
    /// must not touch the tree.
    #[test]
    fn dry_run_reports_without_deleting() {
        let root = tmpdir("dry");
        let layout = root.join("wasm32-unknown-unknown/debug");
        for hash in ["1111111111111111", "2222222222222222"] {
            touch(&layout.join(format!(".fingerprint/web-sys-{hash}/lib-web_sys")), 16);
            touch(&layout.join(format!("deps/libweb_sys-{hash}.rlib")), 4096);
        }
        filetime_set(
            &layout.join(".fingerprint/web-sys-1111111111111111"),
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
        );

        let reported = prune_stale(&root, true).unwrap();
        assert_eq!(reported, 16 + 4096);
        assert!(layout
            .join(".fingerprint/web-sys-1111111111111111")
            .exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// Without `--deep`, the nested cargo target dirs under a generated
    /// platform project go, but the project scaffolding around them
    /// stays so the next `dev` skips regeneration.
    #[test]
    fn nested_target_dirs_finds_per_app_build_output_only() {
        let root = tmpdir("nested");
        let app = root.join("welcome/ios/wrapper");
        touch(&app.join("target/debug/libfoo.rlib"), 8);
        touch(&app.join("Cargo.toml"), 8);
        touch(&root.join("welcome/web/premint-dump/target/debug/x.rlib"), 8);

        let mut found = nested_target_dirs(&root);
        found.sort();
        assert_eq!(
            found,
            vec![
                root.join("welcome/ios/wrapper/target"),
                root.join("welcome/web/premint-dump/target"),
            ]
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Set an mtime without pulling in a dependency — the test needs
    /// deterministic ordering, and `filetime` isn't in the CLI's tree.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let file = fs::File::open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(when)).unwrap();
    }
}
