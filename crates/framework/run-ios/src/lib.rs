//! Direct iOS-Simulator app builder + launcher for `idealyst run ios`.
//!
//! No Xcode project. We invoke the same command-line tools Xcode does:
//!
//! ```text
//!   build-ios::build   → Rust staticlib (lib<project>_ios_wrapper.a)
//!   swiftc             → compiles AppDelegate.swift + ViewController.swift
//!                        AND links them against the staticlib + iOS frameworks
//!                        into a Mach-O executable
//!   assemble bundle    → Mach-O + Info.plist + PkgInfo into a .app directory
//!   simctl boot        → bring up an iOS simulator
//!   simctl install     → copy the .app to the booted simulator
//!   simctl launch      → start it
//! ```
//!
//! Why no Xcode project: the user's app is platform-agnostic source
//! plus a tiny `[package.metadata.idealyst]` block. The .app is just
//! a build artifact, derivable from those inputs. Maintaining a
//! pbxproj template (UUIDs, build phases, Xcode-version drift) buys
//! us nothing the user asked for. Apps that need Xcode-the-IDE for
//! debugging will eventually be served by a `scaffold ios` command
//! that emits an .xcodeproj on demand.
//!
//! Limited to simulator builds today. Device builds need code
//! signing (provisioning profile + signing identity + entitlements)
//! which is a separate problem.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use build_ios::{BuildOptions, Manifest};

/// Embedded Swift sources + plist template. Tiny and identical for
/// every project, so we ship them as `include_str!` rather than
/// generating from scratch.
const APP_DELEGATE_SWIFT: &str = include_str!("../templates/AppDelegate.swift");
const VIEW_CONTROLLER_SWIFT: &str = include_str!("../templates/ViewController.swift");
const BRIDGING_HEADER_H: &str = include_str!("../templates/BridgingHeader.h");
const INFO_PLIST_TMPL: &str = include_str!("../templates/Info.plist.tmpl");

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Build the Rust staticlib in release mode. Swift always
    /// compiles with `-O` regardless — Swift's debug build is
    /// painfully slow on iOS and these sources are trivial.
    pub release: bool,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// The `.app` bundle that was launched.
    pub app_bundle: PathBuf,
    /// UDID of the simulator the app is running on.
    pub simulator_udid: String,
}

pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = build_ios::parse_manifest(&project_dir)?;
    let workspace_root = build_ios::find_workspace_root(&project_dir)?;

    // ── 1. Build the Rust staticlib ──────────────────────────────
    let artifact = build_ios::build(
        &project_dir,
        BuildOptions {
            release: opts.release,
            device: false,
        },
    )?;

    // ── 2. Lay out the bundle dir ────────────────────────────────
    let ios_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("ios");
    let swift_dir = ios_dir.join("swift");
    let executable_name = title_case_for_executable(&manifest.name);
    let app_bundle = ios_dir.join(format!("{executable_name}.app"));

    if app_bundle.exists() {
        fs::remove_dir_all(&app_bundle).with_context(|| {
            format!("clear stale {} before rebuild", app_bundle.display())
        })?;
    }
    fs::create_dir_all(&app_bundle).with_context(|| format!("create {}", app_bundle.display()))?;
    fs::create_dir_all(&swift_dir).with_context(|| format!("create {}", swift_dir.display()))?;

    // ── 3. Write Swift sources + bridging header ─────────────────
    fs::write(swift_dir.join("AppDelegate.swift"), APP_DELEGATE_SWIFT)?;
    fs::write(swift_dir.join("ViewController.swift"), VIEW_CONTROLLER_SWIFT)?;
    fs::write(swift_dir.join("BridgingHeader.h"), BRIDGING_HEADER_H)?;

    // ── 4. swiftc: compile Swift + link executable ───────────────
    let exe_path = app_bundle.join(&executable_name);
    compile_and_link(
        &swift_dir,
        artifact.staticlib.parent().expect("staticlib has parent"),
        &format!("{}_ios_wrapper", manifest.lib_name),
        &exe_path,
    )?;

    // ── 5. Info.plist + PkgInfo ──────────────────────────────────
    fs::write(
        app_bundle.join("Info.plist"),
        render_info_plist(&manifest, &executable_name),
    )?;
    // 8 bytes; "APPL????" is the canonical "type=APPL, creator=anonymous"
    // for iOS apps. Optional in modern iOS but harmless and ~free.
    fs::write(app_bundle.join("PkgInfo"), b"APPL????")?;

    // ── 6. Simulator: boot, install, launch ──────────────────────
    let udid = ensure_simulator_booted()?;
    install_app(&udid, &app_bundle)?;
    launch_app(&udid, &manifest.app.bundle_id)?;

    Ok(RunArtifact {
        app_bundle,
        simulator_udid: udid,
    })
}

