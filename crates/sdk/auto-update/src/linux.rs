//! Linux backend.
//!
//! The portable desktop shape is an **AppImage** — a single self-contained
//! executable image. Because the running image is a mounted FUSE view, the
//! *file on disk* can be replaced while the app runs: [`apply`] downloads the
//! verified new image beside the current one, and [`relaunch`] atomically
//! renames it over `$APPIMAGE` and re-execs. Only runs when actually launched
//! from an AppImage (distro/package installs update out-of-band).
//!
//! > A production build can swap this whole-file replacement for
//! > AppImageUpdate's **zsync delta** (download only changed blocks) behind the
//! > same seam; the SDK flow above it is unchanged.

use crate::{InstallKind, PreparedUpdate, UpdateError};

#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
thread_local! {
    /// `(downloaded_new_image, target_appimage)` staged by [`apply`].
    static STAGED: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

pub(crate) fn install_kind() -> InstallKind {
    // The AppImage runtime exports `$APPIMAGE` as the mounted image path. Its
    // presence is the reliable "I am a self-updatable AppImage" signal.
    match std::env::var_os("APPIMAGE") {
        Some(_) => InstallKind::Direct,
        None => InstallKind::Unknown,
    }
}

pub(crate) async fn apply(_prepared: &PreparedUpdate) -> Result<(), UpdateError> {
    #[cfg(not(target_os = "linux"))]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;

        let target = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .ok_or_else(|| UpdateError::Install("not running from an AppImage ($APPIMAGE unset)".into()))?;

        let bytes = crate::download::download_verified(&_prepared.url, &_prepared.sha256).await?;

        // Stage next to the target so the later rename is a same-filesystem
        // atomic replace (a cross-device rename would fail).
        let staged = target.with_extension("new");
        std::fs::write(&staged, &bytes).map_err(|e| UpdateError::Install(e.to_string()))?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| UpdateError::Install(e.to_string()))?;

        STAGED.with(|s| *s.borrow_mut() = Some((staged, target)));
        Ok(())
    }
}

pub(crate) fn relaunch() -> Result<(), UpdateError> {
    #[cfg(not(target_os = "linux"))]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "linux")]
    {
        let (staged, target) = STAGED
            .with(|s| s.borrow_mut().take())
            .ok_or(UpdateError::NothingToInstall)?;

        // Atomic replace of the on-disk image, then re-exec the updated file.
        std::fs::rename(&staged, &target).map_err(|e| UpdateError::Install(e.to_string()))?;
        std::process::Command::new(&target)
            .spawn()
            .map_err(|e| UpdateError::Install(format!("could not relaunch AppImage: {e}")))?;
        std::process::exit(0);
    }
}
