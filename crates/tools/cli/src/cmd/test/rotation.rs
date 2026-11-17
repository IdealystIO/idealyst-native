//! Turning a device's screen for robot-test's `set_orientation`, where the
//! app can't turn it itself: an Android device ([`AndroidScreen`]) and an
//! iOS simulator ([`SimulatorScreen`]).
//!
//! # Android
//!
//! An Android app can't turn the screen it runs on: it can only REQUEST an
//! orientation (`Activity.setRequestedOrientation`), and since Android 12L a
//! large screen ignores the request and letterboxes the app instead. A
//! `pixel_tablet` asked for portrait stayed a landscape display with the
//! app in a phone-sized box, so a tablet test turned "upright" was a phone
//! test. The runner turns the display itself, as a person turning the
//! device does: `settings put system user_rotation` with auto-rotate off,
//! over adb, on an emulator or a device alike. The settings it changed are
//! put back when the row ends (on drop).
//!
//! Only the device the row was pointed at is ever touched (its AVD's
//! emulator, or its `serial:`); a run that named no device doesn't turn
//! anything.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result};
use robot_test::Orientation;

/// The screen of a phone or tablet row, which the runner can turn.
pub enum DeviceScreen {
    Android(AndroidScreen),
    Simulator(SimulatorScreen),
}

impl DeviceScreen {
    /// The screen of `platform`'s `device` (an AVD, serial, simulator name
    /// or UDID); `None` on a platform whose screen the runner doesn't turn,
    /// or when its tools are missing.
    pub fn new(platform: &str, device: &str) -> Option<DeviceScreen> {
        match platform {
            "android" => AndroidScreen::new(device).map(DeviceScreen::Android),
            "ios" => Some(DeviceScreen::Simulator(SimulatorScreen::new(device))),
            _ => None,
        }
    }

    pub fn turn(&self, orientation: Orientation) -> Result<()> {
        match self {
            DeviceScreen::Android(s) => s.turn(orientation),
            DeviceScreen::Simulator(s) => s.turn(orientation),
        }
    }
}

/// One Android device's screen, turned by the runner.
pub struct AndroidScreen {
    adb: PathBuf,
    /// The device the row runs on: an AVD name or an adb serial, resolved to
    /// a serial on first use (the emulator is up by then).
    device: String,
    serial: Mutex<Option<String>>,
    /// `accelerometer_rotation` and `user_rotation` before the first turn.
    saved: Mutex<Option<(String, String)>>,
}

impl AndroidScreen {
    /// The screen of `device` (an AVD name or an adb serial); `None` when
    /// adb isn't installed.
    pub fn new(device: &str) -> Option<AndroidScreen> {
        Some(AndroidScreen {
            adb: super::provision::adb()?,
            device: device.to_string(),
            serial: Mutex::new(None),
            saved: Mutex::new(None),
        })
    }

    fn serial(&self) -> Result<String> {
        let mut serial = self.serial.lock().unwrap();
        if let Some(s) = serial.as_ref() {
            return Ok(s.clone());
        }
        let found = super::provision::emulator_serial(&self.device).or_else(|| {
            let out = Command::new(&self.adb).arg("devices").output().ok()?;
            attached(&String::from_utf8_lossy(&out.stdout)).into_iter().find(|s| *s == self.device)
        });
        let s = found.with_context(|| format!("{} is not running (adb doesn't list it)", self.device))?;
        *serial = Some(s.clone());
        Ok(s)
    }

