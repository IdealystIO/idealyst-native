//! `idealyst doctor` — diagnose the local toolchain per target.
//!
//! IMPLEMENTED: the check registry + the Core, Web, iOS, and Android checks.
//! The per-platform checklists below are the spec for the groups still to
//! come; each line names the build/run crate the requirement actually comes
//! from, so this list stays honest.
//!
//! Pending groups (not yet probed):
//! - Roku — `ROKU_DEV_IP`, `ROKU_DEV_PASSWORD` env vars.
//! - Hot-reload (macOS) — see `crates/tools/build/runtime-server`
//!   (`hotpatch/link.rs`): `ld`, `xcrun` for hotpatch linking.
//!
//! ## The anti-drift rule (do this when adding the groups above)
//!
//! Each build/run crate should EXPORT its requirements as data
//! (`pub fn requirements() -> &'static [Requirement]`) and `doctor` should
//! CONSUME them, rather than keeping a second hand-written copy that drifts.
//! Then doctor can't claim a tool the builder doesn't use, or miss one the
//! builder adds: one source of truth, checked by the thing that builds AND
//! the thing that diagnoses. Same principle as compiled recipes. The Core +
//! Web checks below are inlined for now because the slice is small; fold
//! them into the export form when the build crates grow `requirements()`.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::platform::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Limit checks to one platform's toolchain (the always-on core checks
    /// still run). Omit to report every implemented group.
    #[arg(value_enum)]
    target: Option<Platform>,

    /// Emit machine-readable JSON instead of the grouped report.
    #[arg(long)]
    json: bool,
}

// =============================================================================
// Model.
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Category {
    Core,
    Web,
    Ios,
    Android,
    Roku,
}

impl Category {
    fn title(self) -> &'static str {
        match self {
            Category::Core => "Core",
            Category::Web => "Web",
            Category::Ios => "iOS",
            Category::Android => "Android",
            Category::Roku => "Roku",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Category::Core => "core",
            Category::Web => "web",
            Category::Ios => "ios",
            Category::Android => "android",
            Category::Roku => "roku",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Required,
    Recommended,
    Optional,
}

impl Level {
    fn key(self) -> &'static str {
        match self {
            Level::Required => "required",
            Level::Recommended => "recommended",
            Level::Optional => "optional",
        }
    }
}

/// The result of probing one check.
enum Outcome {
    /// Present. Carries a version / detail line when one could be read.
    Ok(Option<String>),
    /// Not installed / not on PATH / env var unset.
    Missing,
    /// Found but incomplete/incompatible (e.g. a build-tools dir that's
    /// missing `d8`). Counts as a failure like `Missing`; the string says why.
    Wrong(String),
    /// Couldn't determine (e.g. `rustup` itself absent, so installed
    /// targets can't be listed). Never counts as a failure.
    Unknown(String),
}

struct Check {
    id: &'static str,
    category: Category,
    level: Level,
    /// One-line remediation, shown after a failing/▵ check.
    fix: &'static str,
    probe: fn() -> Outcome,
}

// =============================================================================
// The registry — Core + Web today; the groups in the module doc to follow.
// =============================================================================

