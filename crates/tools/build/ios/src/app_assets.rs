//! Shipping the app crate's `assets/` directory into every platform's
//! output, where `asset!` sources are read from at runtime.
//!
//! `asset!("images/logo.png")` names `assets/images/logo.png` in the
//! declaring crate (`docs/image-src.md`, "Bundled assets"). Each build path
//! copies the APP crate's `assets/` to the one place its backend reads:
//!
//! | Output | Destination | Who calls |
//! |---|---|---|
//! | web bundle | `<bundle>/assets/` | `build-web` `stage_bundle` (always, allowlist or not) |
//! | iOS simulator `.app` | `<App>.app/assets/` | `run-ios` |
//! | iOS device / App Store | `<project_root>/assets/` → folder reference → `<App>.app/assets/` | `run-ios` `prepare_xcode_project` |
//! | macOS `.app` | `<App>.app/Contents/Resources/assets/` | `run-macos` `assemble_app_bundle` |
//! | APK | zip entry `assets/<path>` → `AssetManager.open("<path>")` | `run-android` |
//! | desktop binaries (macOS bare, Linux, Windows, wgpu sim) | `<exe dir>/assets/` | `build-{macos,linux,windows,sim}` |
//!
//! Only the APP crate's `assets/` ships. Dependency crates are not walked:
//! the runtime namespace is flat (`assets/<path>`, no crate prefix), so two
//! crates' `images/logo.png` would collide silently, and framework crates
//! already keep unrelated `assets/` dirs (`icons-lucide/assets` is 7.7 MB of
//! SVG sources compiled in with `include_str!`) that every app would then
//! carry. A library that needs a file at runtime uses `embed_asset!`.
//!
//! [`sync_dir`] mirrors incrementally: a file whose size and mtime already
//! match is not rewritten, and files gone from the source are removed from
//! the destination. Symlinks INSIDE the tree are skipped, never followed —
//! a link pointing out of `assets/` must not drag outside files into a
//! shipped bundle (the web bundler's `copy_dir` has the same rule). The
//! `assets` directory itself may be a symlink; that is the author naming
//! their asset root, not a link inside it.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The directory name, in the app crate and in every output.
pub const ASSETS_DIR: &str = "assets";

/// The app crate's `assets/` directory, when it exists and is a directory.
pub fn app_assets_dir(project_dir: &Path) -> Option<PathBuf> {
    let dir = project_dir.join(ASSETS_DIR);
    dir.is_dir().then_some(dir)
}

/// What a [`sync_dir`] pass did. `copied + unchanged` is the number of
/// files in the source tree (symlinks excluded).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    /// Files written because they were new or changed.
    pub copied: usize,
    /// Files left alone because size and mtime already matched.
    pub unchanged: usize,
    /// Destination entries removed because the source no longer has them.
    pub removed: usize,
    /// Symlinks in the source that were skipped.
    pub skipped_symlinks: usize,
}

/// Mirror the app's `assets/` into `dest` (the `assets` directory inside
/// the output). With no `assets/` in the app, a `dest` left by an earlier
/// build is removed so deleting the folder really un-ships it. Returns
/// `true` when the app has assets (and `dest` now mirrors them).
pub fn stage_app_assets(project_dir: &Path, dest: &Path) -> Result<bool> {
    match app_assets_dir(project_dir) {
        Some(src) => {
            sync_dir(&src, dest)?;
            Ok(true)
        }
        None => {
            remove_path(dest)?;
            Ok(false)
        }
    }
}

/// [`stage_app_assets`] into `<dir of binary>/assets` — the desktop
/// location (Linux, Windows, wgpu, bare macOS binaries), read at runtime
/// relative to the running executable.
pub fn stage_app_assets_beside(project_dir: &Path, binary: &Path) -> Result<bool> {
    let exe_dir = binary
        .parent()
        .ok_or_else(|| anyhow::anyhow!("binary path has no parent: {}", binary.display()))?;
    stage_app_assets(project_dir, &exe_dir.join(ASSETS_DIR))
}

/// Make `dst` a copy of the regular-file tree under `src`.
///
/// Incremental: a destination file with the source's size and mtime is
/// left alone (the copy stamps the source mtime onto the destination, so
/// an unchanged file is skipped on every later pass). Everything under
/// `dst` that is not in `src` — stale files, directories, any symlink —
/// is removed. Symlinks under `src` are skipped, never followed.
pub fn sync_dir(src: &Path, dst: &Path) -> Result<SyncStats> {
    let src_canon =
        fs::canonicalize(src).with_context(|| format!("resolve {}", src.display()))?;
    // A destination inside the source would recurse into its own output.
    if let Some(parent) = dst.parent().and_then(|p| fs::canonicalize(p).ok()) {
        if parent.starts_with(&src_canon) {
            anyhow::bail!(
                "asset destination {} is inside the source {}",
                dst.display(),
                src.display()
            );
        }
    }
    let mut stats = SyncStats::default();
    sync_into(&src_canon, dst, &mut stats)?;
    Ok(stats)
}

