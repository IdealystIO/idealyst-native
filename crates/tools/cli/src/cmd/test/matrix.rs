//! The device matrix: which device each class (`phone`, `tablet`,
//! `desktop`, or any name the project picks) runs as on each platform,
//! declared once in the project's manifest, and the plan of what this
//! machine can actually run.
//!
//! ```toml
//! [package.metadata.idealyst.test.devices]
//! phone   = { web = "chromium:iphone-15", ios = "iPhone 15 @ iOS 17", android = "pixel_7 @ 34" }
//! tablet  = { web = "chromium:ipad-air",  ios = "iPad Air 11-inch (M2)", android = "Pixel_Tablet_API_34" }
//! desktop = { web = "1440x900", macos = "1440x900", windows = "1440x900", linux = "1440x900" }
//! ```
//!
//! A device spec per platform:
//!
//! - `web`: `[engine:]preset` or `[engine:]WxH`. The engine is `chromium`
//!   (the default), `firefox` (headless, WebDriver BiDi) or `webkit`
//!   (Safari through `safaridriver`, macOS only); see `engines`.
//! - `ios`: a simulator name, optionally `@ iOS <version>` to pin its
//!   runtime (`iPhone SE (3rd generation) @ iOS 17`).
//! - `android`: an AVD name, `<device profile> @ <api level>` (an AVD made
//!   from that hardware profile and system image), or `serial:<adb serial>`
//!   for an attached device.
//! - `macos` / `linux` / `windows`: the window size, `WxH`.
//!
//! [`plan`] turns the matrix into one [`Row`] per class × platform the run
//! asked for, each either a [`Target`] to run or the reason it can't run
//! here: the project doesn't build for the platform (its `targets`), the
//! host can't run it, a runtime, image or browser isn't installed, an
//! image is for another CPU architecture. The runner prints the plan before
//! anything builds and repeats the skips in the final report: nothing is
//! dropped silently.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

/// The platforms, in the order the plan lists them.
pub const PLATFORMS: [&str; 6] = ["web", "ios", "android", "macos", "linux", "windows"];

/// The project's device matrix: class → platform → device spec, in the
/// manifest's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Matrix {
    pub classes: Vec<(String, BTreeMap<String, String>)>,
}

/// What the project's manifest says about testing.
#[derive(Debug, Default)]
pub struct ProjectTest {
    pub matrix: Option<Matrix>,
    /// `[package.metadata.idealyst.app] targets`; `None` when absent.
    pub targets: Option<Vec<String>>,
}

/// Read the matrix and the app's targets from `dir/Cargo.toml`.
pub fn load(dir: &Path) -> Result<ProjectTest> {
    let path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("in {}", path.display()))
}

pub fn parse(manifest: &str) -> Result<ProjectTest> {
    // `toml_edit`, not `toml`: it keeps the manifest's order, which is the
    // order the plan runs the classes in.
    let doc: toml_edit::DocumentMut = manifest.parse()?;
    let idealyst = doc.get("package").and_then(|p| p.get("metadata")).and_then(|m| m.get("idealyst"));
    let targets = idealyst
        .and_then(|i| i.get("app"))
        .and_then(|a| a.get("targets"))
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
    let Some(devices) = idealyst.and_then(|i| i.get("test")).and_then(|t| t.get("devices")) else {
        return Ok(ProjectTest { matrix: None, targets });
    };
    let table = devices.as_table_like().context("[package.metadata.idealyst.test.devices] must be a table")?;
    let mut classes = Vec::new();
    for (class, row) in table.iter() {
        let row = row
            .as_table_like()
            .with_context(|| format!("test.devices.{class} must be a table of platform = \"device\""))?;
        let mut specs = BTreeMap::new();
        for (platform, spec) in row.iter() {
            anyhow::ensure!(
                PLATFORMS.contains(&platform),
                "test.devices.{class}: unknown platform `{platform}` (use {})",
                PLATFORMS.join(", ")
            );
            let spec = spec.as_str().with_context(|| format!("test.devices.{class}.{platform} must be a string"))?;
            specs.insert(platform.to_string(), spec.trim().to_string());
        }
        classes.push((class.to_string(), specs));
    }
    Ok(ProjectTest { matrix: Some(Matrix { classes }), targets })
}

/// A device the run makes before it can run on it: from what this machine
/// already has installed (a simulator runtime, a system image), never a
/// download.
#[derive(Debug, Clone, PartialEq)]
pub enum Create {
    /// `xcrun simctl create <name> <device type> <runtime>`.
    Simulator { name: String, device_type: String, runtime: String, runtime_name: String },
    /// `avdmanager create avd -n <name> -k <package> -d <profile>`.
    Avd { name: String, package: String, profile: String },
}

impl Create {
    fn describe(&self) -> String {
        match self {
            Create::Simulator { name, runtime_name, .. } => format!("creates simulator \"{name}\" on {runtime_name} first"),
            Create::Avd { name, package, .. } => format!("creates AVD {name} ({package}) first"),
        }
    }
}

/// One run the plan can make: what `idealyst test` would be told by flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// A device to make first; the run then points `simulator` /
    /// `emulator` at it.
    pub create: Option<Create>,
    /// `--simulator` (a UDID or name).
    pub simulator: Option<String>,
    /// `--emulator` (an AVD name or adb serial).
    pub emulator: Option<String>,
    /// `--device`: a web emulation preset.
    pub device: Option<String>,
    /// `--viewport`.
    pub viewport: Option<(u32, u32)>,
    /// Web: the engine (`None`: Chromium).
    pub engine: Option<super::engines::Engine>,
}