fn checks() -> Vec<Check> {
    vec![
        // ---- Core: every build shells `cargo`; the framework is git-fetched.
        Check {
            id: "rustc",
            category: Category::Core,
            level: Level::Required,
            fix: "Install Rust via https://rustup.rs",
            probe: || bin_version("rustc", &["--version"]),
        },
        Check {
            id: "cargo",
            category: Category::Core,
            level: Level::Required,
            fix: "Install Rust via https://rustup.rs",
            probe: || bin_version("cargo", &["--version"]),
        },
        Check {
            id: "git",
            category: Category::Core,
            level: Level::Required,
            fix: "Install git from https://git-scm.com",
            probe: || bin_version("git", &["--version"]),
        },
        // ---- Web: see crates/tools/build/web.
        Check {
            id: "wasm32-unknown-unknown target",
            category: Category::Web,
            level: Level::Required,
            fix: "rustup target add wasm32-unknown-unknown",
            probe: || rustup_target("wasm32-unknown-unknown"),
        },
        Check {
            id: "wasm-bindgen",
            category: Category::Web,
            level: Level::Required,
            fix: "cargo install wasm-bindgen-cli",
            probe: || bin_version("wasm-bindgen", &["--version"]),
        },
        Check {
            id: "wasm-opt",
            category: Category::Web,
            level: Level::Recommended,
            fix: "Install binaryen (brew install binaryen / apt install binaryen) — release builds only",
            probe: || bin_version("wasm-opt", &["--version"]),
        },
        Check {
            id: "nightly toolchain",
            category: Category::Web,
            level: Level::Optional,
            fix: "rustup toolchain install nightly && rustup component add rust-src --toolchain nightly — only for --strip-panics builds",
            probe: || rustup_toolchain("nightly"),
        },
        // ---- iOS: see crates/tools/run/ios (lib.rs simulator + device.rs).
        // NB: device.rs generates the pbxproj from a template and does NOT
        // depend on `xcodegen`, so it's deliberately not a requirement here.
        Check {
            id: "macOS host",
            category: Category::Ios,
            level: Level::Required,
            fix: "iOS builds require macOS",
            probe: macos_host,
        },
        Check {
            id: "xcrun",
            category: Category::Ios,
            level: Level::Required,
            fix: "Install Xcode + Command Line Tools (xcode-select --install)",
            probe: || bin_version("xcrun", &["--version"]),
        },
        Check {
            id: "xcodebuild",
            category: Category::Ios,
            level: Level::Required,
            fix: "Install the full Xcode app, then: sudo xcode-select -s /Applications/Xcode.app",
            probe: || bin_ok("xcodebuild", &["-version"]),
        },
        Check {
            id: "aarch64-apple-ios target",
            category: Category::Ios,
            level: Level::Required,
            fix: "rustup target add aarch64-apple-ios",
            probe: || rustup_target("aarch64-apple-ios"),
        },
        Check {
            id: "aarch64-apple-ios-sim target",
            category: Category::Ios,
            level: Level::Required,
            fix: "rustup target add aarch64-apple-ios-sim — Apple-Silicon simulator",
            probe: || rustup_target("aarch64-apple-ios-sim"),
        },
        Check {
            id: "x86_64-apple-ios target",
            category: Category::Ios,
            level: Level::Recommended,
            fix: "rustup target add x86_64-apple-ios — Intel simulator only",
            probe: || rustup_target("x86_64-apple-ios"),
        },
        Check {
            id: "ios-deploy",
            category: Category::Ios,
            level: Level::Recommended,
            fix: "brew install ios-deploy — needed only for physical-device installs",
            probe: || bin_version("ios-deploy", &["--version"]),
        },
        // ---- Android: see crates/tools/build/android + crates/tools/run/android.
        Check {
            id: "ANDROID_NDK_HOME",
            category: Category::Android,
            level: Level::Required,
            fix: "Set ANDROID_NDK_HOME to your NDK (e.g. ~/Library/Android/sdk/ndk/<version>)",
            probe: android_ndk,
        },
        Check {
            id: "aarch64-linux-android target",
            category: Category::Android,
            level: Level::Required,
            fix: "rustup target add aarch64-linux-android",
            probe: || rustup_target("aarch64-linux-android"),
        },
        Check {
            id: "Android SDK",
            category: Category::Android,
            level: Level::Required,
            fix: "Set ANDROID_HOME, or install the SDK via Android Studio",
            probe: android_sdk,
        },
        Check {
            id: "SDK build-tools",
            category: Category::Android,
            level: Level::Required,
            fix: "Install via: sdkmanager 'build-tools;36.0.0'",
            probe: android_build_tools,
        },
        Check {
            id: "SDK platform (android.jar)",
            category: Category::Android,
            level: Level::Required,
            fix: "Install via: sdkmanager 'platforms;android-36'",
            probe: android_platform,
        },
        Check {
            id: "adb",
            category: Category::Android,
            level: Level::Required,
            fix: "Install the SDK platform-tools (sdkmanager platform-tools)",
            probe: android_adb,
        },
        Check {
            id: "javac (JDK)",
            category: Category::Android,
            level: Level::Required,
            fix: "Install a JDK (e.g. Temurin 21) and put javac on PATH",
            probe: || bin_version("javac", &["-version"]),
        },
    ]
}

// =============================================================================
// Probes.
// =============================================================================

/// First non-empty trimmed line of `bytes`.
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
}

