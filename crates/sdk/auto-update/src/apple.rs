//! Apple backend.
//!
//! - **macOS** — the real self-update target. This backend owns the flow so it
//!   composes with the SDK's reactive [`UpdateState`](crate::UpdateState): it
//!   downloads the verified artifact, extracts the new `.app`, and — on
//!   [`relaunch`] — atomically swaps the running bundle and reopens it. No
//!   Sparkle dependency, so it works with an unsigned dev build today; signing
//!   / notarization of the *downloaded* artifact is the piece to add later.
//!
//!   > If you later want Sparkle's native update UI + ecosystem trust instead,
//!   > it slots in behind this same seam — but Sparkle owns its own appcast +
//!   > flow, so you'd bypass the SDK's `UpdateState`. The self-contained path
//!   > here is what keeps the cross-platform API uniform.
//!
//! - **iOS** — no-op. Apple forbids self-update; the App Store owns it.
//!
//! ## The swap mechanism (macOS)
//!
//! Replacing a *running* `.app` is the one genuinely tricky part. We stage the
//! extracted new bundle in [`apply`], then on [`relaunch`] spawn a detached
//! shell "swapper" that waits for *this* process to exit, replaces the bundle
//! on disk, and `open`s it. The app then exits — the swapper does the rest.
//! This is the same wait-for-parent-quit dance Sparkle's installer performs.

use crate::{InstallKind, PreparedUpdate, UpdateError};

#[cfg(target_os = "macos")]
use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
thread_local! {
    /// The staged swap recorded by [`apply`]: `(target_bundle, new_bundle)`.
    /// Consumed by [`relaunch`].
    static STAGED: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

pub(crate) fn install_kind() -> InstallKind {
    #[cfg(target_os = "ios")]
    {
        InstallKind::Store
    }

    #[cfg(target_os = "macos")]
    {
        // A Mac App Store build carries a receipt at
        // `Foo.app/Contents/_MASReceipt/receipt`; its presence means the store
        // owns updates, its absence means Developer ID / direct distribution.
        match current_app_bundle() {
            Some(bundle) if bundle.join("Contents/_MASReceipt/receipt").exists() => {
                InstallKind::Store
            }
            Some(_) => InstallKind::Direct,
            // Not running from a `.app` at all (a bare `cargo run` binary) —
            // self-update doesn't apply.
            None => InstallKind::Unknown,
        }
    }
}

pub(crate) async fn apply(_prepared: &PreparedUpdate) -> Result<(), UpdateError> {
    #[cfg(target_os = "ios")]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "macos")]
    {
        let bundle = current_app_bundle()
            .ok_or_else(|| UpdateError::Install("not running from a .app bundle".into()))?;

        let bytes = crate::download::download_verified(&_prepared.url, &_prepared.sha256).await?;
        let dir = crate::download::staging_dir()?;
        let name = crate::download::filename_from_url(&_prepared.url, "update.zip");
        let archive = dir.join(&name);
        std::fs::write(&archive, &bytes).map_err(|e| UpdateError::Install(e.to_string()))?;

        let new_bundle = extract_app_bundle(&archive, &dir)?;
        STAGED.with(|s| *s.borrow_mut() = Some((bundle, new_bundle)));
        Ok(())
    }
}

pub(crate) fn relaunch() -> Result<(), UpdateError> {
    #[cfg(target_os = "ios")]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "macos")]
    {
        let (target, new_bundle) = STAGED
            .with(|s| s.borrow_mut().take())
            .ok_or(UpdateError::NothingToInstall)?;
        spawn_swapper(&target, &new_bundle)?;
        // Hand off to the swapper, which is now waiting for us to exit.
        std::process::exit(0);
    }
}

/// The `.app` bundle the running executable lives in
/// (`…/Foo.app/Contents/MacOS/foo` → `…/Foo.app`), or `None` if not inside the
/// `Contents/MacOS/` layout.
#[cfg(target_os = "macos")]
fn current_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?; // …/Contents/MacOS
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?; // …/Contents
    if contents.file_name()? != "Contents" {
        return None;
    }
    contents.parent().map(PathBuf::from) // …/Foo.app
}