/// One class × platform of the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub class: String,
    pub platform: String,
    /// The spec as written.
    pub spec: String,
    pub plan: Result<Target, String>,
}

impl Row {
    /// How the plan and the report name this row.
    pub fn label(&self) -> String {
        format!("{} on {} ({})", self.class, self.platform, self.spec)
    }
}

/// An iOS simulator, as `simctl` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Simulator {
    pub name: String,
    pub udid: String,
    /// `iOS 17.5`.
    pub runtime: String,
}

/// An Android virtual device.
#[derive(Debug, Clone, PartialEq)]
pub struct Avd {
    pub name: String,
    /// `hw.device.name` (`pixel_7`).
    pub device: Option<String>,
    /// The system image's API level.
    pub api: Option<u32>,
    /// The system image's ABI (`arm64-v8a`, `x86_64`).
    pub abi: Option<String>,
}

/// What this machine has: the probes the plan asks, behind a trait so the
/// plan is tested without a Mac, an SDK or a browser.
pub trait Host {
    /// `macos`, `linux`, `windows`.
    fn os(&self) -> &str;
    /// `aarch64`, `x86_64`.
    fn arch(&self) -> &str;
    /// Whether a Chromium-family browser is installed.
    fn chromium(&self) -> bool;
    /// Whether Firefox is installed.
    fn firefox(&self) -> bool;
    /// Whether `safaridriver` is installed (macOS).
    fn safaridriver(&self) -> bool;
    /// The available simulators, or why there are none to ask.
    fn simulators(&self) -> Result<Vec<Simulator>, String>;
    /// The AVDs, or why there are none to ask (no SDK).
    fn avds(&self) -> Result<Vec<Avd>, String>;
    /// Attached adb serials, or why they can't be listed.
    fn adb_serials(&self) -> Result<Vec<String>, String>;
    /// Whether a remote Windows machine is configured.
    fn windows_remote(&self) -> bool;
    /// The installed iOS simulator runtimes, or why they can't be listed.
    fn ios_runtimes(&self) -> Result<Vec<IosRuntime>, String>;
    /// The installed Android system images, or why they can't be listed.
    fn android_images(&self) -> Result<Vec<SystemImage>, String>;
    /// `avdmanager`'s hardware profile ids, or why it can't be asked.
    fn android_profiles(&self) -> Result<Vec<String>, String>;
}

/// An installed iOS simulator runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct IosRuntime {
    /// `iOS 26.5`.
    pub name: String,
    /// `26.5` (or `18.3.1`).
    pub version: String,
    pub identifier: String,
    /// The device types it runs: (name, identifier).
    pub device_types: Vec<(String, String)>,
}

/// An installed Android system image.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemImage {
    pub api: u32,
    /// `google_apis`, `google_apis_playstore`, `default`, …
    pub tag: String,
    pub abi: String,
}

/// What the run asked for: classes and platforms to keep (empty: all).
#[derive(Debug, Default)]
pub struct Filter {
    pub classes: Vec<String>,
    pub platforms: Vec<String>,
    /// Don't create missing simulators / AVDs (`--no-create`): report them.
    pub no_create: bool,
}

