//! `idealyst clean` — reclaim the build output the framework's own
//! pipelines produce.
//!
//! ## Why this exists as a command at all
//!
//! `cargo clean` only knows about the workspace's own `target/`. The
//! framework's build pipelines write to places cargo has no idea about:
//!
//! - `idealyst-web-<key>/` — the wasm build's target dirs, one per build
//!   configuration (see `build_web::web_target_dir`), under the framework
//!   target root. Each is a full copy of the dependency graph.
//! - `idealyst-dev-server/` and `idealyst-mcp/` — the dev server's and the
//!   MCP catalog's isolated builds, under the cargo workspace's target
//!   (`framework_source::cli_target_root`). An older CLI put the dev
//!   server under each APP crate's `target/` instead; those are removed too.
//! - `target/idealyst/<app>/…` — per-app ephemeral platform projects
//!   (the iOS wrapper, the premint dump crate), each with its own nested
//!   `target/` that shares nothing with anything.
//!
//! ## Why it grows without bound
//!
//! Cargo never garbage-collects superseded compilation units, and rustc
//! never removes another hash's incremental cache. A unit's artifacts are
//! named `<crate>-<metadata-hash>`, and the hash folds in the resolved
//! feature set of every dependency, so every framework release, `cargo
//! update` or flag toggle leaves the previous generation behind.
//!
//! Builds now trim this themselves: every CLI-run build records the units
//! it consists of and prunes its target dir to what the recorded builds
//! use (`build_ios::target_gc`), and the web build retires a superseded
//! `idealyst-web-*` dir (`build_web::record_web_key`). `--stale` is the
//! sweep for what they can't reach: web dirs no build slot records
//! (including every dir a CLI from before the records built), and target
//! dirs nothing has built in since an upgrade. Where a profile dir has
//! records, `--stale` removes exactly what the next build's own prune
//! would; where it has none (an older CLI built it), it falls back to
//! keeping the newest of everything, so the next build is still warm.
//! Deleting something another configuration still wanted is not a
//! correctness problem: cargo just recompiles it.
//!
//! Nothing is touched while a build holds its cargo locks; such a dir is
//! reported as skipped.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use build_ios::target_gc::{self, GcReport};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Also drop the ephemeral platform projects under
    /// `target/idealyst/`. Without this flag, `clean` only removes
    /// Cargo build output and leaves the generated Xcode/Gradle
    /// projects in place so the next `dev` doesn't pay the
    /// regeneration cost.
    #[arg(long)]
    pub deep: bool,

    /// Remove only *superseded* build output, keeping what current builds
    /// use: `idealyst-web-*` dirs no app's build records any more, dirs an
    /// older CLI used, superseded compilation units in the live web dirs,
    /// and stale incremental caches everywhere. Reclaims the copies that
    /// pile up as you switch apps, toggle flags or take framework releases,
    /// without forcing a cold rebuild of what you're working on. Dirs a
    /// running build holds are skipped. Mutually exclusive with the
    /// wholesale removal above.
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
    let dirs = CliDirs::resolve(&source, &dir);
    let platform_root = source.wrapper_root(&dir);

    let mut reclaimed = 0u64;
    let mut skipped = Vec::new();

    if args.stale {
        reclaimed += sweep_stale(&dirs, &platform_root, args.dry_run, &mut skipped)?;
    } else {
        for target in dirs.all_build_dirs() {
            reclaimed += remove_build_dir(&target, args.dry_run, &mut skipped)?;
        }
        // The per-app nested cargo target dirs are build output; the
        // generated Xcode/Gradle project around them is not. Without
        // `--deep` we take the former and leave the latter, so the next
        // `dev` skips project regeneration but still gets a clean build.
        if args.deep {
            reclaimed += remove_path(&platform_root, args.dry_run)?;
        } else {
            for nested in nested_target_dirs(&platform_root) {
                reclaimed += remove_build_dir(&nested, args.dry_run, &mut skipped)?;
            }
        }
    }

    let verb = if args.dry_run { "would reclaim" } else { "reclaimed" };
    eprintln!("[idealyst clean] {verb} {}", human_bytes(reclaimed));
    for dir in &skipped {
        eprintln!("[idealyst clean] skipped {} — a build is using it", dir.display());
    }
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

/// Every target dir the CLI's own builds use for one project.
struct CliDirs {
    /// Where the `idealyst-web-<key>` dirs (and their key records) live.
    web_root: PathBuf,
    /// The dev server's and the MCP catalog's isolated builds.
    side_builds: Vec<PathBuf>,
    /// Dirs an older CLI used and nothing builds into any more.
    legacy: Vec<PathBuf>,
}