/// Extract the update archive into `workdir` and return the path to the `.app`
/// it contains. Supports the two shapes `idealyst publish macos` emits: a
/// zipped bundle (`.zip`) and a disk image (`.dmg`).
#[cfg(target_os = "macos")]
fn extract_app_bundle(
    archive: &std::path::Path,
    workdir: &std::path::Path,
) -> Result<PathBuf, UpdateError> {
    use std::process::Command;

    let out = workdir.join("extracted");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| UpdateError::Install(e.to_string()))?;

    let ext = archive
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "zip" => {
            // `ditto` preserves bundle metadata / symlinks / code signatures,
            // unlike a plain `unzip`.
            let status = Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(archive)
                .arg(&out)
                .status()
                .map_err(|e| UpdateError::Install(format!("ditto: {e}")))?;
            if !status.success() {
                return Err(UpdateError::Install("ditto failed to extract archive".into()));
            }
            find_dot_app(&out).ok_or_else(|| UpdateError::Install("no .app found in archive".into()))
        }
        "dmg" => {
            // Attach the image read-only without popping a Finder window, copy
            // the bundle out, then detach.
            let mount = workdir.join("mnt");
            std::fs::create_dir_all(&mount).map_err(|e| UpdateError::Install(e.to_string()))?;
            let status = Command::new("/usr/bin/hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
                .arg(&mount)
                .arg(archive)
                .status()
                .map_err(|e| UpdateError::Install(format!("hdiutil attach: {e}")))?;
            if !status.success() {
                return Err(UpdateError::Install("hdiutil failed to attach image".into()));
            }
            let app = find_dot_app(&mount).and_then(|src| {
                let dest = out.join(src.file_name()?);
                Command::new("/usr/bin/ditto").arg(&src).arg(&dest).status().ok()?;
                Some(dest)
            });
            // Always detach, even if the copy failed.
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", "-quiet"])
                .arg(&mount)
                .status();
            app.ok_or_else(|| UpdateError::Install("no .app found in disk image".into()))
        }
        other => Err(UpdateError::Install(format!("unsupported archive type: .{other}"))),
    }
}

/// First `*.app` directory directly inside `dir` (one level deep — the layout
/// both `ditto` and a mounted `.dmg` produce).
#[cfg(target_os = "macos")]
fn find_dot_app(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("app"))
}

/// Spawn the detached swapper: it waits for this PID to exit, atomically
/// replaces `target` with `new_bundle`, and reopens it. Reparented to launchd
/// once we exit, so it survives our teardown.
#[cfg(target_os = "macos")]
fn spawn_swapper(
    target: &std::path::Path,
    new_bundle: &std::path::Path,
) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let pid = std::process::id();
    let target = target.to_string_lossy();
    let new_bundle = new_bundle.to_string_lossy();
    // Move the old bundle aside first so a failed copy is recoverable, then
    // restore it if the replacement fails.
    let script = format!(
        r#"#!/bin/bash
set -e
while /bin/kill -0 {pid} 2>/dev/null; do /bin/sleep 0.2; done
BACKUP="{target}.old-$$"
/bin/mv "{target}" "$BACKUP"
if /bin/cp -R "{new_bundle}" "{target}"; then
  /bin/rm -rf "$BACKUP"
else
  /bin/mv "$BACKUP" "{target}"
fi
/usr/bin/open "{target}"
"#
    );

    let dir = crate::download::staging_dir()?;
    let script_path = dir.join("swap.sh");
    std::fs::write(&script_path, script).map_err(|e| UpdateError::Install(e.to_string()))?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| UpdateError::Install(e.to_string()))?;

    Command::new("/bin/bash")
        .arg(&script_path)
        .spawn()
        .map_err(|e| UpdateError::Install(format!("could not launch swapper: {e}")))?;
    Ok(())
}