/// Run `bin <args>` and report whether it exists. A binary that runs at
/// all (even nonzero exit) counts as present; `NotFound` is `Missing`.
fn bin_version(bin: &str, args: &[&str]) -> Outcome {
    match Command::new(bin).args(args).output() {
        Ok(out) => Outcome::Ok(first_line(&out.stdout).or_else(|| first_line(&out.stderr))),
        Err(e) if e.kind() == ErrorKind::NotFound => Outcome::Missing,
        Err(e) => Outcome::Unknown(e.to_string()),
    }
}

/// Like [`bin_version`] but a NONZERO exit also counts as `Missing`. Used
/// for `xcodebuild`, whose shim exists with the Command Line Tools alone
/// but exits with "tool 'xcodebuild' requires Xcode" until the full Xcode
/// app is installed — exactly the state the iOS build can't use.
fn bin_ok(bin: &str, args: &[&str]) -> Outcome {
    match Command::new(bin).args(args).output() {
        Ok(out) if out.status.success() => {
            Outcome::Ok(first_line(&out.stdout).or_else(|| first_line(&out.stderr)))
        }
        Ok(_) => Outcome::Missing,
        Err(e) if e.kind() == ErrorKind::NotFound => Outcome::Missing,
        Err(e) => Outcome::Unknown(e.to_string()),
    }
}

/// Is the host macOS? iOS builds require it.
fn macos_host() -> Outcome {
    if cfg!(target_os = "macos") {
        Outcome::Ok(Some("macOS host".to_string()))
    } else {
        Outcome::Missing
    }
}

// ---- Android SDK/NDK resolution — mirrors crates/tools/run/android. ----

fn user_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Locate the Android SDK the same way `run-android` does: `ANDROID_HOME`,
/// then `ANDROID_SDK_ROOT`, then the per-OS default install paths.
fn find_android_sdk() -> Option<PathBuf> {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(h) = std::env::var(var) {
            let p = PathBuf::from(h);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    let home = user_home()?;
    [
        home.join("Library/Android/sdk"),  // macOS default
        home.join("Android/Sdk"),          // Linux default
        home.join("AppData/Local/Android/Sdk"), // Windows-style
    ]
    .into_iter()
    .find(|p| p.is_dir())
}

/// Highest-named immediate subdirectory of `dir` (e.g. latest build-tools).
fn latest_subdir(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if best.as_ref().map_or(true, |(b, _)| name > *b) {
                best = Some((name, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Does `dir` hold an executable named `name` (allowing `.bat`/`.exe`)?
fn tool_in_dir(dir: &Path, name: &str) -> bool {
    [name.to_string(), format!("{name}.bat"), format!("{name}.exe")]
        .iter()
        .any(|n| dir.join(n).is_file())
}

fn android_ndk() -> Outcome {
    match std::env::var("ANDROID_NDK_HOME") {
        Ok(h) if PathBuf::from(&h).is_dir() => Outcome::Ok(Some(h)),
        Ok(h) => Outcome::Wrong(format!("ANDROID_NDK_HOME={h} is not a directory")),
        Err(_) => Outcome::Missing,
    }
}

fn android_sdk() -> Outcome {
    match find_android_sdk() {
        Some(p) => Outcome::Ok(Some(p.display().to_string())),
        None => Outcome::Missing,
    }
}

fn android_build_tools() -> Outcome {
    let Some(sdk) = find_android_sdk() else {
        return Outcome::Unknown("Android SDK not found".to_string());
    };
    let Some(bt) = latest_subdir(&sdk.join("build-tools")) else {
        return Outcome::Missing;
    };
    let version = bt.file_name().map(|n| n.to_string_lossy().to_string());
    let missing: Vec<&str> = ["aapt2", "apksigner", "d8", "zipalign"]
        .into_iter()
        .filter(|t| !tool_in_dir(&bt, t))
        .collect();
    if missing.is_empty() {
        Outcome::Ok(version)
    } else {
        Outcome::Wrong(format!(
            "build-tools {} is missing {}",
            version.unwrap_or_default(),
            missing.join(", ")
        ))
    }
}

fn android_platform() -> Outcome {
    let Some(sdk) = find_android_sdk() else {
        return Outcome::Unknown("Android SDK not found".to_string());
    };
    let has_jar = std::fs::read_dir(sdk.join("platforms"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().join("android.jar").is_file());
    if has_jar {
        Outcome::Ok(None)
    } else {
        Outcome::Missing
    }
}

fn android_adb() -> Outcome {
    if let Some(sdk) = find_android_sdk() {
        if tool_in_dir(&sdk.join("platform-tools"), "adb") {
            return Outcome::Ok(None);
        }
    }
    // Fall back to a PATH-installed adb.
    bin_version("adb", &["version"])
}

/// Is `triple` an installed rustup target? `Unknown` if rustup isn't
/// present (the target may still be installed another way — we just
/// can't tell, so we don't fail the user for it).
fn rustup_target(triple: &str) -> Outcome {
    match Command::new("rustup").args(["target", "list", "--installed"]).output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.lines().any(|l| l.trim() == triple) {
                Outcome::Ok(None)
            } else {
                Outcome::Missing
            }
        }
        Ok(out) => Outcome::Unknown(
            first_line(&out.stderr).unwrap_or_else(|| "rustup returned an error".to_string()),
        ),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Outcome::Unknown("rustup not found — can't verify installed targets".to_string())
        }
        Err(e) => Outcome::Unknown(e.to_string()),
    }
}

/// Is a toolchain whose name contains `needle` (e.g. "nightly") installed?
fn rustup_toolchain(needle: &str) -> Outcome {
    match Command::new("rustup").args(["toolchain", "list"]).output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.lines().any(|l| l.contains(needle)) {
                Outcome::Ok(None)
            } else {
                Outcome::Missing
            }
        }
        Ok(out) => Outcome::Unknown(
            first_line(&out.stderr).unwrap_or_else(|| "rustup returned an error".to_string()),
        ),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Outcome::Unknown("rustup not found — can't list toolchains".to_string())
        }
        Err(e) => Outcome::Unknown(e.to_string()),
    }
}

