//! Windows backend.
//!
//! The SDK owns the flow (download + digest-verify), and this backend hands the
//! verified installer to the right Windows mechanism, picked by the artifact's
//! extension:
//!
//! - `.msi`  → `msiexec /i <file> /qn` (quiet install)
//! - `.msix` → `powershell Add-AppxPackage <file>`
//! - `.exe`  → run the installer with `/S` (NSIS/Inno silent convention)
//!
//! Like macOS, the installer can't overwrite a running `.exe`, so [`relaunch`]
//! spawns a detached `cmd` "waiter" that blocks until this PID exits, runs the
//! installer, then relaunches the app. The download is staged in [`apply`].
//!
//! > A packaged MSIX app can also be kept current declaratively by Windows App
//! > Installer from an `.appinstaller` manifest — no in-app code at all. This
//! > backend is the *in-app, on-demand* path that composes with the SDK's
//! > `UpdateState`.

use crate::{InstallKind, PreparedUpdate, UpdateError};

#[cfg(target_os = "windows")]
use std::cell::RefCell;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
thread_local! {
    /// The staged installer path recorded by [`apply`], consumed by [`relaunch`].
    static STAGED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(crate) fn install_kind() -> InstallKind {
    // SEAM: `GetCurrentPackageFullName` returning a non-empty name means we're
    // a packaged (MSIX) app whose updates Windows can own. For the in-app path
    // we treat everything as directly-distributed.
    InstallKind::Direct
}

pub(crate) async fn apply(_prepared: &PreparedUpdate) -> Result<(), UpdateError> {
    #[cfg(not(target_os = "windows"))]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "windows")]
    {
        let bytes = crate::download::download_verified(&_prepared.url, &_prepared.sha256).await?;
        let dir = crate::download::staging_dir()?;
        let name = crate::download::filename_from_url(&_prepared.url, "update.exe");
        let installer = dir.join(&name);
        std::fs::write(&installer, &bytes).map_err(|e| UpdateError::Install(e.to_string()))?;
        STAGED.with(|s| *s.borrow_mut() = Some(installer));
        Ok(())
    }
}

pub(crate) fn relaunch() -> Result<(), UpdateError> {
    #[cfg(not(target_os = "windows"))]
    {
        Err(UpdateError::Unsupported)
    }

    #[cfg(target_os = "windows")]
    {
        let installer = STAGED
            .with(|s| s.borrow_mut().take())
            .ok_or(UpdateError::NothingToInstall)?;
        spawn_waiter(&installer)?;
        std::process::exit(0);
    }
}

/// Spawn a detached `cmd` script that waits for this PID to exit, runs the
/// installer with the right silent invocation for its type, then relaunches us.
#[cfg(target_os = "windows")]
fn spawn_waiter(installer: &std::path::Path) -> Result<(), UpdateError> {
    use std::process::Command;

    let exe = std::env::current_exe().map_err(|e| UpdateError::Install(e.to_string()))?;
    let pid = std::process::id();
    let installer_str = installer.to_string_lossy();
    let ext = installer
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // The install command by artifact type.
    let install_cmd = match ext.as_str() {
        "msi" => format!("msiexec /i \"{installer_str}\" /qn"),
        "msix" => format!("powershell -NoProfile -Command \"Add-AppxPackage -Path '{installer_str}'\""),
        _ => format!("\"{installer_str}\" /S"),
    };

    let dir = crate::download::staging_dir()?;
    let script_path = dir.join("update.cmd");
    let script = format!(
        "@echo off\r\n\
         :wait\r\n\
         tasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 /nobreak >nul & goto wait)\r\n\
         {install_cmd}\r\n\
         start \"\" \"{exe}\"\r\n",
        exe = exe.to_string_lossy(),
    );
    std::fs::write(&script_path, script).map_err(|e| UpdateError::Install(e.to_string()))?;

    // Detach via `cmd /c start` so the waiter outlives this process.
    Command::new("cmd")
        .args(["/C", "start", "", "/MIN", "cmd", "/C"])
        .arg(&script_path)
        .spawn()
        .map_err(|e| UpdateError::Install(format!("could not launch updater: {e}")))?;
    Ok(())
}