    fn shell(&self, args: &[&str]) -> Result<String> {
        let serial = self.serial()?;
        let out = Command::new(&self.adb)
            .args(["-s", &serial, "shell"])
            .args(args)
            .output()
            .with_context(|| format!("running adb shell {}", args.join(" ")))?;
        anyhow::ensure!(
            out.status.success(),
            "adb -s {serial} shell {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Turn the display to `orientation`.
    pub fn turn(&self, orientation: Orientation) -> Result<()> {
        let natural = parse_physical_size(&self.shell(&["wm", "size"])?)
            .context("`wm size` printed no physical size")?;
        {
            let mut saved = self.saved.lock().unwrap();
            if saved.is_none() {
                let auto = self.shell(&["settings", "get", "system", "accelerometer_rotation"])?;
                let user = self.shell(&["settings", "get", "system", "user_rotation"])?;
                *saved = Some((auto, user));
            }
        }
        let rotation = user_rotation_for(natural.0 > natural.1, orientation).to_string();
        self.shell(&["settings", "put", "system", "accelerometer_rotation", "0"])?;
        self.shell(&["settings", "put", "system", "user_rotation", &rotation])?;
        Ok(())
    }
}

impl Drop for AndroidScreen {
    /// Put back the rotation settings the row changed. Best-effort: the
    /// device may already be gone.
    fn drop(&mut self) {
        let Some((auto, user)) = self.saved.lock().unwrap().take() else { return };
        for (key, value) in [("user_rotation", user), ("accelerometer_rotation", auto)] {
            // `null`: the setting was never written on this device.
            if value != "null" && !value.is_empty() {
                let _ = self.shell(&["settings", "put", "system", key, &value]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// iOS simulator
// ---------------------------------------------------------------------------

/// An iOS simulator's screen, turned from Simulator.app's Device ▸
/// Orientation menu — `simctl` has no rotate command. An iPhone app turns
/// itself (`requestGeometryUpdate`), but a multitasking iPad app can't:
/// iPadOS ignores its request, so the simulator is turned as a person turns
/// the iPad, and the app follows. Needs the simulator's window open (the
/// runner's `idealyst dev` opens Simulator.app) and permission for the
/// terminal to control the computer (Accessibility), which the error names
/// when it's missing. Turned back upright on drop if it was turned.
pub struct SimulatorScreen {
    /// The simulator's name or UDID.
    device: String,
    turned: Mutex<bool>,
}

impl SimulatorScreen {
    pub fn new(device: &str) -> SimulatorScreen {
        SimulatorScreen { device: device.to_string(), turned: Mutex::new(false) }
    }

    /// The booted simulator's name, which its window title starts with.
    fn name(&self) -> Result<String> {
        let out = Command::new("xcrun").args(["simctl", "list", "devices", "booted", "-j"]).output()?;
        let sims = super::matrix::parse_simctl(&out.stdout).map_err(anyhow::Error::msg)?;
        sims.into_iter()
            .find(|s| s.udid == self.device || s.name == self.device)
            .map(|s| s.name)
            .with_context(|| format!("simulator {} is not booted", self.device))
    }

    pub fn turn(&self, orientation: Orientation) -> Result<()> {
        let script = simulator_script(&self.name()?, orientation);
        let out = Command::new("osascript").arg("-e").arg(&script).output().context("running osascript")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("{}", simulator_error(err.trim()));
        }
        *self.turned.lock().unwrap() = orientation != Orientation::Portrait;
        Ok(())
    }
}

impl Drop for SimulatorScreen {
    fn drop(&mut self) {
        if *self.turned.lock().unwrap() {
            let _ = self.turn(Orientation::Portrait);
        }
    }
}

/// The Orientation menu item for `orientation`.
pub fn orientation_menu_item(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::Portrait => "Portrait",
        Orientation::Landscape => "Landscape Left",
    }
}

/// The AppleScript that raises simulator `name`'s window and picks the
/// orientation. A window title is `<name> – <runtime>`: matching up to the
/// dash keeps "iPad Air" from taking "iPad Air 11-inch (M3)"'s window.
pub fn simulator_script(name: &str, orientation: Orientation) -> String {
    let prefix = format!("{name} – ").replace('\\', "\\\\").replace('"', "\\\"");
    let item = orientation_menu_item(orientation);
    format!(
        "tell application \"Simulator\" to activate\n\
         tell application \"System Events\" to tell process \"Simulator\"\n\
         \tset w to first window whose name starts with \"{prefix}\"\n\
         \tperform action \"AXRaise\" of w\n\
         \tclick menu item \"{item}\" of menu 1 of menu item \"Orientation\" of menu \"Device\" of menu bar 1\n\
         end tell"
    )
}

/// osascript's error, said as what to do.
fn simulator_error(err: &str) -> String {
    // -1719 is both "not allowed assistive access" and "no such window":
    // the text tells them apart.
    if err.contains("assistive access") || err.contains("-25211") || err.contains("not allowed") {
        format!(
            "turning the simulator needs the terminal allowed to control the computer (System Settings ▸ \
             Privacy & Security ▸ Accessibility): {err}"
        )
    } else if err.contains("Can’t get window") || err.contains("Can't get window") || err.contains("-1719") {
        format!("Simulator.app shows no window for the simulator (open it with `open -a Simulator`): {err}")
    } else {
        format!("Simulator.app did not turn the simulator: {err}")
    }
}

/// `wm size`'s physical size, `(width, height)` in px at rotation 0.
pub fn parse_physical_size(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("Physical size:"))?;
    let (w, h) = line.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// The `user_rotation` (quarter turns from the display's natural
/// orientation) that shows `orientation`. A tablet's natural orientation is
/// landscape, a phone's portrait: a quarter turn makes the other one.
pub fn user_rotation_for(natural_landscape: bool, orientation: Orientation) -> u8 {
    match (natural_landscape, orientation) {
        (false, Orientation::Portrait) | (true, Orientation::Landscape) => 0,
        (false, Orientation::Landscape) | (true, Orientation::Portrait) => 1,
    }
}

/// The serials `adb devices` lists as ready.
fn attached(text: &str) -> Vec<String> {
    text.lines()
        .skip(1)
        .filter_map(|l| l.split_once('\t'))
        .filter(|(_, state)| state.trim() == "device")
        .map(|(s, _)| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_physical_size_is_read_from_wm_size() {
        assert_eq!(parse_physical_size("Physical size: 2560x1600\n"), Some((2560, 1600)));
        assert_eq!(
            parse_physical_size("Physical size: 1080x2400\nOverride size: 720x1600\n"),
            Some((1080, 2400)),
            "the override is not the display's natural shape"
        );
        assert_eq!(parse_physical_size("error: no devices"), None);
    }

    /// A phone is upright at rotation 0; a tablet (natural landscape) is
    /// upright a quarter turn round.
    #[test]
    fn the_rotation_follows_the_displays_natural_shape() {
        assert_eq!(user_rotation_for(false, Orientation::Portrait), 0);
        assert_eq!(user_rotation_for(false, Orientation::Landscape), 1);
        assert_eq!(user_rotation_for(true, Orientation::Portrait), 1);
        assert_eq!(user_rotation_for(true, Orientation::Landscape), 0);
    }

    /// The script targets the simulator's own window by its title up to
    /// the dash, and picks an absolute orientation.
    #[test]
    fn the_simulator_is_turned_from_its_own_windows_menu() {
        let script = simulator_script("iPad Air 11-inch (M3)", Orientation::Landscape);
        assert!(script.contains("whose name starts with \"iPad Air 11-inch (M3) – \""), "{script}");
        assert!(script.contains("click menu item \"Landscape Left\" of menu 1 of menu item \"Orientation\""), "{script}");
        assert!(simulator_script("iPhone 17", Orientation::Portrait).contains("\"Portrait\""));
        assert!(simulator_script("a \"b\"", Orientation::Portrait).contains("a \\\"b\\\" – "), "quotes escaped");
    }

    /// osascript's two -1719s are told apart by their text.
    #[test]
    fn a_simulator_failure_says_what_to_do() {
        let denied = "execution error: System Events got an error: osascript is not allowed assistive access. (-1719)";
        assert!(simulator_error(denied).contains("Privacy & Security ▸ Accessibility"));
        let no_window = "execution error: System Events got an error: Can’t get window 1 of process \"Simulator\" whose name starts with \"iPad – \". Invalid index. (-1719)";
        assert!(simulator_error(no_window).contains("open -a Simulator"));
    }

    #[test]
    fn only_ready_devices_are_attached() {
        let out = "List of devices attached\nemulator-5554\tdevice\nHA2HC863\tunauthorized\n";
        assert_eq!(attached(out), ["emulator-5554"]);
    }
}