impl CliDirs {
    fn resolve(source: &build_ios::FrameworkSource, project: &Path) -> Self {
        let web_root = source.cargo_target_dir(project);
        let cli_root = crate::framework_source::cli_target_root(source, project, None);
        let side_builds = vec![
            cli_root.join("idealyst-dev-server"),
            cli_root.join(super::catalog_wrapper::SIDECAR_TARGET_DIR),
        ];
        let mut legacy = vec![
            // Pre-key web target (one dir for every config).
            web_root.join("idealyst-web"),
            // Pre-`cli_target_root` placements under the app crate.
            project.join("target").join("idealyst-dev-server"),
            project.join("target").join(super::catalog_wrapper::SIDECAR_TARGET_DIR),
        ];
        legacy.retain(|d| !side_builds.contains(d));
        Self { web_root, side_builds, legacy }
    }

    /// `idealyst-web-<key>` dirs under the web root.
    fn web_dirs(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = fs::read_dir(&self.web_root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let key = name.strip_prefix("idealyst-web-")?.to_string();
                // `idealyst-web-keys` holds the records, not a build.
                (e.path().is_dir() && key != "keys").then(|| (key, e.path()))
            })
            .collect();
        out.sort();
        out
    }

    fn all_build_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.web_dirs().into_iter().map(|(_, p)| p).collect();
        out.extend(self.side_builds.iter().cloned());
        out.extend(self.legacy.iter().cloned());
        out
    }
}

/// `--stale`: drop what no current build uses, trim the rest.
fn sweep_stale(dirs: &CliDirs, platform_root: &Path, dry_run: bool, skipped: &mut Vec<PathBuf>) -> Result<u64> {
    let mut reclaimed = 0u64;
    let live = build_web::referenced_web_keys(&dirs.web_root);
    for (key, web) in dirs.web_dirs() {
        if !live.contains(&key) {
            reclaimed += remove_build_dir(&web, dry_run, skipped)?;
            continue;
        }
        // A web dir holds one config; without records, one live unit per
        // crate is the safe assumption.
        reclaimed += prune_target(&web, dry_run, |profile| {
            let mut r = target_gc::prune_units_keep_newest(profile, dry_run);
            absorb(&mut r, target_gc::prune_incremental_in_profile(profile, build_web::WEB_INCREMENTAL_KEEP, dry_run));
            r
        });
        // `incremental-hotpatch/` is never recorded (rustc writes it
        // outside cargo), so it is trimmed by recency even in a recorded dir.
        reclaimed += prune_each_profile(&web, |p| {
            target_gc::prune_incremental_in_profile(p, build_web::WEB_HOTPATCH_INCREMENTAL_KEEP, dry_run)
        });
    }
    for legacy in &dirs.legacy {
        reclaimed += remove_build_dir(legacy, dry_run, skipped)?;
    }
    // A shared host build can legitimately hold several live units of one
    // crate (build-dependency vs normal features, two apps' servers), so
    // WITHOUT records only its superseded incremental caches are trimmed —
    // never units.
    for side in &dirs.side_builds {
        reclaimed += prune_target(side, dry_run, |profile| {
            target_gc::prune_incremental_in_profile(
                profile,
                &[("incremental", super::catalog_wrapper::INCREMENTAL_KEEP_PER_CRATE)],
                dry_run,
            )
        });
    }
    // The generated wrappers' private target dirs. Without records a
    // wrapper dir holds a dev AND a non-dev build of every crate, so only
    // incremental caches beyond those two go.
    for nested in nested_target_dirs(platform_root) {
        reclaimed += prune_target(&nested, dry_run, |profile| {
            target_gc::prune_incremental_in_profile(profile, &[("incremental", 2)], dry_run)
        });
    }
    Ok(reclaimed)
}

/// Prune `target` by its records where it has them, and by `fallback` in
/// every profile dir that has none. Returns the bytes (a dry run: that
/// would be) freed.
fn prune_target(target: &Path, dry_run: bool, fallback: impl Fn(&Path) -> GcReport) -> u64 {
    let (mut report, unrecorded) = target_gc::prune_recorded(target, dry_run);
    for profile in &unrecorded {
        absorb(&mut report, fallback(profile));
    }
    report.bytes_freed
}

fn prune_each_profile(target: &Path, f: impl Fn(&Path) -> GcReport) -> u64 {
    target_gc::profile_dirs(target).iter().map(|p| f(p).bytes_freed).sum()
}