/// The plan: one row per class × platform the matrix names and the filter
/// keeps, in matrix order then [`PLATFORMS`] order.
pub fn plan(matrix: &Matrix, targets: Option<&[String]>, filter: &Filter, host: &dyn Host) -> Result<Vec<Row>> {
    for c in &filter.classes {
        anyhow::ensure!(
            matrix.classes.iter().any(|(name, _)| name == c),
            "--devices {c}: the matrix has no class `{c}` (it has {})",
            matrix.classes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    let mut rows = Vec::new();
    for (class, specs) in &matrix.classes {
        if !filter.classes.is_empty() && !filter.classes.contains(class) {
            continue;
        }
        for platform in PLATFORMS {
            let Some(spec) = specs.get(platform) else { continue };
            if !filter.platforms.is_empty() && !filter.platforms.iter().any(|p| p == platform) {
                continue;
            }
            let plan = match targets {
                Some(t) if !t.iter().any(|t| t == platform) => Err(format!(
                    "the project doesn't build for {platform} ([package.metadata.idealyst.app] targets = [{}])",
                    t.join(", ")
                )),
                _ => resolve(platform, spec, host, !filter.no_create),
            };
            rows.push(Row { class: class.clone(), platform: platform.to_string(), spec: spec.clone(), plan });
        }
    }
    Ok(rows)
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// The target for one spec on one platform, or why it can't run here.
pub fn resolve(platform: &str, spec: &str, host: &dyn Host, create: bool) -> Result<Target, String> {
    let none = Target { create: None, simulator: None, emulator: None, device: None, viewport: None, engine: None };
    match platform {
        "web" => {
            let (engine, rest) = match spec.split_once(':') {
                Some((e, r)) => (e.trim(), r.trim()),
                None => ("chromium", spec.trim()),
            };
            use super::engines::Engine;
            let engine = Engine::parse(engine)
                .ok_or_else(|| format!("unknown web engine `{engine}` (use chromium, webkit or firefox)"))?;
            match engine {
                Engine::Chromium if !host.chromium() => {
                    return Err("no Chromium-family browser is installed (install Chrome or Chromium, or set \
                                IDEALYST_CHROME to one)"
                        .into())
                }
                Engine::Firefox if !host.firefox() => {
                    return Err("Firefox isn't installed (install it, or set IDEALYST_FIREFOX to one)".into())
                }
                Engine::Webkit if host.os() != "macos" => {
                    return Err(format!(
                        "webkit runs as Safari, through safaridriver, on macOS only; this host is {}",
                        host.os()
                    ))
                }
                Engine::Webkit if !host.safaridriver() => {
                    return Err("safaridriver isn't installed (it ships with macOS: /usr/bin/safaridriver)".into())
                }
                _ => {}
            }
            let engine = (engine != Engine::Chromium).then_some(engine);
            if let Some(size) = parse_size(rest) {
                return Ok(Target { viewport: Some(size), engine, ..none });
            }
            match super::browser::device_preset(rest) {
                Ok(_) => Ok(Target { device: Some(rest.to_string()), engine, ..none }),
                Err(e) => Err(format!("{e:#}")),
            }
        }
        "macos" | "linux" | "windows" => {
            let host_ok = match platform {
                "macos" => host.os() == "macos",
                "linux" => host.os() == "linux",
                _ => host.os() == "windows" || host.windows_remote(),
            };
            if !host_ok {
                return Err(match platform {
                    "windows" => format!(
                        "a Windows app needs a Windows host, or IDEALYST_WINDOWS_REMOTE pointing at one \
                         (docs/windows.md); this host is {}",
                        host.os()
                    ),
                    p => format!("a {p} app runs only on a {p} host; this host is {}", host.os()),
                });
            }
            let size = parse_size(spec).ok_or_else(|| format!("`{spec}` is not a window size (WxH)"))?;
            Ok(Target { viewport: Some(size), ..none })
        }
        "ios" => {
            if host.os() != "macos" {
                return Err(format!("iOS simulators run only on a Mac; this host is {}", host.os()));
            }
            let (name, version) = match spec.split_once('@') {
                Some((n, v)) => (n.trim(), Some(v.trim().trim_start_matches("iOS").trim())),
                None => (spec.trim(), None),
            };
            let sims = host.simulators()?;
            let found = sims.iter().find(|s| {
                s.name == name
                    && version.is_none_or(|v| {
                        let have = s.runtime.trim_start_matches("iOS").trim();
                        have == v || have.starts_with(&format!("{v}."))
                    })
            });
            if let Some(s) = found {
                return Ok(Target { simulator: Some(s.udid.clone()), ..none });
            }
            // None to run on: make one from an installed runtime.
            let runtimes = host.ios_runtimes()?;
            let matching: Vec<&IosRuntime> =
                runtimes.iter().filter(|r| version.is_none_or(|v| version_matches(&r.version, v))).collect();
            if matching.is_empty() {
                let installed: Vec<&str> = runtimes.iter().map(|r| r.name.as_str()).collect();
                return Err(format!(
                    "the iOS {} simulator runtime isn't installed (installed: {}); add it in Xcode > \
                     Settings > Components, or `xcodebuild -downloadPlatform iOS`",
                    version.unwrap_or_default(),
                    if installed.is_empty() { "none".to_string() } else { installed.join(", ") }
                ));
            }
            let pick = matching
                .iter()
                .filter_map(|r| r.device_types.iter().find(|(n, _)| n == name).map(|(_, id)| (*r, id)))
                .max_by(|a, b| version_key(&a.0.version).cmp(&version_key(&b.0.version)));
            let Some((runtime, device_type)) = pick else {
                return Err(format!(
                    "no installed iOS{} runtime runs a device named \"{name}\" (`xcrun simctl list \
                     devicetypes` lists the names; a newer device needs a newer runtime)",
                    version.map(|v| format!(" {v}")).unwrap_or_default()
                ));
            };
            if !create {
                return Err(format!(
                    "no simulator named \"{name}\" exists ({} could run one; --no-create was given)",
                    runtime.name
                ));
            }
            Ok(Target {
                create: Some(Create::Simulator {
                    name: name.to_string(),
                    device_type: device_type.clone(),
                    runtime: runtime.identifier.clone(),
                    runtime_name: runtime.name.clone(),
                }),
                ..none
            })
        }
        "android" => {
            if let Some(serial) = spec.strip_prefix("serial:") {
                let serial = serial.trim();
                let attached = host.adb_serials()?;
                return if attached.iter().any(|s| s == serial) {
                    Ok(Target { emulator: Some(serial.to_string()), ..none })
                } else {
                    Err(format!("device {serial} is not attached (`adb devices` doesn't list it)"))
                };
            }
            let avds = host.avds()?;
            let host_abi = if host.arch() == "aarch64" { "arm64-v8a" } else { "x86_64" };
            let pick = match spec.split_once('@') {
                None => avds.iter().find(|a| a.name == spec.trim()),
                // Only the AVD this runner makes for the spec, never another
                // AVD that happens to share the profile: that one belongs to
                // someone (a kiosk or device-owner setup, an app's data), and
                // a suite would install into it. An existing AVD is used by
                // naming it.
                Some((device, api)) => {
                    let (device, api) = (device.trim(), api.trim());
                    let api: u32 = api.parse().map_err(|_| format!("`{api}` is not an Android API level"))?;
                    let name = avd_name(device, api);
                    avds.iter().find(|a| a.name == name)
                }
            };
            match pick {
                Some(avd) => match &avd.abi {
                    Some(abi) if abi != host_abi && !(abi == "x86" && host_abi == "x86_64") => Err(format!(
                        "AVD {} uses a {abi} system image, which can't run on this {} host; make one with \
                         an {host_abi} image",
                        avd.name,
                        host.arch()
                    )),
                    _ => Ok(Target { emulator: Some(avd.name.clone()), ..none }),
                },
                None => match spec.split_once('@') {
                    Some((device, api)) => {
                        let (device, api_text) = (device.trim(), api.trim());
                        let api: u32 = api_text.parse().map_err(|_| format!("`{api_text}` is not an Android API level"))?;
                        create_avd(host, device, api, host_abi, create)
                    }
                    None => Err(format!(
                        "no AVD named {} exists (`emulator -list-avds` lists {}); name a hardware profile \
                         and API level instead (`pixel_7 @ 34`) to have one made",
                        spec.trim(),
                        if avds.is_empty() {
                            "none".to_string()
                        } else {
                            avds.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                        }
                    )),
                },
            }
        }
        other => Err(format!("unknown platform `{other}`")),
    }
}

/// A missing `<profile> @ <api>` AVD: made from an installed system image
/// for this host's architecture, or why it can't be.
fn create_avd(host: &dyn Host, device: &str, api: u32, host_abi: &str, create: bool) -> Result<Target, String> {
    let images = host.android_images()?;
    // Google APIs first: what a framework app's Google Play services and
    // the stock emulator images expect.
    let rank = |tag: &str| match tag {
        "google_apis" => 0,
        "google_apis_playstore" => 1,
        "default" => 2,
        _ => 3,
    };
    let Some(image) = images.iter().filter(|i| i.api == api && i.abi == host_abi).min_by_key(|i| rank(&i.tag)) else {
        return Err(format!(
            "no API {api} system image for {host_abi} is installed; install one with `sdkmanager \
             \"system-images;android-{api};google_apis;{host_abi}\"`"
        ));
    };
    let profiles = host.android_profiles()?;
    if !profiles.iter().any(|p| p == device) {
        return Err(format!(
            "avdmanager has no hardware profile `{device}` (`avdmanager list device -c` lists them)"
        ));
    }
    let name = avd_name(device, api);
    let package = format!("system-images;android-{api};{};{}", image.tag, image.abi);
    if !create {
        return Err(format!("no AVD made from {device} with API {api} exists ({package} could make one; --no-create was given)"));
    }
    Ok(Target {
        create: Some(Create::Avd { name, package, profile: device.to_string() }),
        simulator: None,
        emulator: None,
        device: None,
        viewport: None,
        engine: None,
    })
}

/// The AVD a `<profile> @ <api>` spec runs on, made by the runner when
/// missing.
fn avd_name(profile: &str, api: u32) -> String {
    format!("idealyst_{profile}_{api}")
}

/// `18.3.1` → `[18, 3, 1]`, for ordering runtimes.
fn version_key(v: &str) -> Vec<u32> {
    v.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// Whether runtime version `have` (`18.3.1`) is the asked-for `want`
/// (`18`, `18.3`).
fn version_matches(have: &str, want: &str) -> bool {
    have == want || have.starts_with(&format!("{want}."))
}

/// The plan as the runner prints it before anything builds.
pub fn describe(rows: &[Row]) -> String {
    let mut out = String::from("[test] plan:\n");
    let width = rows.iter().map(|r| r.label().len()).max().unwrap_or(0);
    for row in rows {
        let label = row.label();
        match &row.plan {
            Ok(Target { create: Some(c), .. }) => {
                out.push_str(&format!("  run    {label:width$}  {}\n", c.describe()))
            }
            // The AVD or serial it runs on: a spec can name a profile, and
            // the plan says which device that is.
            Ok(Target { emulator: Some(on), .. }) if row.platform == "android" => {
                out.push_str(&format!("  run    {label:width$}  on {on}\n"))
            }
            Ok(_) => out.push_str(&format!("  run    {label}\n")),
            Err(why) => out.push_str(&format!("  skip   {label:width$}  {why}\n")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The real host
// ---------------------------------------------------------------------------

/// This machine, probed on demand.
pub struct ThisHost;

impl Host for ThisHost {
    fn os(&self) -> &str {
        std::env::consts::OS
    }

    fn arch(&self) -> &str {
        std::env::consts::ARCH
    }

    fn chromium(&self) -> bool {
        super::browser::headless_browser().is_some()
    }

    fn firefox(&self) -> bool {
        super::engines::firefox_binary().is_some()
    }

    fn safaridriver(&self) -> bool {
        super::engines::safaridriver().is_some()
    }

    fn simulators(&self) -> Result<Vec<Simulator>, String> {
        let out = std::process::Command::new("xcrun")
            .args(["simctl", "list", "devices", "available", "-j"])
            .output()
            .map_err(|_| "Xcode's command-line tools aren't installed (`xcode-select --install`)".to_string())?;
        if !out.status.success() {
            return Err(format!(
                "`xcrun simctl` failed (is Xcode installed and selected? `xcode-select -p`): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        parse_simctl(&out.stdout)
    }

    fn avds(&self) -> Result<Vec<Avd>, String> {
        let sdk = android_sdk().ok_or_else(|| {
            "the Android SDK isn't installed (set ANDROID_HOME, or install Android Studio)".to_string()
        })?;
        let emulator = sdk.join("emulator").join("emulator");
        let out = std::process::Command::new(&emulator)
            .arg("-list-avds")
            .output()
            .map_err(|_| format!("the Android emulator isn't installed ({} is missing; `sdkmanager emulator`)", emulator.display()))?;
        let home = std::env::var_os("ANDROID_AVD_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".android").join("avd")));
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("INFO"))
            .map(|name| {
                let config = home
                    .as_ref()
                    .and_then(|h| std::fs::read_to_string(h.join(format!("{name}.avd")).join("config.ini")).ok())
                    .unwrap_or_default();
                parse_avd(name, &config)
            })
            .collect())
    }

    fn adb_serials(&self) -> Result<Vec<String>, String> {
        let sdk = android_sdk().ok_or("the Android SDK isn't installed (set ANDROID_HOME)")?;
        let out = std::process::Command::new(sdk.join("platform-tools").join("adb"))
            .arg("devices")
            .output()
            .map_err(|_| "adb isn't installed (`sdkmanager platform-tools`)".to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once('\t'))
            .filter(|(_, state)| state.trim() == "device")
            .map(|(serial, _)| serial.to_string())
            .collect())
    }

    fn windows_remote(&self) -> bool {
        std::env::var_os("IDEALYST_WINDOWS_REMOTE").is_some()
    }

    fn ios_runtimes(&self) -> Result<Vec<IosRuntime>, String> {
        let out = std::process::Command::new("xcrun")
            .args(["simctl", "list", "runtimes", "-j"])
            .output()
            .map_err(|_| "Xcode's command-line tools aren't installed (`xcode-select --install`)".to_string())?;
        if !out.status.success() {
            return Err(format!("`xcrun simctl list runtimes` failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        parse_runtimes(&out.stdout)
    }

    fn android_images(&self) -> Result<Vec<SystemImage>, String> {
        let sdk = android_sdk().ok_or("the Android SDK isn't installed (set ANDROID_HOME, or install Android Studio)")?;
        Ok(scan_system_images(&sdk.join("system-images")))
    }

    fn android_profiles(&self) -> Result<Vec<String>, String> {
        let sdk = android_sdk().ok_or("the Android SDK isn't installed (set ANDROID_HOME)")?;
        let avdmanager = avdmanager(&sdk).ok_or(
            "avdmanager isn't installed (`sdkmanager \"cmdline-tools;latest\"`, or Android Studio > SDK Manager > \
             SDK Tools > Android SDK Command-line Tools)",
        )?;
        let out = std::process::Command::new(&avdmanager)
            .args(["list", "device", "-c"])
            .output()
            .map_err(|e| format!("running {}: {e}", avdmanager.display()))?;
        if !out.status.success() {
            return Err(format!(
                "`avdmanager list device` failed (it needs a Java runtime): {}",
                String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
    }
}

/// `avdmanager` in the SDK: the `cmdline-tools` package (`latest`, else any
/// version), else the old `tools`.
pub fn avdmanager(sdk: &Path) -> Option<std::path::PathBuf> {
    let latest = sdk.join("cmdline-tools/latest/bin/avdmanager");
    if latest.is_file() {
        return Some(latest);
    }
    let any = std::fs::read_dir(sdk.join("cmdline-tools"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin/avdmanager"))
        .find(|p| p.is_file());
    any.or_else(|| Some(sdk.join("tools/bin/avdmanager")).filter(|p| p.is_file()))
}

/// The Android SDK root (see [`android_sdk`]), for the provisioning steps.
pub fn sdk_root() -> Option<std::path::PathBuf> {
    android_sdk()
}

/// `xcrun simctl list runtimes -j` → the available iOS runtimes.
pub fn parse_runtimes(json: &[u8]) -> Result<Vec<IosRuntime>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("reading simctl's runtimes: {e}"))?;
    Ok(v["runtimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["platform"] == "iOS" && r["isAvailable"] != false)
        .filter_map(|r| {
            Some(IosRuntime {
                name: r["name"].as_str()?.to_string(),
                version: r["version"].as_str()?.to_string(),
                identifier: r["identifier"].as_str()?.to_string(),
                device_types: r["supportedDeviceTypes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| Some((d["name"].as_str()?.to_string(), d["identifier"].as_str()?.to_string())))
                    .collect(),
            })
        })
        .collect())
}

/// `<sdk>/system-images/android-<api>/<tag>/<abi>/` → the installed images.
pub fn scan_system_images(dir: &Path) -> Vec<SystemImage> {
    let mut out = Vec::new();
    let dirs = |d: &Path| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(d).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
    };
    for api_dir in dirs(dir) {
        let name = api_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // `android-34`, `android-37.0`: the level is the leading digits.
        let Some(level) = name.strip_prefix("android-") else { continue };
        let digits: String = level.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(api) = digits.parse() else { continue };
        for tag_dir in dirs(&api_dir) {
            let tag = tag_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for abi_dir in dirs(&tag_dir) {
                let abi = abi_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(SystemImage { api, tag: tag.clone(), abi });
            }
        }
    }
    out
}

/// The Android SDK root: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, or the default
/// Android Studio location.
fn android_sdk() -> Option<std::path::PathBuf> {
    let from_env = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .map(std::path::PathBuf::from)
        .find(|p| p.is_dir());
    from_env.or_else(|| {
        let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
        [home.join("Library/Android/sdk"), home.join("Android/Sdk")].into_iter().find(|p| p.is_dir())
    })
}

/// `xcrun simctl list devices available -j` → the iOS simulators.
pub fn parse_simctl(json: &[u8]) -> Result<Vec<Simulator>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("reading simctl's list: {e}"))?;
    let mut out = Vec::new();
    for (runtime, devices) in v["devices"].as_object().into_iter().flatten() {
        // `com.apple.CoreSimulator.SimRuntime.iOS-17-5` → `iOS 17.5`.
        let Some(tail) = runtime.rsplit('.').next() else { continue };
        let Some(version) = tail.strip_prefix("iOS-") else { continue };
        let runtime = format!("iOS {}", version.replace('-', "."));
        for d in devices.as_array().into_iter().flatten() {
            if d["isAvailable"].as_bool() == Some(false) {
                continue;
            }
            if let (Some(name), Some(udid)) = (d["name"].as_str(), d["udid"].as_str()) {
                out.push(Simulator { name: name.into(), udid: udid.into(), runtime: runtime.clone() });
            }
        }
    }
    Ok(out)
}

/// An AVD's `config.ini` → its hardware profile, API level and ABI.
pub fn parse_avd(name: &str, config: &str) -> Avd {
    let get = |key: &str| {
        config.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    // `image.sysdir.1=system-images/android-34/google_apis/arm64-v8a/`
    let sysdir = get("image.sysdir.1").unwrap_or_default();
    let api = sysdir.split('/').find_map(|p| p.strip_prefix("android-")?.parse().ok());
    Avd { name: name.into(), device: get("hw.device.name"), api, abi: get("abi.type") }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHost {
        os: &'static str,
        arch: &'static str,
        chromium: bool,
        sims: Result<Vec<Simulator>, String>,
        avds: Result<Vec<Avd>, String>,
    }

    impl Host for FakeHost {
        fn os(&self) -> &str {
            self.os
        }
        fn arch(&self) -> &str {
            self.arch
        }
        fn chromium(&self) -> bool {
            self.chromium
        }
        fn firefox(&self) -> bool {
            self.chromium
        }
        fn safaridriver(&self) -> bool {
            self.os == "macos"
        }
        fn simulators(&self) -> Result<Vec<Simulator>, String> {
            self.sims.clone()
        }
        fn avds(&self) -> Result<Vec<Avd>, String> {
            self.avds.clone()
        }
        fn adb_serials(&self) -> Result<Vec<String>, String> {
            Ok(vec!["emulator-5554".into()])
        }
        fn windows_remote(&self) -> bool {
            false
        }
        fn ios_runtimes(&self) -> Result<Vec<IosRuntime>, String> {
            Ok(vec![
                IosRuntime {
                    name: "iOS 17.5".into(),
                    version: "17.5".into(),
                    identifier: "rt.iOS-17-5".into(),
                    device_types: vec![
                        ("iPhone 15".into(), "dt.iPhone-15".into()),
                        ("iPhone SE (3rd generation)".into(), "dt.iPhone-SE-3".into()),
                    ],
                },
                IosRuntime {
                    name: "iOS 17.0".into(),
                    version: "17.0".into(),
                    identifier: "rt.iOS-17-0".into(),
                    device_types: vec![("iPhone SE (3rd generation)".into(), "dt.iPhone-SE-3".into())],
                },
            ])
        }
        fn android_images(&self) -> Result<Vec<SystemImage>, String> {
            Ok(vec![
                SystemImage { api: 34, tag: "google_apis_playstore".into(), abi: "arm64-v8a".into() },
                SystemImage { api: 34, tag: "google_apis".into(), abi: "arm64-v8a".into() },
                SystemImage { api: 30, tag: "google_apis".into(), abi: "x86_64".into() },
            ])
        }
        fn android_profiles(&self) -> Result<Vec<String>, String> {
            Ok(vec!["pixel_7".into(), "pixel_tablet".into()])
        }
    }

    fn mac() -> FakeHost {
        FakeHost {
            os: "macos",
            arch: "aarch64",
            chromium: true,
            sims: Ok(vec![Simulator { name: "iPhone 15".into(), udid: "U-15".into(), runtime: "iOS 17.5".into() }]),
            avds: Ok(vec![
                Avd { name: "idealyst_pixel_7_34".into(), device: Some("pixel_7".into()), api: Some(34), abi: Some("arm64-v8a".into()) },
                Avd { name: "Kiosk_Tablet".into(), device: Some("pixel_tablet".into()), api: Some(34), abi: Some("arm64-v8a".into()) },
                Avd { name: "Old_x86".into(), device: Some("pixel_4".into()), api: Some(30), abi: Some("x86_64".into()) },
            ]),
        }
    }

    const MANIFEST: &str = r#"
        [package]
        name = "app"
        [package.metadata.idealyst.app]
        targets = ["web", "ios", "android", "macos"]
        [package.metadata.idealyst.test.devices]
        phone = { web = "chromium:iphone-15", ios = "iPhone 15 @ iOS 17", android = "pixel_7 @ 34" }
        tablet = { web = "webkit:ipad-air", ios = "iPad Air @ iOS 18", android = "Old_x86" }
        desktop = { web = "1440x900", macos = "1440x900", windows = "1440x900", linux = "1440x900" }
    "#;

    fn rows(host: &dyn Host, filter: Filter) -> Vec<Row> {
        let p = parse(MANIFEST).unwrap();
        plan(p.matrix.as_ref().unwrap(), p.targets.as_deref(), &filter, host).unwrap()
    }

    fn row<'a>(rows: &'a [Row], class: &str, platform: &str) -> &'a Row {
        rows.iter().find(|r| r.class == class && r.platform == platform).unwrap()
    }

    /// On a Mac with Chrome, an iPhone 15 simulator and an arm64 Pixel AVD,
    /// the phone row runs everywhere; every other row says why it can't.
    #[test]
    fn the_plan_runs_what_this_host_can_and_says_why_for_the_rest() {
        let rows = rows(&mac(), Filter::default());
        let phone = |p| row(&rows, "phone", p).plan.clone();
        assert_eq!(phone("web").unwrap().device.as_deref(), Some("iphone-15"));
        assert_eq!(phone("ios").unwrap().simulator.as_deref(), Some("U-15"), "the simulator, by UDID");
        assert_eq!(phone("android").unwrap().emulator.as_deref(), Some("idealyst_pixel_7_34"));

        let why = |c, p| row(&rows, c, p).plan.clone().unwrap_err();
        assert_eq!(row(&rows, "tablet", "web").plan.clone().unwrap().engine, Some(crate::cmd::test::engines::Engine::Webkit), "Safari on a Mac");
        assert!(why("tablet", "ios").contains("iOS 18 simulator runtime isn't installed (installed: iOS 17.5, iOS 17.0)"));
        assert!(why("tablet", "android").contains("x86_64 system image, which can't run on this aarch64 host"));
        assert!(why("desktop", "windows").contains("doesn't build for windows"), "targets come first");
        assert!(why("desktop", "linux").contains("doesn't build for linux"));
        assert_eq!(row(&rows, "desktop", "macos").plan.clone().unwrap().viewport, Some((1440, 900)));
        let shown = describe(&rows);
        assert!(shown.contains("  run    phone on ios (iPhone 15 @ iOS 17)"), "{shown}");
        assert!(shown.contains("  skip   tablet on ios"), "{shown}");
        assert!(shown.contains("phone on android (pixel_7 @ 34)") && shown.contains("on idealyst_pixel_7_34"), "{shown}");
    }

    /// A `<profile> @ <api>` spec never runs on an AVD it didn't make: the
    /// first matching AVD on a dev machine was a CrewForge kiosk tablet's,
    /// and the suite would have installed into it. It gets its own
    /// `idealyst_<profile>_<api>`; an existing AVD runs when named.
    #[test]
    fn regression_a_profile_spec_does_not_take_someone_elses_avd() {
        let host = mac();
        let t = resolve("android", "pixel_tablet @ 34", &host, true).unwrap();
        assert_eq!(t.emulator, None, "Kiosk_Tablet shares the profile and is not used");
        assert!(matches!(t.create, Some(Create::Avd { ref name, .. }) if name == "idealyst_pixel_tablet_34"), "{t:?}");
        let e = resolve("android", "pixel_tablet @ 34", &host, false).unwrap_err();
        assert!(e.contains("--no-create"), "{e}");
        assert_eq!(resolve("android", "Kiosk_Tablet", &host, true).unwrap().emulator.as_deref(), Some("Kiosk_Tablet"), "by name");
    }

    #[test]
    fn a_linux_host_without_tools_says_what_is_missing() {
        let host = FakeHost {
            os: "linux",
            arch: "x86_64",
            chromium: false,
            sims: Err("unused".into()),
            avds: Err("the Android SDK isn't installed (set ANDROID_HOME, or install Android Studio)".into()),
        };
        let rows = rows(&host, Filter::default());
        let why = |c, p| row(&rows, c, p).plan.clone().unwrap_err();
        assert!(why("phone", "web").contains("no Chromium-family browser"));
        assert!(why("tablet", "web").contains("webkit runs as Safari, through safaridriver, on macOS only; this host is linux"));
        assert!(why("phone", "ios").contains("only on a Mac; this host is linux"));
        assert!(why("phone", "android").contains("Android SDK isn't installed"));
        assert!(why("desktop", "macos").contains("only on a macos host; this host is linux"));
    }

    #[test]
    fn the_filter_keeps_the_classes_and_platforms_asked_for() {
        let rows = rows(&mac(), Filter { classes: vec!["phone".into()], platforms: vec!["ios".into(), "web".into()], no_create: false });
        let got: Vec<(&str, &str)> = rows.iter().map(|r| (r.class.as_str(), r.platform.as_str())).collect();
        assert_eq!(got, [("phone", "web"), ("phone", "ios")]);
        let p = parse(MANIFEST).unwrap();
        let err = plan(p.matrix.as_ref().unwrap(), None, &Filter { classes: vec!["watch".into()], ..Default::default() }, &mac())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no class `watch`") && err.contains("phone, tablet, desktop"), "{err}");
    }

    /// A missing simulator or AVD that can be made from what's installed is
    /// planned as a run that creates it first (the newest runtime that runs
    /// the device; the Google APIs image for this host); `--no-create`
    /// reports it instead. What can't be made says what to install.
    #[test]
    fn missing_devices_are_created_from_what_is_installed_or_say_what_to_install() {
        let host = mac();
        let t = resolve("ios", "iPhone SE (3rd generation) @ iOS 17", &host, true).unwrap();
        assert_eq!(
            t.create,
            Some(Create::Simulator {
                name: "iPhone SE (3rd generation)".into(),
                device_type: "dt.iPhone-SE-3".into(),
                runtime: "rt.iOS-17-5".into(),
                runtime_name: "iOS 17.5".into(),
            }),
            "the newest iOS 17 runtime that runs it"
        );
        let e = resolve("ios", "iPhone SE (3rd generation) @ iOS 17", &host, false).unwrap_err();
        assert!(e.contains("--no-create was given"), "{e}");
        let e = resolve("ios", "iPhone 99 @ iOS 17", &host, true).unwrap_err();
        assert!(e.contains("no installed iOS 17 runtime runs a device named \"iPhone 99\""), "{e}");

        let t = resolve("android", "pixel_tablet @ 34", &host, true).unwrap();
        assert_eq!(
            t.create,
            Some(Create::Avd {
                name: "idealyst_pixel_tablet_34".into(),
                package: "system-images;android-34;google_apis;arm64-v8a".into(),
                profile: "pixel_tablet".into(),
            })
        );
        let e = resolve("android", "pixel_7 @ 30", &host, true).unwrap_err();
        assert!(e.contains("no API 30 system image for arm64-v8a") && e.contains("sdkmanager"), "an x86_64 image doesn't count: {e}");
        let e = resolve("android", "nexus_1 @ 34", &host, true).unwrap_err();
        assert!(e.contains("no hardware profile `nexus_1`"), "{e}");
        let e = resolve("android", "serial:HA2HC863", &host, true).unwrap_err();
        assert!(e.contains("HA2HC863 is not attached"), "{e}");
        assert!(resolve("web", "chromium:galaxy-fold-9", &host, true).is_err(), "an unknown preset");
        assert!(resolve("macos", "big", &host, true).unwrap_err().contains("not a window size"));
        let rows = vec![Row {
            class: "phone".into(),
            platform: "ios".into(),
            spec: "iPhone SE (3rd generation) @ iOS 17".into(),
            plan: resolve("ios", "iPhone SE (3rd generation) @ iOS 17", &host, true),
        }];
        assert!(describe(&rows).contains("creates simulator \"iPhone SE (3rd generation)\" on iOS 17.5 first"));
    }

    #[test]
    fn runtimes_and_system_images_are_read() {
        let json = br#"{"runtimes":[
            {"name":"iOS 26.5","version":"26.5","identifier":"rt26","platform":"iOS","isAvailable":true,
             "supportedDeviceTypes":[{"name":"iPhone 17","identifier":"dt17"}]},
            {"name":"watchOS 11","version":"11.0","identifier":"w","platform":"watchOS","isAvailable":true,"supportedDeviceTypes":[]}]}"#;
        let r = parse_runtimes(json).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].device_types, [("iPhone 17".to_string(), "dt17".to_string())]);

        let dir = tempfile::tempdir().unwrap();
        for p in ["android-34/google_apis/arm64-v8a", "android-37.0/google_apis_playstore/arm64-v8a", "notes"] {
            std::fs::create_dir_all(dir.path().join(p)).unwrap();
        }
        let mut images = scan_system_images(dir.path());
        images.sort_by_key(|i| i.api);
        assert_eq!(
            images,
            [
                SystemImage { api: 34, tag: "google_apis".into(), abi: "arm64-v8a".into() },
                SystemImage { api: 37, tag: "google_apis_playstore".into(), abi: "arm64-v8a".into() },
            ]
        );
    }

    #[test]
    fn manifests_are_read_and_checked() {
        let p = parse("[package]\nname = \"x\"\n").unwrap();
        assert!(p.matrix.is_none() && p.targets.is_none());
        let e = parse("[package]\nname=\"x\"\n[package.metadata.idealyst.test.devices]\nphone = { tvos = \"x\" }\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown platform `tvos`"), "{e}");
    }

    #[test]
    fn simctl_and_avd_configs_parse() {
        let json = br#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5":[{"name":"iPhone 15","udid":"A","isAvailable":true}],
            "com.apple.CoreSimulator.SimRuntime.watchOS-10-0":[{"name":"Watch","udid":"W","isAvailable":true}]}}"#;
        assert_eq!(
            parse_simctl(json).unwrap(),
            [Simulator { name: "iPhone 15".into(), udid: "A".into(), runtime: "iOS 17.5".into() }]
        );
        let avd = parse_avd(
            "P7",
            "abi.type=arm64-v8a\nhw.device.name=pixel_7\nimage.sysdir.1=system-images/android-34/google_apis/arm64-v8a/\n",
        );
        assert_eq!((avd.device.as_deref(), avd.api, avd.abi.as_deref()), (Some("pixel_7"), Some(34), Some("arm64-v8a")));
    }
}