fn sync_into(src: &Path, dst: &Path, stats: &mut SyncStats) -> Result<()> {
    // `dst` must be a real directory. Anything else in its place (a file,
    // a symlink — possibly to somewhere outside the output) is removed
    // rather than written through.
    match fs::symlink_metadata(dst) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => remove_path(dst)?,
        Err(_) => {}
    }
    fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;

    let mut kept: HashSet<OsString> = HashSet::new();
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let from = entry.path();
        let to = dst.join(&name);
        // `DirEntry::file_type` does not follow symlinks.
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            stats.skipped_symlinks += 1;
            continue;
        }
        if ft.is_dir() {
            sync_into(&from, &to, stats)?;
            kept.insert(name);
        } else if ft.is_file() {
            copy_if_changed(&from, &to, stats)?;
            kept.insert(name);
        }
        // Sockets, FIFOs, devices: not assets.
    }

    for entry in fs::read_dir(dst).with_context(|| format!("read {}", dst.display()))? {
        let entry = entry?;
        if !kept.contains(&entry.file_name()) {
            remove_path(&entry.path())?;
            stats.removed += 1;
        }
    }
    Ok(())
}

fn copy_if_changed(from: &Path, to: &Path, stats: &mut SyncStats) -> Result<()> {
    let src_meta = fs::metadata(from).with_context(|| format!("stat {}", from.display()))?;
    match fs::symlink_metadata(to) {
        Ok(dst_meta) if dst_meta.is_file() => {
            let same_len = dst_meta.len() == src_meta.len();
            let same_mtime = match (dst_meta.modified(), src_meta.modified()) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if same_len && same_mtime {
                stats.unchanged += 1;
                return Ok(());
            }
            // Remove first: an output file can be read-only (a signed
            // bundle's copy), which `fs::copy` would fail to open.
            remove_path(to)?;
        }
        Ok(_) => remove_path(to)?,
        Err(_) => {}
    }
    // `fs::copy` clones on APFS (`fclonefileat`), so even a full first copy
    // of a large tree costs no data blocks on macOS.
    fs::copy(from, to).with_context(|| format!("copy {} → {}", from.display(), to.display()))?;
    if let Ok(mtime) = src_meta.modified() {
        // Stamp the source mtime so the next pass sees an unchanged file.
        // Best effort: a filesystem without settable mtimes just recopies.
        let _ = fs::File::options().write(true).open(to).and_then(|f| f.set_modified(mtime));
    }
    stats.copied += 1;
    Ok(())
}

/// Remove `path` whatever it is, without following a symlink. Missing is fine.
fn remove_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with_logo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("assets/images")).unwrap();
        fs::write(tmp.path().join("assets/images/logo.png"), b"png").unwrap();
        tmp
    }

    #[test]
    fn stage_app_assets_mirrors_tree() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        assert!(stage_app_assets(project.path(), &dest).unwrap());
        assert_eq!(fs::read(dest.join("images/logo.png")).unwrap(), b"png");
    }

    #[test]
    fn sync_skips_unchanged_files_on_second_pass() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        let first = sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert_eq!(first.copied, 1);
        let second = sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert_eq!((second.copied, second.unchanged), (0, 1));
    }

    #[test]
    fn sync_recopies_a_changed_file() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        sync_dir(&project.path().join("assets"), &dest).unwrap();
        fs::write(project.path().join("assets/images/logo.png"), b"new-png").unwrap();
        let stats = sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert_eq!(stats.copied, 1);
        assert_eq!(fs::read(dest.join("images/logo.png")).unwrap(), b"new-png");
    }

    #[test]
    fn sync_removes_files_deleted_from_source() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        fs::write(project.path().join("assets/old.png"), b"old").unwrap();
        sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert!(dest.join("old.png").is_file());
        fs::remove_file(project.path().join("assets/old.png")).unwrap();
        let stats = sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert_eq!(stats.removed, 1);
        assert!(!dest.join("old.png").exists());
        assert!(dest.join("images/logo.png").is_file());
    }

    /// A symlink inside `assets/` pointing at a file outside the app must
    /// not put that file into a shipped bundle.
    #[cfg(unix)]
    #[test]
    fn regression_sync_never_follows_symlinks_out_of_assets() {
        let project = project_with_logo();
        let secret = project.path().join("secret.env");
        fs::write(&secret, b"TOKEN=1").unwrap();
        std::os::unix::fs::symlink(&secret, project.path().join("assets/leak.env")).unwrap();
        std::os::unix::fs::symlink(project.path(), project.path().join("assets/up")).unwrap();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        let stats = sync_dir(&project.path().join("assets"), &dest).unwrap();
        assert_eq!(stats.skipped_symlinks, 2);
        assert!(fs::symlink_metadata(dest.join("leak.env")).is_err());
        assert!(fs::symlink_metadata(dest.join("up")).is_err());
    }

    #[test]
    fn deleting_assets_dir_unships_previous_copy() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("assets");
        stage_app_assets(project.path(), &dest).unwrap();
        fs::remove_dir_all(project.path().join("assets")).unwrap();
        assert!(!stage_app_assets(project.path(), &dest).unwrap());
        assert!(!dest.exists());
    }

    #[test]
    fn desktop_binaries_get_assets_beside_the_exe() {
        let project = project_with_logo();
        let out = tempfile::tempdir().unwrap();
        let binary = out.path().join("debug/app-linux");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"elf").unwrap();
        assert!(stage_app_assets_beside(project.path(), &binary).unwrap());
        assert!(out.path().join("debug/assets/images/logo.png").is_file());
    }

    #[test]
    fn sync_refuses_destination_inside_source() {
        let project = project_with_logo();
        let src = project.path().join("assets");
        let err = sync_dir(&src, &src.join("images/out")).unwrap_err();
        assert!(err.to_string().contains("inside the source"));
    }
}
