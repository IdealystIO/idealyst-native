//! Running the Windows wrapper under Wine on a macOS / Linux host.
//!
//! This is a dev/test path: it lets a machine with no Windows box run the
//! Windows backend for a smoke check or a `--parity …,windows` run. Four
//! things differ from launching the `.exe` natively, and each is handled
//! here rather than in the app:
//!
//! - **Wine ≥ 9.** Rust ≥ 1.78's `std` imports `ProcessPrng` from
//!   `bcryptprimitives.dll`, which Wine 8 does not ship, so the loader fails
//!   (`c0000135`) before `main`. The version is checked up front so that
//!   shows up as an actionable error, not a silent dead window.
//! - **A prefix of our own.** A newer Wine upgrades whatever prefix it runs
//!   in, in place, which can break that prefix for an older Wine used for
//!   other software. `WINEPREFIX` defaults to `~/.idealyst/wine-prefix`
//!   unless the caller set one.
//! - **Host paths.** Env values the launcher passes (the bridge port file)
//!   are host paths; the Windows process needs them as `Z:\…` (Wine maps
//!   `Z:` to `/` in every default prefix).
//! - **The app registry.** Wine PIDs are not host PIDs, and the bridge prunes
//!   `~/.idealyst/apps` by PID liveness, so a Wine app sharing it would delete
//!   every native app's registration (and vice versa). It registers into
//!   `~/.idealyst/apps-wine` instead (`IDEALYST_APPS_DIR`), which host tools
//!   scan alongside the native one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// The first Wine major version that ships `bcryptprimitives.dll`.
pub const MIN_WINE_MAJOR: u32 = 9;

/// Registry directory name for Wine-hosted apps, beside `~/.idealyst/apps`.
/// Must match `robot_test::WINE_APPS_DIR_NAME`, which host tools scan.
pub const WINE_APPS_DIR_NAME: &str = "apps-wine";

/// `wine <binary>`, with the env translated and the prefix / registry set up.
pub fn command(binary: &Path, env_vars: &[(String, String)]) -> Result<Command> {
    let wine = wine_binary();
    check_version(&wine)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);

    let mut cmd = Command::new(&wine);
    cmd.arg(binary);
    for (k, v) in env_vars {
        cmd.env(k, to_guest_value(v));
    }
    if std::env::var_os("WINEPREFIX").is_none() {
        if let Some(h) = &home {
            cmd.env("WINEPREFIX", h.join(".idealyst").join("wine-prefix"));
        }
    }
    // Wine's own `fixme:` chatter buries the app's log; errors stay visible.
    if std::env::var_os("WINEDEBUG").is_none() {
        cmd.env("WINEDEBUG", "-all,err+module");
    }
    if let Some(h) = &home {
        let dir = h.join(".idealyst").join(WINE_APPS_DIR_NAME);
        cmd.env("IDEALYST_APPS_DIR", to_guest_path(&dir.to_string_lossy()));
    }
    Ok(cmd)
}

/// `$IDEALYST_WINE`, else `wine` from `PATH`.
fn wine_binary() -> OsString {
    std::env::var_os("IDEALYST_WINE").unwrap_or_else(|| OsString::from("wine"))
}

fn check_version(wine: &OsString) -> Result<()> {
    let out = Command::new(wine).arg("--version").output().with_context(|| {
        format!(
            "running a Windows app on this host needs Wine ≥ {MIN_WINE_MAJOR}, and `{}` \
             could not be run. Install Wine (macOS: the WineHQ builds at \
             github.com/Gcenx/macOS_Wine_builds) and put it on PATH, or point \
             IDEALYST_WINE at its `wine` binary",
            wine.to_string_lossy()
        )
    })?;
    let text = String::from_utf8_lossy(&out.stdout);
    match parse_major(&text) {
        Some(major) if major >= MIN_WINE_MAJOR => Ok(()),
        Some(_) => anyhow::bail!(
            "`{}` is {} — Rust binaries need Wine ≥ {MIN_WINE_MAJOR} (older Wine has no \
             bcryptprimitives.dll, so the app dies in the loader). Install a newer Wine \
             (macOS: github.com/Gcenx/macOS_Wine_builds) and set IDEALYST_WINE to its \
             `wine` binary",
            wine.to_string_lossy(),
            text.trim()
        ),
        None => anyhow::bail!(
            "could not read a version from `{} --version` (got {:?})",
            wine.to_string_lossy(),
            text.trim()
        ),
    }
}

/// `"wine-11.18"` / `"wine-8.0.1 (CrossOverFOSS 23.7.1)"` / `"wine-9.0-rc1"`
/// → the major version.
fn parse_major(version: &str) -> Option<u32> {
    let rest = version.trim().strip_prefix("wine-")?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// A host absolute path as the Windows process sees it: `Z:` + the path with
/// backslashes. Anything that is not an absolute host path passes through.
fn to_guest_value(v: &str) -> String {
    if v.starts_with('/') { to_guest_path(v) } else { v.to_string() }
}

fn to_guest_path(p: &str) -> String {
    format!("Z:{}", p.replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wine 8 (the `wine-crossover` cask) loads no Rust ≥ 1.78 binary; it must
    /// be rejected by version, not discovered as a window that never opens.
    #[test]
    fn regression_wine_8_is_rejected_by_version() {
        assert_eq!(parse_major("wine-8.0.1 (CrossOverFOSS 23.7.1)\n"), Some(8));
        assert_eq!(parse_major("wine-11.18"), Some(11));
        assert_eq!(parse_major("wine-9.0-rc1"), Some(9));
        assert_eq!(parse_major("not wine"), None);
        assert!(parse_major("wine-8.0.1").unwrap() < MIN_WINE_MAJOR);
    }

    /// The bridge port file is a host path; the Windows process resolved
    /// `/Users/...` against its current drive and wrote it somewhere else.
    #[test]
    fn regression_host_paths_reach_the_guest_as_z_drive() {
        assert_eq!(
            to_guest_value("/Users/me/app/.idealyst/bridge.port"),
            r"Z:\Users\me\app\.idealyst\bridge.port"
        );
        assert_eq!(to_guest_value("7001"), "7001");
        assert_eq!(to_guest_value("http://127.0.0.1:3000"), "http://127.0.0.1:3000");
    }
}