// ---------------------------------------------------------------------------
// Swift compile + link
// ---------------------------------------------------------------------------

fn compile_and_link(
    swift_dir: &Path,
    lib_dir: &Path,
    lib_name: &str,
    output: &Path,
) -> Result<()> {
    // Target triple matches the rustc one. `build-ios::pick_target`
    // returned `aarch64-apple-ios-sim`; Swift's equivalent is
    // `arm64-apple-ios16.0-simulator`. We keep the deployment target
    // (16.0) in sync with Info.plist's `MinimumOSVersion`.
    let target = if cfg!(target_arch = "aarch64") {
        "arm64-apple-ios16.0-simulator"
    } else {
        "x86_64-apple-ios16.0-simulator"
    };
    let sdk_path = xcrun_sdk_path("iphonesimulator")?;
    let bridging = swift_dir.join("BridgingHeader.h");

    eprintln!("[run-ios] swiftc -target {target} → {}", output.display());

    let status = Command::new("xcrun")
        .args(["-sdk", "iphonesimulator", "swiftc"])
        .args(["-target", target])
        .args(["-sdk"])
        .arg(&sdk_path)
        .args(["-import-objc-header"])
        .arg(&bridging)
        .arg("-emit-executable")
        .arg("-O")
        .args(["-o"])
        .arg(output)
        .arg(swift_dir.join("AppDelegate.swift"))
        .arg(swift_dir.join("ViewController.swift"))
        .arg("-L")
        .arg(lib_dir)
        .arg(format!("-l{lib_name}"))
        // The Rust staticlib pulls in objc2/objc2-foundation/objc2-ui-kit
        // which need these frameworks at link time. Foundation +
        // UIKit are the must-haves; QuartzCore is used by
        // backend-ios' CALayer code; CoreGraphics by anything that
        // touches CGRect/CGFloat at the FFI boundary.
        .args(["-framework", "UIKit"])
        .args(["-framework", "Foundation"])
        .args(["-framework", "CoreGraphics"])
        .args(["-framework", "QuartzCore"])
        .status()
        .with_context(|| "spawn xcrun swiftc")?;

    if !status.success() {
        anyhow::bail!("swiftc exited with {status}");
    }
    Ok(())
}