// =============================================================================
// Run.
// =============================================================================

/// Which check category a platform argument scopes to. The host-side
/// preview targets (sim / macos / terminal / runtime-server) need nothing
/// beyond the core toolchain, so they map to `None`.
fn platform_category(p: Platform) -> Option<Category> {
    match p {
        Platform::Web => Some(Category::Web),
        Platform::Ios => Some(Category::Ios),
        Platform::Android => Some(Category::Android),
        Platform::Roku => Some(Category::Roku),
        Platform::Sim | Platform::Macos | Platform::Terminal | Platform::RuntimeServer => None,
    }
}

/// Which category groups to display for a given `--target`. With no target,
/// every implemented group; with a platform target, Core plus that
/// platform's group (or Core alone for the host-side preview targets).
fn categories_to_show(target: Option<Platform>) -> Vec<Category> {
    match target {
        None => vec![Category::Core, Category::Web, Category::Ios, Category::Android],
        Some(p) => match platform_category(p) {
            Some(cat) => vec![Category::Core, cat],
            None => vec![Category::Core],
        },
    }
}

/// Whether a failing `Required` check in `cat` should fail the run. Core is
/// always enforced; a platform group is enforced only when the user asked
/// for it — so a bare `doctor` reports other platforms without blocking on
/// toolchains the dev hasn't installed.
fn is_enforced(cat: Category, scope_cat: Option<Category>) -> bool {
    cat == Category::Core || scope_cat == Some(cat)
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let scope_cat = args.target.and_then(platform_category);
    let show = categories_to_show(args.target);

    let all = checks();
    let mut failures = 0usize;
    let mut json_items: Vec<serde_json::Value> = Vec::new();

    if !args.json {
        println!("idealyst doctor\n");
    }

    for &cat in &show {
        let group: Vec<&Check> = all.iter().filter(|c| c.category == cat).collect();

        if group.is_empty() {
            // A requested platform whose checks aren't written yet.
            if !args.json {
                println!("{}\n  (checks not implemented yet)\n", cat.title());
            }
            continue;
        }

        if !args.json {
            println!("{}", cat.title());
        }

        let enforced = is_enforced(cat, scope_cat);
        for chk in group {
            let outcome = (chk.probe)();
            let is_problem = matches!(outcome, Outcome::Missing | Outcome::Wrong(_));
            if is_problem && enforced && chk.level == Level::Required {
                failures += 1;
            }

            if args.json {
                json_items.push(serde_json::json!({
                    "id": chk.id,
                    "category": chk.category.key(),
                    "level": chk.level.key(),
                    "status": status_key(&outcome),
                    "detail": detail(&outcome),
                    "fix": chk.fix,
                }));
            } else {
                print_line(chk, &outcome);
            }
        }
        if !args.json {
            println!();
        }
    }

    if let Some(p) = args.target {
        if scope_cat.is_none() && !args.json {
            println!("{p} needs no toolchain beyond the core checks.\n");
        }
    } else if !args.json {
        println!("Roku toolchain checks are not implemented yet.\n");
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&json_items)?);
    }

    if failures == 0 {
        if !args.json {
            println!("All required checks passed.");
        }
        Ok(())
    } else {
        anyhow::bail!("{failures} required check(s) failed — see above")
    }
}

