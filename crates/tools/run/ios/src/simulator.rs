//! Picking the iOS simulator to run on.
//!
//! `idealyst run ios` / `idealyst dev --ios` take `--simulator <NAME|UDID>`.
//! Without it they reuse the first booted iOS simulator, and boot the first
//! iPhone on the newest runtime when none is booted.
//!
//! The device list comes from `xcrun simctl list devices --json`. The JSON is
//! parsed by [`select_simulator`], a pure function, so the selection rules are
//! unit-tested against captured output. Older Xcodes spell availability as
//! `"availability": "(available)"` and key runtimes as `"iOS 12.0"`; newer ones
//! use `"isAvailable": true` and `"com.apple.CoreSimulator.SimRuntime.iOS-17-5"`.
//! Both are accepted.

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

/// One iOS simulator from `simctl list devices --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulator {
    pub udid: String,
    pub name: String,
    /// The runtime key, e.g. `com.apple.CoreSimulator.SimRuntime.iOS-17-5`.
    pub runtime: String,
    pub booted: bool,
}

/// Choose a simulator from `simctl list devices --json` output.
///
/// - `wanted = Some(s)`: the available iOS simulator whose UDID equals `s`, or
///   else whose name equals `s` (both case-insensitive). When several share
///   the name (one per installed runtime), a booted one wins, then the
///   newest runtime.
/// - `wanted = None`: the first booted iOS simulator (newest runtime first);
///   if none is booted, the first iPhone on the newest runtime.
///
/// Only iOS runtimes and available devices are considered.
pub fn select_simulator(list: &Value, wanted: Option<&str>) -> Result<Simulator> {
    let sims = ios_simulators(list);
    match wanted {
        Some(wanted) => {
            let wanted = wanted.trim();
            if let Some(sim) = sims.iter().find(|s| s.udid.eq_ignore_ascii_case(wanted)) {
                return Ok(sim.clone());
            }
            let by_name = sims.iter().filter(|s| s.name.eq_ignore_ascii_case(wanted));
            // `sims` is newest-runtime first, so the first booted (else the
            // first) match is the right one.
            let by_name: Vec<&Simulator> = by_name.collect();
            if let Some(sim) = by_name.iter().find(|s| s.booted).or(by_name.first()) {
                return Ok((*sim).clone());
            }
            let mut names: Vec<&str> = sims.iter().map(|s| s.name.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            anyhow::bail!(
                "no available iOS simulator named or with UDID `{wanted}`. Available: {}. \
                 `xcrun simctl list devices available` lists them with their UDIDs.",
                if names.is_empty() { "none".to_string() } else { names.join(", ") },
            )
        }
        None => {
            if let Some(sim) = sims.iter().find(|s| s.booted) {
                return Ok(sim.clone());
            }
            if let Some(sim) = sims.iter().find(|s| s.name.starts_with("iPhone")) {
                return Ok(sim.clone());
            }
            anyhow::bail!(
                "no available iPhone simulator found — run `xcrun simctl list devices available` \
                 to see what's installed, or `xcodebuild -downloadPlatform iOS` to fetch a runtime"
            )
        }
    }
}

/// Every available iOS simulator, newest runtime first, keeping simctl's
/// order within a runtime.
fn ios_simulators(list: &Value) -> Vec<Simulator> {
    let Some(runtimes) = list.get("devices").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut keyed: Vec<(Vec<u32>, &String, &Vec<Value>)> = runtimes
        .iter()
        .filter_map(|(runtime, devices)| {
            let version = ios_runtime_version(runtime)?;
            Some((version, runtime, devices.as_array()?))
        })
        .collect();
    // Newest first. A stable sort keeps equal versions in key order.
    keyed.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::new();
    for (_, runtime, devices) in keyed {
        for d in devices {
            let available = match (d.get("isAvailable"), d.get("availability")) {
                (Some(v), _) => v.as_bool().unwrap_or(false),
                (None, Some(v)) => v.as_str() == Some("(available)"),
                (None, None) => true,
            };
            let (Some(udid), Some(name)) = (
                d.get("udid").and_then(Value::as_str),
                d.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            if !available {
                continue;
            }
            out.push(Simulator {
                udid: udid.to_string(),
                name: name.to_string(),
                runtime: runtime.clone(),
                booted: d.get("state").and_then(Value::as_str) == Some("Booted"),
            });
        }
    }
    out
}

/// The version of an iOS runtime key (`…SimRuntime.iOS-17-5` → `[17, 5]`,
/// `iOS 12.0` → `[12, 0]`); `None` for watchOS / tvOS / visionOS runtimes.
fn ios_runtime_version(runtime: &str) -> Option<Vec<u32>> {
    let rest = if let Some(i) = runtime.find("SimRuntime.iOS-") {
        &runtime[i + "SimRuntime.iOS-".len()..]
    } else {
        runtime.strip_prefix("iOS ")?
    };
    Some(
        rest.split(['-', '.'])
            .map_while(|p| p.parse::<u32>().ok())
            .collect(),
    )
}

/// Run `xcrun simctl list devices --json` and select from it.
pub(crate) fn find_simulator(wanted: Option<&str>) -> Result<Simulator> {
    let out = Command::new("xcrun")
        .args(["simctl", "list", "devices", "--json"])
        .output()
        .with_context(|| "spawn xcrun simctl list devices --json")?;
    if !out.status.success() {
        anyhow::bail!(
            "xcrun simctl list devices --json failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let list: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| "parse `xcrun simctl list devices --json` output")?;
    select_simulator(&list, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Shape of `xcrun simctl list devices --json` (Xcode 15/16), trimmed.
    fn sample() -> Value {
        json!({
            "devices": {
                "com.apple.CoreSimulator.SimRuntime.watchOS-10-5": [
                    { "udid": "W-1", "name": "Apple Watch Series 9 (45mm)", "state": "Booted", "isAvailable": true }
                ],
                "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                    { "udid": "OLD-IPHONE", "name": "iPhone 15", "state": "Shutdown", "isAvailable": true },
                    { "udid": "OLD-IPAD", "name": "iPad Pro 13-inch (M4)", "state": "Shutdown", "isAvailable": true }
                ],
                "com.apple.CoreSimulator.SimRuntime.iOS-18-2": [
                    { "udid": "NEW-IPAD", "name": "iPad Pro 13-inch (M4)", "state": "Shutdown", "isAvailable": true },
                    { "udid": "NEW-IPHONE", "name": "iPhone 16", "state": "Shutdown", "isAvailable": true },
                    { "udid": "GONE", "name": "iPhone 16 Pro", "state": "Shutdown", "isAvailable": false,
                      "availabilityError": "runtime profile not found" }
                ]
            }
        })
    }

    fn with_booted(mut list: Value, udid: &str) -> Value {
        for devices in list["devices"].as_object_mut().unwrap().values_mut() {
            for d in devices.as_array_mut().unwrap() {
                if d["udid"] == udid {
                    d["state"] = json!("Booted");
                }
            }
        }
        list
    }

    #[test]
    fn default_reuses_first_booted_ios_simulator() {
        let list = with_booted(sample(), "OLD-IPAD");
        let sim = select_simulator(&list, None).unwrap();
        assert_eq!(sim.udid, "OLD-IPAD");
        assert!(sim.booted);
    }

    /// A booted watchOS simulator is not an iOS target.
    #[test]
    fn default_ignores_non_ios_runtimes() {
        let sim = select_simulator(&sample(), None).unwrap();
        assert_eq!(sim.udid, "NEW-IPHONE", "nothing iOS booted → newest-runtime iPhone");
        assert!(!sim.booted);
    }

    #[test]
    fn selects_by_udid_case_insensitive() {
        let sim = select_simulator(&sample(), Some("old-iphone")).unwrap();
        assert_eq!(sim.udid, "OLD-IPHONE");
    }

    #[test]
    fn selects_by_name_preferring_newest_runtime() {
        let sim = select_simulator(&sample(), Some("iPad Pro 13-inch (M4)")).unwrap();
        assert_eq!(sim.udid, "NEW-IPAD");
        assert_eq!(sim.runtime, "com.apple.CoreSimulator.SimRuntime.iOS-18-2");
    }

    #[test]
    fn selects_by_name_preferring_booted() {
        let list = with_booted(sample(), "OLD-IPAD");
        let sim = select_simulator(&list, Some("ipad pro 13-inch (m4)")).unwrap();
        assert_eq!(sim.udid, "OLD-IPAD");
    }

    #[test]
    fn unavailable_simulator_is_not_selectable() {
        let err = select_simulator(&sample(), Some("iPhone 16 Pro")).unwrap_err().to_string();
        assert!(err.contains("no available iOS simulator"), "{err}");
        assert!(err.contains("iPad Pro 13-inch (M4)"), "lists what is available: {err}");
        assert!(!err.contains("Apple Watch"), "non-iOS devices aren't offered: {err}");
    }

    #[test]
    fn legacy_availability_string_and_runtime_key() {
        let list = json!({
            "devices": {
                "iOS 12.0": [
                    { "udid": "A", "name": "iPhone XS", "state": "Shutdown", "availability": "(unavailable, runtime profile not found)" },
                    { "udid": "B", "name": "iPhone XR", "state": "Shutdown", "availability": "(available)" }
                ]
            }
        });
        assert_eq!(select_simulator(&list, None).unwrap().udid, "B");
    }

    #[test]
    fn runtime_versions_order_numerically() {
        assert_eq!(ios_runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-17-5"), Some(vec![17, 5]));
        assert_eq!(ios_runtime_version("iOS 12.0"), Some(vec![12, 0]));
        assert_eq!(ios_runtime_version("com.apple.CoreSimulator.SimRuntime.tvOS-17-5"), None);
        assert!(vec![9u32, 3] < vec![17, 0], "9.3 sorts before 17.0 numerically");
    }

    #[test]
    fn no_ios_simulators_is_an_error() {
        let list = json!({ "devices": {} });
        assert!(select_simulator(&list, None).is_err());
    }
}
