//! Making and stopping the devices a matrix run uses: create a simulator or
//! AVD the plan said to ([`matrix::Create`]), and after the row, shut down a
//! simulator or emulator the run booted (one that was already running is
//! left as it was).
//!
//! Booting itself is `idealyst dev`'s: given a simulator UDID or an AVD
//! name it boots the device when it isn't running, and leaves it up.
//!
//! Only devices the matrix names are touched, and only emulators are ever
//! stopped: a physical device (`serial:`) is never shut down.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::matrix::{self, Create};

/// Make the device; returns what `idealyst dev` is then pointed at: the new
/// simulator's UDID, or the AVD's name.
pub fn create(c: &Create) -> Result<String> {
    match c {
        Create::Simulator { name, device_type, runtime, runtime_name } => {
            eprintln!("[test] creating simulator \"{name}\" on {runtime_name}…");
            let out = Command::new("xcrun")
                .args(["simctl", "create", name, device_type, runtime])
                .output()
                .context("running xcrun simctl create")?;
            anyhow::ensure!(
                out.status.success(),
                "xcrun simctl create \"{name}\" failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            let udid = String::from_utf8_lossy(&out.stdout).trim().to_string();
            anyhow::ensure!(!udid.is_empty(), "xcrun simctl create \"{name}\" printed no UDID");
            Ok(udid)
        }
        Create::Avd { name, package, profile } => {
            eprintln!("[test] creating AVD {name} ({package}, {profile})…");
            let sdk = matrix::sdk_root().context("the Android SDK isn't installed")?;
            let avdmanager = matrix::avdmanager(&sdk).context("avdmanager isn't installed")?;
            let mut child = Command::new(&avdmanager)
                .args(["create", "avd", "-n", name, "-k", package, "-d", profile])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("running {}", avdmanager.display()))?;
            // "Do you wish to create a custom hardware profile?" — no.
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(b"no\n");
            }
            let out = child.wait_with_output().context("waiting for avdmanager")?;
            anyhow::ensure!(
                out.status.success(),
                "avdmanager create avd -n {name} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            Ok(name.clone())
        }
    }
}

/// A device a row runs on, to check and stop afterwards.
#[derive(Debug, Clone, PartialEq)]
pub enum Device {
    Simulator(String),
    Avd(String),
}

impl Device {
    /// Whether it is running now.
    pub fn running(&self) -> bool {
        match self {
            Device::Simulator(udid) => simulator_booted(udid),
            Device::Avd(name) => emulator_serial(name).is_some(),
        }
    }

    /// Stop it. Best-effort: a failure is printed, not fatal.
    pub fn stop(&self) {
        match self {
            Device::Simulator(udid) => {
                eprintln!("[test] shutting down simulator {udid} (this run booted it)");
                let _ = Command::new("xcrun").args(["simctl", "shutdown", udid]).status();
            }
            Device::Avd(name) => {
                let Some(serial) = emulator_serial(name) else { return };
                eprintln!("[test] shutting down emulator {name} ({serial}; this run booted it)");
                if let Some(adb) = adb() {
                    let _ = Command::new(adb).args(["-s", &serial, "emu", "kill"]).status();
                }
            }
        }
    }
}

fn simulator_booted(udid: &str) -> bool {
    let Ok(out) = Command::new("xcrun").args(["simctl", "list", "devices", "-j"]).output() else { return false };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return false };
    v["devices"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .flat_map(|list| list.as_array().into_iter().flatten())
        .any(|d| d["udid"] == udid && d["state"] == "Booted")
}

fn adb() -> Option<std::path::PathBuf> {
    Some(matrix::sdk_root()?.join("platform-tools/adb")).filter(|p| p.is_file())
}

/// The serial of the running emulator that is AVD `name`: only `emulator-*`
/// serials are asked, so a physical device is never matched.
fn emulator_serial(name: &str) -> Option<String> {
    let adb = adb()?;
    let out = Command::new(&adb).arg("devices").output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once('\t').map(|(s, _)| s.to_string()))
        .filter(|s| s.starts_with("emulator-"))
        .find(|serial| {
            Command::new(&adb)
                .args(["-s", serial, "emu", "avd", "name"])
                .output()
                .ok()
                .is_some_and(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(str::trim) == Some(name))
        })
}