fn absorb(into: &mut GcReport, other: GcReport) {
    into.units_removed += other.units_removed;
    into.incremental_removed += other.incremental_removed;
    into.bytes_freed += other.bytes_freed;
    into.skipped_locked += other.skipped_locked;
}

/// Remove a whole build dir unless a build holds one of its cargo locks.
fn remove_build_dir(dir: &Path, dry_run: bool, skipped: &mut Vec<PathBuf>) -> Result<u64> {
    if !dir.exists() {
        return Ok(0);
    }
    let Some(locks) = target_gc::CargoBuildLocks::try_acquire_all(dir) else {
        skipped.push(dir.to_path_buf());
        return Ok(0);
    };
    let bytes = remove_path(dir, dry_run)?;
    drop(locks);
    Ok(bytes)
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

/// The recency fallback for one web target dir with no records: drop
/// every compilation unit except the most recently built one per crate,
/// in each profile dir (`target_gc::prune_units_keep_newest`).
#[cfg(test)]
fn prune_stale(web_target: &Path, dry_run: bool) -> Result<u64> {
    Ok(prune_each_profile(web_target, |p| target_gc::prune_units_keep_newest(p, dry_run)))
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
    target_gc::dir_size(path)
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

    /// A registry-sourced app inside a cargo workspace (CrewForge's shape).
    fn registry_app(root: &Path) -> (PathBuf, build_ios::FrameworkSource) {
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/app\"]\nresolver = \"2\"\n")
            .unwrap();
        let app = root.join("crates/app");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::write(app.join("src/lib.rs"), "").unwrap();
        fs::write(app.join("Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
            .unwrap();
        let source = build_ios::FrameworkSource::Registry {
            registry: "idealyst".into(),
            versions: Default::default(),
        };
        (app, source)
    }

    /// Regression (CrewForge, 2026-10-08): `clean` looked for
    /// `target/idealyst-web`, a name no build has used since the web dir
    /// became `idealyst-web-<key>`, and knew nothing of the dev-server or
    /// MCP builds — so it reclaimed nothing from any of them.
    #[test]
    fn regression_clean_misses_keyed_web_dirs_and_side_builds() {
        let root = tempfile::tempdir().unwrap();
        let (app, source) = registry_app(root.path());
        let dirs = CliDirs::resolve(&source, &app);
        let all = dirs.all_build_dirs();
        for d in ["idealyst-web-d9cd1286", "idealyst-web-b59982c6"] {
            fs::create_dir_all(app.join("target").join(d).join("wasm32-unknown-unknown/debug/deps"))
                .unwrap();
        }
        let all_now = CliDirs::resolve(&source, &app).all_build_dirs();
        assert!(all_now.contains(&app.join("target/idealyst-web-d9cd1286")), "{all_now:?}");
        assert!(all_now.contains(&app.join("target/idealyst-web-b59982c6")));
        // The workspace target as cargo reports it (honours CARGO_TARGET_DIR).
        let ws_target = crate::framework_source::cargo_workspace_target_dir(root.path()).unwrap();
        assert!(all.contains(&ws_target.join("idealyst-dev-server")), "{all:?}");
        assert!(all.contains(&ws_target.join("idealyst-mcp")));
        // The pre-workspace-root placement is legacy, cleaned too.
        assert!(all.contains(&app.join("target/idealyst-dev-server")));
        // The key records are not a build dir.
        fs::create_dir_all(app.join("target/idealyst-web-keys")).unwrap();
        assert!(!CliDirs::resolve(&source, &app)
            .all_build_dirs()
            .contains(&app.join("target/idealyst-web-keys")));
    }

    /// `--stale` drops the web dirs no slot records and the legacy dirs,
    /// keeps the recorded one, and leaves a locked dir alone.
    #[test]
    fn stale_sweep_drops_unrecorded_web_dirs_and_keeps_the_live_one() {
        let root = tempfile::tempdir().unwrap();
        let (app, source) = registry_app(root.path());
        let target = app.join("target");
        let live = target.join("idealyst-web-d9cd1286");
        let stale = target.join("idealyst-web-b59982c6");
        let locked = target.join("idealyst-web-d42137e0");
        for d in [&live, &stale, &locked] {
            touch(&d.join("wasm32-unknown-unknown/debug/deps/libx-0123456789abcdef.rlib"), 64);
        }
        build_web::record_web_key(&target, "app.dev.hotpatch", "d9cd1286");
        let legacy = target.join("idealyst-dev-server");
        touch(&legacy.join("debug/deps/libs-0123456789abcdef.rlib"), 64);
        let lock_path = locked.join("wasm32-unknown-unknown/debug/.cargo-lock");
        let lock = fs::File::create(&lock_path).unwrap();
        lock.lock().unwrap();

        let dirs = CliDirs::resolve(&source, &app);
        let mut skipped = Vec::new();
        let reclaimed = sweep_stale(&dirs, &app.join("target/idealyst"), false, &mut skipped).unwrap();
        assert!(reclaimed >= 128, "{reclaimed}");
        assert!(live.is_dir(), "the recorded dir is live");
        assert!(!stale.exists(), "an unrecorded dir is superseded");
        assert!(!legacy.exists(), "the per-app dev-server dir is legacy");
        assert!(locked.is_dir(), "a running build's dir is never removed");
        assert_eq!(skipped, vec![locked.clone()]);
    }

    /// `--stale` in a dir whose builds record their units removes exactly
    /// what the builds' own prune would: two apps sharing a web dir each
    /// keep their unit of a crate (keep-newest kept only one and cost the
    /// other app a rebuild), and only the unit no record names goes.
    #[test]
    fn regression_stale_sweep_ignores_records_and_drops_a_live_unit() {
        let root = tempfile::tempdir().unwrap();
        let (app, source) = registry_app(root.path());
        let target = app.join("target");
        let web = target.join("idealyst-web-d9cd1286");
        build_web::record_web_key(&target, "app.dev", "d9cd1286");
        let profile = web.join("wasm32-unknown-unknown/debug");
        let (lab, fiddle, gone) = ("1111111111111111", "2222222222222222", "3333333333333333");
        for (i, h) in [lab, fiddle, gone].iter().enumerate() {
            touch(&profile.join(format!(".fingerprint/idea-ui-{h}/lib-idea_ui")), 16);
            touch(&profile.join(format!("deps/libidea_ui-{h}.rlib")), 4096);
            // `lab` is the OLDEST: recency would delete it.
            filetime_set(
                &profile.join(format!(".fingerprint/idea-ui-{h}")),
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 * (i as u64 + 1)),
            );
        }
        let records = profile.join(target_gc::RECORDS_DIR);
        fs::create_dir_all(&records).unwrap();
        for (variant, h) in [("lab", lab), ("fiddle", fiddle)] {
            fs::write(records.join(format!("{variant}.json")), format!("{{\"hashes\":[\"{h}\"],\"incremental\":{{}}}}"))
                .unwrap();
        }
        let dirs = CliDirs::resolve(&source, &app);
        let mut skipped = Vec::new();
        let reclaimed = sweep_stale(&dirs, &app.join("target/idealyst"), false, &mut skipped).unwrap();
        assert_eq!(reclaimed, 16 + 4096, "exactly the unrecorded unit");
        for h in [lab, fiddle] {
            assert!(profile.join(format!("deps/libidea_ui-{h}.rlib")).is_file(), "{h} is recorded");
        }
        assert!(!profile.join(format!("deps/libidea_ui-{gone}.rlib")).exists());
    }

    /// The generated wrappers' private target dirs are swept too, by their
    /// records: a superseded generation goes, both recorded variants stay.
    #[test]
    fn stale_sweep_prunes_wrapper_targets_by_their_records() {
        let root = tempfile::tempdir().unwrap();
        let (app, source) = registry_app(root.path());
        let platform_root = app.join("target/idealyst");
        let profile = platform_root.join("app/macos/target/debug");
        let (dev, plain, old) = ("1111111111111111", "2222222222222222", "3333333333333333");
        for h in [dev, plain, old] {
            touch(&profile.join(format!("deps/libx-{h}.rlib")), 64);
        }
        let records = profile.join(target_gc::RECORDS_DIR);
        fs::create_dir_all(&records).unwrap();
        for (variant, h) in [("dev", dev), ("default", plain)] {
            fs::write(records.join(format!("{variant}.json")), format!("{{\"hashes\":[\"{h}\"],\"incremental\":{{}}}}"))
                .unwrap();
        }
        let dirs = CliDirs::resolve(&source, &app);
        let dry = sweep_stale(&dirs, &platform_root, true, &mut Vec::new()).unwrap();
        assert_eq!(dry, 64);
        assert!(profile.join(format!("deps/libx-{old}.rlib")).is_file(), "a dry run deletes nothing");
        assert_eq!(sweep_stale(&dirs, &platform_root, false, &mut Vec::new()).unwrap(), 64);
        assert!(!profile.join(format!("deps/libx-{old}.rlib")).exists());
        assert!(profile.join(format!("deps/libx-{dev}.rlib")).is_file());
        assert!(profile.join(format!("deps/libx-{plain}.rlib")).is_file());
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
