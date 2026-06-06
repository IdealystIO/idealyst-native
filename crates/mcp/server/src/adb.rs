//! Host-side `adb` driver for **OS-level** input injection on Android
//! emulators / devices.
//!
//! The Robot bridge runs *inside* the app — on the device, which has no
//! `adb`. `adb` lives on the host, which is exactly where this MCP
//! server runs. So OS-level taps are issued here: we read the target
//! element's physical screen-pixel rect over the bridge (the
//! `get_device_frame` verb, backed by `Backend::device_frame`) and shell
//! out to `adb shell input tap`.
//!
//! Why this is different from the framework's `click` verb: `click`
//! invokes the element's handler closure directly and bypasses the
//! platform event system entirely, so it can't catch hit-testing /
//! overlay / disabled-state bugs. An `adb input tap` is a *real* OS
//! touch — it travels the full Android input stack (InputManager →
//! window → view hit-test), exactly as a finger would.
//!
//! Coordinate space: `adb input tap` takes physical display pixels with
//! origin at the screen top-left (status bar included). That is the same
//! space `Backend::device_frame` (Android `getLocationOnScreen`) reports,
//! so there is **no host-side density conversion** — the dp→px scaling is
//! applied on-device where the true density is known.

use anyhow::{bail, Context, Result};
use tokio::process::Command;

/// Resolve which adb serial to drive. An explicit serial is trusted
/// as-is. Otherwise we list attached devices and require exactly one
/// (the common single-emulator dev case); ambiguity is a hard error
/// listing the choices rather than a silent pick.
pub async fn resolve_serial(explicit: Option<&str>) -> Result<String> {
    if let Some(s) = explicit {
        return Ok(s.to_string());
    }
    let out = Command::new("adb")
        .arg("devices")
        .output()
        .await
        .context("failed to run `adb devices` — is Android platform-tools `adb` on PATH?")?;
    if !out.status.success() {
        bail!(
            "`adb devices` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let serials = parse_devices(&String::from_utf8_lossy(&out.stdout));
    match serials.as_slice() {
        [] => bail!(
            "no Android devices/emulators attached (`adb devices` is empty). \
             Boot an emulator or plug in a device, then retry."
        ),
        [one] => Ok(one.clone()),
        many => bail!(
            "{} Android devices attached ({}); pass `serial` to pick one",
            many.len(),
            many.join(", ")
        ),
    }
}

/// Parse `adb devices` stdout into the serials in the `device` state.
/// Skips the `List of devices attached` header and any device that's
/// `offline` / `unauthorized` / `no permissions` (a tap to those would
/// fail anyway). Tolerates the `* daemon started *` preamble adb prints
/// on a cold start.
pub fn parse_devices(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        // Drop everything up to and including the header line.
        .skip_while(|l| !l.trim_start().starts_with("List of devices attached"))
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') {
                return None;
            }
            let mut cols = line.split_whitespace();
            let serial = cols.next()?;
            let state = cols.next()?;
            (state == "device").then(|| serial.to_string())
        })
        .collect()
}

/// Issue a real OS touch at physical pixel `(x, y)` on `serial`.
pub async fn tap(serial: &str, x: i32, y: i32) -> Result<()> {
    let out = Command::new("adb")
        .args(["-s", serial, "shell", "input", "tap", &x.to_string(), &y.to_string()])
        .output()
        .await
        .context("failed to run `adb shell input tap`")?;
    if !out.status.success() {
        bail!(
            "`adb input tap` failed on {serial}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_devices_picks_only_ready_devices() {
        let out = "List of devices attached\n\
                   emulator-5554\tdevice\n\
                   emulator-5556\toffline\n\
                   ZY223abc\tunauthorized\n";
        assert_eq!(parse_devices(out), vec!["emulator-5554".to_string()]);
    }

    #[test]
    fn parse_devices_tolerates_daemon_preamble_and_blank_lines() {
        let out = "* daemon not running; starting now at tcp:5037\n\
                   * daemon started successfully\n\
                   List of devices attached\n\
                   \n\
                   ZY223abc\tdevice\n\
                   emulator-5554\tdevice\n";
        assert_eq!(
            parse_devices(out),
            vec!["ZY223abc".to_string(), "emulator-5554".to_string()]
        );
    }

    #[test]
    fn parse_devices_empty_when_none_attached() {
        assert!(parse_devices("List of devices attached\n\n").is_empty());
        // Missing header (defensive) → empty, never a panic.
        assert!(parse_devices("garbage\n").is_empty());
    }
}