fn status_key(o: &Outcome) -> &'static str {
    match o {
        Outcome::Ok(_) => "ok",
        Outcome::Missing => "missing",
        Outcome::Wrong(_) => "wrong",
        Outcome::Unknown(_) => "unknown",
    }
}

fn detail(o: &Outcome) -> Option<String> {
    match o {
        Outcome::Ok(v) => v.clone(),
        Outcome::Missing => None,
        Outcome::Wrong(d) | Outcome::Unknown(d) => Some(d.clone()),
    }
}

fn print_line(chk: &Check, o: &Outcome) {
    // A missing Required check is an error (✗); a missing Recommended /
    // Optional check, or an undeterminable one, is just a note (▵).
    let glyph = match o {
        Outcome::Ok(_) => "\u{2713}", // ✓
        Outcome::Unknown(_) => "\u{25b5}", // ▵
        Outcome::Missing | Outcome::Wrong(_) => {
            if chk.level == Level::Required {
                "\u{2717}" // ✗
            } else {
                "\u{25b5}" // ▵
            }
        }
    };

    let detail = match o {
        Outcome::Ok(Some(v)) => v.clone(),
        Outcome::Ok(None) => "installed".to_string(),
        Outcome::Unknown(r) => format!("{r} ({})", chk.fix),
        Outcome::Wrong(d) => format!("{d} — {}", chk.fix),
        Outcome::Missing => format!("not found — {}", chk.fix),
    };

    println!("  {glyph} {:<32} {detail}", chk.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_doctor_shows_every_implemented_group() {
        assert_eq!(
            categories_to_show(None),
            vec![Category::Core, Category::Web, Category::Ios, Category::Android]
        );
    }

    #[test]
    fn implemented_groups_have_checks() {
        for cat in [Category::Ios, Category::Android] {
            assert!(
                checks().iter().any(|c| c.category == cat),
                "{} group should be implemented",
                cat.title()
            );
        }
    }

    #[test]
    fn platform_target_scopes_to_core_plus_that_group() {
        assert_eq!(
            categories_to_show(Some(Platform::Web)),
            vec![Category::Core, Category::Web]
        );
        assert_eq!(
            categories_to_show(Some(Platform::Ios)),
            vec![Category::Core, Category::Ios]
        );
    }

    #[test]
    fn host_preview_targets_need_only_core() {
        // sim / macos / terminal / runtime-server map to no extra group.
        for p in [Platform::Sim, Platform::Macos, Platform::Terminal, Platform::RuntimeServer] {
            assert_eq!(platform_category(p), None, "{p} should need no extra group");
            assert_eq!(categories_to_show(Some(p)), vec![Category::Core]);
        }
    }

    #[test]
    fn enforcement_is_core_plus_requested_only() {
        // Bare doctor: only Core is enforced, Web is report-only.
        assert!(is_enforced(Category::Core, None));
        assert!(!is_enforced(Category::Web, None));
        // `doctor web`: Core and Web enforced, others not.
        let web = Some(Category::Web);
        assert!(is_enforced(Category::Core, web));
        assert!(is_enforced(Category::Web, web));
        assert!(!is_enforced(Category::Android, web));
    }

    #[test]
    fn status_keys_match_outcomes() {
        assert_eq!(status_key(&Outcome::Ok(None)), "ok");
        assert_eq!(status_key(&Outcome::Missing), "missing");
        assert_eq!(status_key(&Outcome::Unknown("x".into())), "unknown");
    }

    #[test]
    fn every_check_has_a_nonempty_fix() {
        for c in checks() {
            assert!(!c.fix.is_empty(), "check `{}` is missing a fix hint", c.id);
        }
    }
}