fn xcrun_sdk_path(sdk: &str) -> Result<PathBuf> {
    let out = Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .with_context(|| "spawn xcrun --show-sdk-path")?;
    if !out.status.success() {
        anyhow::bail!(
            "xcrun --sdk {sdk} --show-sdk-path failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let path = String::from_utf8(out.stdout).context("xcrun output not utf-8")?;
    Ok(PathBuf::from(path.trim()))
}

// ---------------------------------------------------------------------------
// Info.plist render
// ---------------------------------------------------------------------------

fn render_info_plist(manifest: &Manifest, executable_name: &str) -> String {
    INFO_PLIST_TMPL
        .replace("{{APP_NAME}}", &xml_escape(&manifest.app.name))
        .replace("{{BUNDLE_ID}}", &xml_escape(&manifest.app.bundle_id))
        .replace("{{EXECUTABLE}}", &xml_escape(executable_name))
        .replace("{{VERSION}}", &xml_escape(&manifest.app.version))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Simulator orchestration
// ---------------------------------------------------------------------------

/// Find a booted simulator if one exists, or boot the first available
/// iPhone. Returns its UDID. Also opens Simulator.app so the window
/// surfaces; on a fresh machine the boot can take a few seconds.
fn ensure_simulator_booted() -> Result<String> {
    if let Some(udid) = find_booted_simulator()? {
        eprintln!("[run-ios] reusing booted simulator {udid}");
        // Make sure Simulator.app is visible.
        let _ = Command::new("open").args(["-a", "Simulator"]).status();
        return Ok(udid);
    }

    // Nothing booted — pick the first available iPhone and boot it.
    let udid = pick_iphone()?;
    eprintln!("[run-ios] booting simulator {udid}");
    let status = Command::new("xcrun")
        .args(["simctl", "boot", &udid])
        .status()
        .with_context(|| "spawn xcrun simctl boot")?;
    if !status.success() {
        anyhow::bail!("xcrun simctl boot exited with {status}");
    }
    // Surface the Simulator window.
    let _ = Command::new("open").args(["-a", "Simulator"]).status();

    wait_for_boot(&udid)?;
    Ok(udid)
}

/// Run `simctl list devices booted` and pick the first UDID. Returns
/// `None` if none are booted. We parse the human-readable output —
/// simctl's JSON mode works but the keys vary across Xcode versions
/// and the text format is stable enough for our needs.
fn find_booted_simulator() -> Result<Option<String>> {
    let out = Command::new("xcrun")
        .args(["simctl", "list", "devices", "booted"])
        .output()
        .with_context(|| "spawn xcrun simctl list devices booted")?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        // Format: "    iPhone 15 (XXXX-XXXX) (Booted)"
        if !line.contains("(Booted)") {
            continue;
        }
        if let Some(udid) = extract_udid(line) {
            return Ok(Some(udid));
        }
    }
    Ok(None)
}

/// Pull the first parenthesized GUID out of an simctl line.
fn extract_udid(line: &str) -> Option<String> {
    let start = line.find('(')? + 1;
    let rest = &line[start..];
    let end = rest.find(')')?;
    Some(rest[..end].to_string())
}

/// Pick the first available iPhone simulator. Boot it if it isn't
/// already running. We prefer matching `iPhone N` lines from the
/// `available` list — that filters out unavailable runtimes.
fn pick_iphone() -> Result<String> {
    let out = Command::new("xcrun")
        .args(["simctl", "list", "devices", "available"])
        .output()
        .with_context(|| "spawn xcrun simctl list devices available")?;
    if !out.status.success() {
        anyhow::bail!(
            "xcrun simctl list devices available failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("iPhone ") {
            if let Some(udid) = extract_udid(trimmed) {
                return Ok(udid);
            }
        }
    }
    anyhow::bail!(
        "no available iPhone simulator found — run `xcrun simctl list devices available` \
         to see what's installed, or `xcodebuild -downloadPlatform iOS` to fetch a runtime"
    )
}

fn wait_for_boot(udid: &str) -> Result<()> {
    // `simctl bootstatus` blocks until the device has finished
    // booting. The `-b` flag means "wait for boot to complete".
    let status = Command::new("xcrun")
        .args(["simctl", "bootstatus", udid, "-b"])
        .status()
        .with_context(|| "spawn xcrun simctl bootstatus")?;
    if !status.success() {
        anyhow::bail!("xcrun simctl bootstatus exited with {status}");
    }
    // Belt-and-braces: bootstatus returns once Springboard is up,
    // but installs still occasionally race the first second. A short
    // pause here saves an obscure error from simctl install.
    thread::sleep(Duration::from_millis(500));
    let _ = Instant::now();
    Ok(())
}

fn install_app(udid: &str, app: &Path) -> Result<()> {
    eprintln!("[run-ios] simctl install {} → {udid}", app.display());
    let status = Command::new("xcrun")
        .args(["simctl", "install", udid])
        .arg(app)
        .status()
        .with_context(|| "spawn xcrun simctl install")?;
    if !status.success() {
        anyhow::bail!("xcrun simctl install exited with {status}");
    }
    Ok(())
}

fn launch_app(udid: &str, bundle_id: &str) -> Result<()> {
    eprintln!("[run-ios] simctl launch {bundle_id} on {udid}");
    let status = Command::new("xcrun")
        .args(["simctl", "launch", udid, bundle_id])
        .status()
        .with_context(|| "spawn xcrun simctl launch")?;
    if !status.success() {
        anyhow::bail!("xcrun simctl launch exited with {status}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Title-case a cargo package name for use as the .app executable
/// name (e.g., `docs` → `Docs`, `my-app` → `MyApp`). Stripped of
/// anything Xcode would dislike inside a bundle.
fn title_case_for_executable(s: &str) -> String {
    let mut out = String::new();
    for word in s.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()) {
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            for u in c.to_uppercase() {
                out.push(u);
            }
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        // Fallback for pathological names; should never hit.
        "App".to_string()
    } else {
        out
    }
}
