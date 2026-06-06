//! Physical-iOS-device build/sign/install/launch for
//! `idealyst run ios --device`.
//!
//! The simulator path ([`crate::run`]) compiles Swift with raw `swiftc`
//! and assembles a `.app` by hand. The device path can NOT reuse that:
//! code-signing a device build requires an Xcode project + `xcodebuild`
//! so the auto-provisioning machinery (`-allowProvisioningUpdates`) can
//! register the device with the portal and mint a development profile.
//!
//! The validated sequence (see [[project_ios_device_deploy_from_cli]],
//! proven on an iPhone X / iOS 16.7):
//!
//! ```text
//!   build-ios::build (device, release)  → libNAME_ios_wrapper.a (aarch64-apple-ios)
//!   render Swift + BridgingHeader + Info.plist + project.pbxproj
//!   xcodebuild -destination "id=<UDID>" -allowProvisioningUpdates build
//!   ios-deploy --id <UDID> --bundle <app> --justlaunch
//! ```
//!
//! ## Three non-obvious gotchas baked in here
//!
//! 1. **Weak-link the objc2-bound frameworks** (UIKit / Foundation /
//!    AVFoundation / CoreMedia / CoreVideo). objc2-ui-kit / objc2-foundation
//!    reference framework constants introduced AFTER the device's iOS
//!    (e.g. `_UIAccessibilityPriorityDefault`, post-16.7). Rust/objc2 has no
//!    `@available` gating, so they're HARD imports — dyld aborts at launch
//!    with "Symbol not found" on the older OS. Weak-linking makes absent
//!    symbols resolve to NULL (we never call them), the same back-deploy
//!    Xcode applies automatically. This lives in the generated pbxproj's
//!    `ATTRIBUTES = (Weak, )` on those framework build files. CoreGraphics /
//!    QuartzCore stay strong (old/stable). The simulator never hits this
//!    because it has a current-OS runtime.
//! 2. **Build the staticlib in RELEASE by default.** A debug staticlib makes
//!    compute-heavy paths (camera BGRA→RGBA swizzle, framework layout/reactive)
//!    unusably slow on-device. `--debug` opts out.
//! 3. **Target `id=<UDID>`, NOT `generic/platform=iOS`.** A generic
//!    destination gives the provisioning step no device, so the minted
//!    profile omits the phone and install fails with 0xe8008015
//!    ("valid provisioning profile not found"). The UDID comes from
//!    `xcrun xctrace list devices` — `devicectl` reports older devices as
//!    "unavailable" and can't install to them.
//!
//! And the installer choice: `ios-deploy` (classic lockdown/AFC path) works
//! on older devices where `xcrun devicectl device install` fails with
//! "error 1010 usage assertion". It's a brew tool; we detect its absence
//! and emit `brew install ios-deploy` rather than a opaque failure.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{BuildOptions, FrameworkSource, Manifest};

use crate::frameworks::{collect_ios_frameworks, pbx_ids, Framework};
use crate::{
    render_view_controller, title_case_for_executable, xml_escape,
    RunMode, APP_DELEGATE_SWIFT, BRIDGING_HEADER_LOCAL_H, INFO_PLIST_TMPL,
};

const PBXPROJ_TMPL: &str = include_str!("../templates/project.pbxproj.tmpl");

/// `CFBundleIconName` plist entry spliced into the device/archive Info.plist.
/// Points at the `AppIcon` asset catalog the pbxproj references (and `actool`
/// compiles). Apps built with the iOS 11+ SDK MUST carry this key alongside
/// an asset-catalog icon, or App Store ingestion rejects the build (error
/// 90713). Set on the xcodebuild paths only — the simulator bundle has no
/// compiled catalog, so it keeps loose-PNG `CFBundleIcons` instead.
const CFBUNDLE_ICON_NAME_ENTRY: &str = "<key>CFBundleIconName</key>\n    <string>AppIcon</string>";

#[derive(Clone, Debug)]
pub struct DeviceOptions {
    /// Build the Rust staticlib in release. Defaults to `true` for the
    /// device path (gotcha 3 — debug is unusably slow on-device). The CLI
    /// flips this to `false` only when the user passes `--debug`.
    pub release: bool,
    /// Where the wrapper Cargo.toml sources framework crates from.
    pub source: FrameworkSource,
    /// Cargo features to enable on the build (forwarded to the wrapper
    /// cargo invocation).
    pub user_features: Vec<String>,
    /// Apple Developer team ID (the 10-char identifier embedded as the OU
    /// of the signing cert's subject, e.g. `USC735CN86`). The CLI resolves
    /// this via [`resolve_team`] (explicit `--team` →
    /// `$IDEALYST_DEVELOPMENT_TEAM` → `$DEVELOPMENT_TEAM` → auto-discovery)
    /// before constructing these options, so it's always concrete here.
    pub team: String,
    /// Specific device UDID to target. `None` ⇒ auto-discover the first
    /// connected device via `xcrun xctrace list devices`.
    pub udid: Option<String>,
}

#[derive(Debug)]
pub struct DeviceArtifact {
    /// The signed `.app` bundle xcodebuild produced (under derivedData).
    pub app_bundle: PathBuf,
    /// UDID of the device the app was installed + launched on.
    pub device_udid: String,
    /// The generated `.xcodeproj` (kept for debugging / re-opening in Xcode).
    pub xcodeproj: PathBuf,
}

/// Build, sign, install, and launch the user's project on a connected
/// physical iPhone. See module docs for the full sequence + gotchas.
pub fn run(project_dir: &Path, opts: DeviceOptions) -> Result<DeviceArtifact> {
    let project_dir = std::fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = build_ios::parse_manifest(&project_dir)?;
    // Fail fast on a missing bundle id before the slow build / device probe.
    manifest.app.require_bundle_id()?;

    // ── 0. Resolve the target device UP FRONT ────────────────────
    // Fail before the (slow) Rust build if no device is attached — a
    // missing phone shouldn't cost the user a multi-minute compile.
    let udid = match &opts.udid {
        Some(u) => u.clone(),
        None => discover_device_udid()?,
    };
    eprintln!("[run-ios --device] target device {udid}");

    // ── 1. Build the wrapper staticlib (device target, release) ──
    let artifact = build_ios::build(
        &project_dir,
        BuildOptions {
            release: opts.release,
            device: true,
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
        },
    )?;

    // ── 2-5. Lay out the signed-build project dir (shared with the
    // `idealyst publish ios` archive path — see [`prepare_xcode_project`]).
    let prepared = prepare_xcode_project(
        &project_dir,
        &manifest,
        &artifact,
        &PrepareOpts {
            team: opts.team.clone(),
            subdir: "ios-device",
            source: opts.source.clone(),
        },
    )?;

    // ── 6. xcodebuild: sign + build for the specific device ──────
    let build_dir = prepared.project_root.join("build");
    xcodebuild_for_device(&prepared.xcodeproj, &prepared.scheme, &udid, &build_dir)?;

    // xcodebuild writes the signed bundle here.
    let app_bundle = build_dir
        .join("Build")
        .join("Products")
        .join("Debug-iphoneos")
        .join(format!("{}.app", prepared.scheme));
    if !app_bundle.is_dir() {
        anyhow::bail!(
            "xcodebuild reported success but no .app at {}",
            app_bundle.display(),
        );
    }

    // ── 7. ios-deploy: install + launch ──────────────────────────
    install_and_launch(&udid, &app_bundle)?;

    Ok(DeviceArtifact {
        app_bundle,
        device_udid: udid,
        xcodeproj: prepared.xcodeproj,
    })
}

// ---------------------------------------------------------------------------
// Shared Xcode-project layout (device install + App Store archive)
// ---------------------------------------------------------------------------

/// Knobs for [`prepare_xcode_project`]. The two callers (device install,
/// App Store publish) differ only in the on-disk sub-directory; everything
/// else (Swift glue, icons, Info.plist, framework derivation, signing) is
/// identical. Both archive/build with automatic **development** signing —
/// the App Store *distribution* re-sign happens at `-exportArchive` time
/// (see [`crate::publish`]), not here.
pub(crate) struct PrepareOpts {
    /// Apple Developer team ID embedded as `DEVELOPMENT_TEAM`.
    pub team: String,
    /// Project sub-directory under the wrapper root, e.g. `"ios-device"`
    /// (install) or `"ios-dist"` (publish), so the two never collide.
    pub subdir: &'static str,
    /// Where the wrapper sources framework crates from (for `wrapper_root`
    /// + the cargo-metadata framework walk).
    pub source: FrameworkSource,
}

/// A laid-out, ready-to-`xcodebuild` Xcode project.
pub(crate) struct PreparedProject {
    /// The project root dir (holds `Info.plist`, icons, `swift/`, `.xcodeproj`).
    pub project_root: PathBuf,
    /// The generated `.xcodeproj`.
    pub xcodeproj: PathBuf,
    /// The scheme / target / executable name (title-cased project name).
    pub scheme: String,
}

/// Render the Swift glue + bridging header + icons + Info.plist + a
/// `.xcodeproj` (no `xcodegen` dependency) for an already-built staticlib.
/// Shared by the device-install path ([`run`]) and the App Store archive
/// path ([`crate::publish::publish`]). The only variation is the on-disk
/// sub-directory; both use automatic development signing (the publish path
/// re-signs for distribution at export, not here).
///
/// Splash is forced OFF (duration 0): signed builds mount the framework
/// immediately and the splash path isn't needed to validate signing.
pub(crate) fn prepare_xcode_project(
    project_dir: &Path,
    manifest: &Manifest,
    artifact: &build_ios::BuildArtifact,
    opts: &PrepareOpts,
) -> Result<PreparedProject> {
    let bundle_id = manifest.app.require_bundle_id()?.to_string();

    // The staticlib's parent dir is what `LIBRARY_SEARCH_PATHS` points at;
    // its `-l` name is the filename stem minus `lib`/`.a`.
    let lib_dir = artifact
        .staticlib
        .parent()
        .expect("staticlib has parent")
        .to_path_buf();
    let lib_name = format!("{}_ios_wrapper", manifest.lib_name);

    let wrapper_manifest = artifact.wrapper_dir.join("Cargo.toml");
    let permission_plist =
        crate::ios_permission_entries(&wrapper_manifest, &manifest.app.permissions);

    // Frameworks to link are DERIVED from the dep graph (each SDK declares its
    // own `[package.metadata.idealyst.ios].frameworks`), not hardcoded — a
    // screen-capture app needs ReplayKit linked or `class!(RPScreenRecorder)`
    // panics at launch. See [`crate::frameworks`].
    let frameworks = collect_ios_frameworks(&wrapper_manifest)?;

    let project_root = opts
        .source
        .wrapper_root(project_dir)
        .join(&manifest.name)
        .join(opts.subdir);
    let swift_dir = project_root.join("swift");
    let executable_name = title_case_for_executable(&manifest.name);
    std::fs::create_dir_all(&swift_dir)
        .with_context(|| format!("create {}", swift_dir.display()))?;

    // Swift sources + bridging header (splash forced off — see fn doc).
    std::fs::write(swift_dir.join("AppDelegate.swift"), APP_DELEGATE_SWIFT)?;
    std::fs::write(
        swift_dir.join("ViewController.swift"),
        render_view_controller(&splashless(manifest), &RunMode::Local),
    )?;
    std::fs::write(swift_dir.join("BridgingHeader.h"), BRIDGING_HEADER_LOCAL_H)?;

    // App-icon ASSET CATALOG + Info.plist. The xcodebuild paths (device
    // install + App Store archive) use a real `Assets.xcassets/AppIcon`
    // catalog — `actool` compiles it and injects the icons, which is what
    // App Store ingestion requires (loose PNGs fail validation, error
    // 90713). The catalog is ALWAYS produced (placeholder when the project
    // declares no icon), so the pbxproj can unconditionally reference it and
    // every build carries a valid `CFBundleIconName = AppIcon`. (The
    // simulator path keeps loose PNGs — it hand-assembles the bundle with no
    // `actool`. See [`crate::sync_ios_icons_into_bundle`].)
    crate::sync_ios_asset_catalog_into_project(project_dir, &project_root)?;
    std::fs::write(
        project_root.join("Info.plist"),
        render_device_info_plist(
            manifest,
            &executable_name,
            CFBUNDLE_ICON_NAME_ENTRY,
            &permission_plist,
        )?,
    )?;

    // Generate the .xcodeproj (no xcodegen dependency).
    let xcodeproj = project_root.join(format!("{executable_name}.xcodeproj"));
    write_xcodeproj(
        &xcodeproj,
        &PbxParams {
            app_name: &executable_name,
            bundle_id: &bundle_id,
            team: &opts.team,
            library_search_path: &lib_dir,
            lib_name: &lib_name,
            frameworks: &frameworks,
        },
    )?;

    Ok(PreparedProject {
        project_root,
        xcodeproj,
        scheme: executable_name,
    })
}

/// Clone the manifest with the splash forced off (duration 0). Device
/// builds skip the splash — see `render_view_controller`'s
/// `{{SPLASH_DURATION_MS}}` substitution.
fn splashless(manifest: &Manifest) -> Manifest {
    let mut m = manifest.clone();
    m.app.splash.duration_ms = 0;
    m
}

// ---------------------------------------------------------------------------
// Info.plist (device flavor)
// ---------------------------------------------------------------------------

/// Render the device Info.plist. Unlike the sim/dev plist
/// (`crate::render_info_plist`), this does NOT inject the
/// `NSAllowsLocalNetworking` ATS exception or the runtime-server endpoint:
/// those are simulator/dev-loop concerns (the sim shares the host loopback;
/// a real device's `127.0.0.1` is the phone itself). Device builds are the
/// local-mount, standalone flavor. Icon + capability permission entries are
/// still spliced in.
fn render_device_info_plist(
    manifest: &Manifest,
    executable_name: &str,
    icon_entries: &str,
    permission_entries: &str,
) -> Result<String> {
    let extra_entries = [icon_entries, permission_entries]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n    ");
    let bundle_id = manifest.app.require_bundle_id()?;
    Ok(INFO_PLIST_TMPL
        .replace("{{APP_NAME}}", &xml_escape(&manifest.app.name))
        .replace("{{BUNDLE_ID}}", &xml_escape(bundle_id))
        .replace("{{EXECUTABLE}}", &xml_escape(executable_name))
        .replace("{{VERSION}}", &xml_escape(&manifest.app.version))
        .replace("{{BUILD_NUMBER}}", &xml_escape(&manifest.app.build_number))
        .replace("{{EXTRA_PLIST_ENTRIES}}", &extra_entries))
}

// ---------------------------------------------------------------------------
// pbxproj generation (replaces xcodegen)
// ---------------------------------------------------------------------------

struct PbxParams<'a> {
    app_name: &'a str,
    bundle_id: &'a str,
    team: &'a str,
    library_search_path: &'a Path,
    lib_name: &'a str,
    /// The Apple frameworks to link, derived from the dep graph (base set +
    /// each SDK's declared frameworks). Drives the four framework-related
    /// pbxproj sections, replacing what used to be hardcoded in the template.
    frameworks: &'a [Framework],
}

/// Write `<name>.xcodeproj/project.pbxproj` from the parameterized
/// template. The template is a capture of the pbxproj `xcodegen` produced
/// from the validated `project.yml`, so we don't depend on `xcodegen`
/// being installed. We substitute user-facing values; the non-framework
/// object IDs stay fixed (they only need to be internally consistent), while
/// the framework objects are generated from `params.frameworks` with
/// deterministic IDs (see [`render_frameworks_sections`]).
fn write_xcodeproj(xcodeproj: &Path, params: &PbxParams) -> Result<()> {
    std::fs::create_dir_all(xcodeproj)
        .with_context(|| format!("create {}", xcodeproj.display()))?;
    std::fs::write(
        xcodeproj.join("project.pbxproj"),
        render_pbxproj(params),
    )?;
    Ok(())
}

fn render_pbxproj(params: &PbxParams) -> String {
    let (build_files, file_refs, phase_files, group_children) =
        render_frameworks_sections(params.frameworks);
    PBXPROJ_TMPL
        .replace("{{APP_NAME}}", params.app_name)
        .replace("{{BUNDLE_ID}}", params.bundle_id)
        .replace("{{DEVELOPMENT_TEAM}}", params.team)
        .replace(
            "{{LIBRARY_SEARCH_PATH}}",
            &params.library_search_path.display().to_string(),
        )
        .replace("{{LIB_NAME}}", params.lib_name)
        .replace("{{FRAMEWORK_BUILD_FILES}}", &build_files)
        .replace("{{FRAMEWORK_FILE_REFS}}", &file_refs)
        .replace("{{FRAMEWORK_PHASE_FILES}}", &phase_files)
        .replace("{{FRAMEWORK_GROUP_CHILDREN}}", &group_children)
}

/// Render the four framework-dependent pbxproj sections from the derived list:
/// PBXBuildFile entries, PBXFileReference entries, the frameworks build-phase
/// `files` list, and the frameworks group `children` list. Each framework gets
/// two deterministic 24-hex IDs (build-file + file-ref) so reruns are
/// byte-identical. Weak frameworks carry `ATTRIBUTES = (Weak, )` on their build
/// file — the objc2 back-deployment fix (see [`crate::frameworks`]).
///
/// Indentation matches the surrounding template (two tabs for object lines, one
/// extra for list members) so the generated pbxproj stays tidy.
fn render_frameworks_sections(frameworks: &[Framework]) -> (String, String, String, String) {
    let mut build_files = Vec::new();
    let mut file_refs = Vec::new();
    let mut phase_files = Vec::new();
    let mut group_children = Vec::new();

    for fw in frameworks {
        let name = &fw.name;
        let (bf_id, fr_id) = pbx_ids(name);
        let weak_attr = if fw.weak {
            " settings = {ATTRIBUTES = (Weak, ); };"
        } else {
            ""
        };
        build_files.push(format!(
            "\t\t{bf_id} /* {name}.framework in Frameworks */ = {{isa = PBXBuildFile; fileRef = {fr_id} /* {name}.framework */;{weak_attr} }};"
        ));
        file_refs.push(format!(
            "\t\t{fr_id} /* {name}.framework */ = {{isa = PBXFileReference; lastKnownFileType = wrapper.framework; name = {name}.framework; path = System/Library/Frameworks/{name}.framework; sourceTree = SDKROOT; }};"
        ));
        phase_files.push(format!(
            "\t\t\t\t{bf_id} /* {name}.framework in Frameworks */,"
        ));
        group_children.push(format!("\t\t\t\t{fr_id} /* {name}.framework */,"));
    }

    (
        build_files.join("\n"),
        file_refs.join("\n"),
        phase_files.join("\n"),
        group_children.join("\n"),
    )
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

/// Discover the first connected physical iPhone's UDID via
/// `xcrun xctrace list devices`. We parse `xctrace` rather than
/// `devicectl` because `devicectl` marks older devices (iPhone X / iOS
/// 16.7) "unavailable" and refuses to install to them — `xctrace` still
/// lists them and the classic install path (ios-deploy) works.
///
/// Output format (the device section is above a `== Simulators ==`
/// divider):
/// ```text
/// == Devices ==
/// Nicho's iPhone (16.7.10) (00008020-000123456789ABCD)
/// My Mac (...)
/// == Simulators ==
/// iPhone 15 (17.0) (XXXX-...)
/// ```
/// We take the first line in the Devices section that carries a
/// `(<version>)` AND a `(<udid>)` and whose UDID looks like a real device
/// UDID (contains a hyphen or is 25/40 hex). "My Mac" has no `(version)`
/// pair so it's skipped.
fn discover_device_udid() -> Result<String> {
    let out = Command::new("xcrun")
        .args(["xctrace", "list", "devices"])
        .output()
        .with_context(|| "spawn `xcrun xctrace list devices`")?;
    if !out.status.success() {
        anyhow::bail!(
            "`xcrun xctrace list devices` failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(udid) = parse_first_device_udid(&text) {
        return Ok(udid);
    }
    anyhow::bail!(
        "no connected iPhone found. Plug in a device, unlock it, and trust this \
         Mac (tap \"Trust\" on the phone). Verify it shows up under \
         `xcrun xctrace list devices` (Devices section), then re-run. To target a \
         specific device, pass `--udid <UDID>`."
    )
}

/// Parse the first physical-device UDID out of `xctrace list devices`
/// output. Stops at the `== Simulators ==` divider so a booted simulator
/// is never picked. Pulled out for unit testing without a device.
fn parse_first_device_udid(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        // The simulators block follows the devices block; once we hit it
        // there are no more real devices to consider.
        if trimmed.starts_with("== Simulators") {
            break;
        }
        if trimmed.starts_with("==") || trimmed.is_empty() {
            continue;
        }
        // A device line looks like `Name (version) (udid)`. The LAST
        // parenthesized group is the UDID; the second-to-last is the OS
        // version. "My Mac" lines have only one parenthesized group, so
        // requiring two filters them out.
        let groups: Vec<&str> = collect_paren_groups(trimmed);
        if groups.len() < 2 {
            continue;
        }
        let candidate = groups[groups.len() - 1];
        if looks_like_device_udid(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Collect every `(...)` group's inner text from a line, in order.
fn collect_paren_groups(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            if let Some(close) = line[i + 1..].find(')') {
                out.push(&line[i + 1..i + 1 + close]);
                i = i + 1 + close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Heuristic: a real device UDID is either the modern 25-char
/// `XXXXXXXX-XXXXXXXXXXXXXXXX` form (one hyphen, hex) or a legacy 40-char
/// all-hex string. Crucially it is NOT a plain OS version like `16.7.10`.
fn looks_like_device_udid(s: &str) -> bool {
    // Reject version-looking strings (digits + dots only).
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let hyphens = s.matches('-').count();
    let hex_and_hyphen = s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    // Modern: 25 chars, exactly one hyphen. Legacy: 40 hex chars, no hyphen.
    (hex_and_hyphen && hyphens == 1 && s.len() >= 25)
        || (hyphens == 0 && s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
}

// ---------------------------------------------------------------------------
// xcodebuild
// ---------------------------------------------------------------------------

/// Build + sign the project for a specific device. `-allowProvisioningUpdates`
/// + `-allowProvisioningDeviceRegistration` let Xcode register the device with
/// the portal and mint a development profile on the fly. The destination MUST
/// be `id=<UDID>` (a concrete device), not `generic/platform=iOS` — see
/// module-doc gotcha 3.
fn xcodebuild_for_device(
    xcodeproj: &Path,
    scheme: &str,
    udid: &str,
    derived_data: &Path,
) -> Result<()> {
    eprintln!("[run-ios --device] xcodebuild (sign + build) for id={udid}");
    let status = Command::new("xcodebuild")
        .arg("-project")
        .arg(xcodeproj)
        .args(["-scheme", scheme])
        .args(["-configuration", "Debug"])
        .args(["-destination", &format!("id={udid}")])
        .arg("-derivedDataPath")
        .arg(derived_data)
        .arg("-allowProvisioningUpdates")
        .arg("-allowProvisioningDeviceRegistration")
        .arg("build")
        .status()
        .with_context(|| "spawn xcodebuild")?;
    if !status.success() {
        anyhow::bail!(
            "xcodebuild failed (exit {status}). Common causes: the chosen team \
             ({scheme} project's DEVELOPMENT_TEAM) can't sign for this device, or the \
             device isn't registered. If signing failed with a provisioning error, \
             open {} in Xcode once, select your team under Signing & Capabilities, \
             then re-run.",
            xcodeproj.display(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ios-deploy install + launch
// ---------------------------------------------------------------------------

/// Install + launch via `ios-deploy`. Chosen over `xcrun devicectl device
/// install` because devicectl fails on older devices ("error 1010 usage
/// assertion"); ios-deploy uses the classic lockdown/AFC path that works on
/// iPhone X / iOS 16.7.
///
/// ios-deploy with `--justlaunch` frequently exits non-zero on lldb detach
/// even when install + launch succeeded, so we treat the run as a success if
/// the output contains the success markers rather than trusting the exit code
/// blindly.
fn install_and_launch(udid: &str, app_bundle: &Path) -> Result<()> {
    ensure_ios_deploy_present()?;
    eprintln!(
        "[run-ios --device] ios-deploy --id {udid} --bundle {}",
        app_bundle.display(),
    );
    let out = Command::new("ios-deploy")
        .args(["--id", udid])
        .arg("--bundle")
        .arg(app_bundle)
        .arg("--justlaunch")
        .output()
        .with_context(|| "spawn ios-deploy")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Surface ios-deploy's progress so the user sees install state.
    if !stdout.trim().is_empty() {
        eprint!("{stdout}");
    }

    if installed_ok(&stdout) || installed_ok(&stderr) {
        // ios-deploy may still report a non-zero exit on lldb detach — that's
        // benign once we've seen the success markers.
        return Ok(());
    }
    if out.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "ios-deploy did not report a successful install/launch (exit {}).\n--- stderr ---\n{}",
        out.status,
        stderr.trim(),
    )
}

/// Did ios-deploy's output indicate a completed install + launch? It prints
/// `[100%] Installed package ...` / "InstallComplete" and a launch line; we
/// match the stable markers.
fn installed_ok(output: &str) -> bool {
    (output.contains("Installed package") || output.contains("InstallComplete"))
        || (output.contains("success") && output.contains("Installed"))
}

/// Verify `ios-deploy` is on PATH; emit an actionable error otherwise. We
/// don't vendor it — it's a well-maintained brew formula and shelling out
/// keeps us off its (Objective-C) build chain.
fn ensure_ios_deploy_present() -> Result<()> {
    let found = Command::new("ios-deploy")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if found {
        return Ok(());
    }
    anyhow::bail!(
        "`ios-deploy` is required to install on a physical device but was not found on \
         PATH. Install it with:\n\n    brew install ios-deploy\n\n(We use ios-deploy \
         rather than `xcrun devicectl` because devicectl can't install to older devices \
         like iPhone X / iOS 16.7.)"
    )
}

// ---------------------------------------------------------------------------
// Signing-team resolution
// ---------------------------------------------------------------------------

/// Resolve the Apple Developer team ID to sign with, in priority order:
/// explicit `--team` → `$IDEALYST_DEVELOPMENT_TEAM` → `$DEVELOPMENT_TEAM` →
/// auto-discovery from the first "Apple Development" codesigning identity's
/// certificate (the team is the OU of the cert's subject).
///
/// Public so the CLI front-end can resolve the team before constructing
/// [`DeviceOptions`] and report which source it came from.
pub fn resolve_team(explicit: Option<&str>) -> Result<String> {
    if let Some(t) = explicit {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    for var in ["IDEALYST_DEVELOPMENT_TEAM", "DEVELOPMENT_TEAM"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Ok(v.trim().to_string());
            }
        }
    }
    if let Some(team) = discover_team_from_identities()? {
        return Ok(team);
    }
    anyhow::bail!(
        "could not determine an Apple Developer team ID to sign with. Pass `--team \
         <TEAMID>` (the 10-char team identifier from developer.apple.com / your \
         signing cert), or set $IDEALYST_DEVELOPMENT_TEAM. Found no \"Apple \
         Development\" codesigning identity to auto-detect from — open Xcode > \
         Settings > Accounts and add your Apple ID first."
    )
}

/// Pull a team ID out of the installed codesigning identities. The team is
/// the Organizational Unit (OU) embedded in the cert subject; the identity
/// line itself is `"Apple Development: Name (XXXXXXXXXX)"` where the
/// parenthesized value is the *identity* hash key, NOT the team. To get the
/// team we read the cert and find the OU. We keep it simple: the OU appears
/// in the `security find-certificate -p` PEM's subject only after decoding,
/// which needs openssl. Rather than depend on openssl, we parse the team out
/// of the identity's certificate via `security find-identity` plus a
/// best-effort `openssl` decode when available; if neither yields a team we
/// return `None` and the caller surfaces the `--team` hint.
fn discover_team_from_identities() -> Result<Option<String>> {
    // List codesigning identities. We only need to know one exists + grab
    // its name; the team comes from the cert OU below.
    let out = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .with_context(|| "spawn `security find-identity`")?;
    if !out.status.success() {
        return Ok(None);
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    if !listing.contains("Apple Development") && !listing.contains("iPhone Developer") {
        return Ok(None);
    }
    // Decode the "Apple Development" cert and read the OU (team id) from its
    // subject. `security find-certificate -c <name> -p` emits PEM; pipe to
    // openssl to read the subject. openssl ships with macOS (LibreSSL), so
    // this is normally available.
    if let Some(team) = team_from_cert_subject("Apple Development") {
        return Ok(Some(team));
    }
    if let Some(team) = team_from_cert_subject("iPhone Developer") {
        return Ok(Some(team));
    }
    Ok(None)
}

/// Best-effort: read the OU (team id) from the named codesigning cert's
/// subject using `security find-certificate` piped through `openssl x509`.
/// Returns `None` if either tool is missing or the OU can't be found.
fn team_from_cert_subject(cert_name_prefix: &str) -> Option<String> {
    let pem = Command::new("security")
        .args(["find-certificate", "-a", "-c", cert_name_prefix, "-p"])
        .output()
        .ok()?;
    if !pem.status.success() || pem.stdout.is_empty() {
        return None;
    }
    let mut openssl = Command::new("openssl")
        .args(["x509", "-noout", "-subject"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        openssl.stdin.as_mut()?.write_all(&pem.stdout).ok()?;
    }
    let out = openssl.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let subject = String::from_utf8_lossy(&out.stdout);
    parse_ou_from_subject(&subject)
}

/// Extract the OU value from an openssl `subject=` line. The team id is the
/// OU (e.g. `OU = USC735CN86` or `OU=USC735CN86`). Handles both the modern
/// (`OU = X`) and legacy (`/OU=X`) openssl subject formats.
fn parse_ou_from_subject(subject: &str) -> Option<String> {
    // Modern: "subject=UID = ..., CN = ..., OU = USC735CN86, O = ..."
    for sep in ["OU = ", "OU=", "/OU="] {
        if let Some(idx) = subject.find(sep) {
            let rest = &subject[idx + sep.len()..];
            let val: String = rest
                .chars()
                .take_while(|&c| c != ',' && c != '/' && c != '\n')
                .collect();
            let val = val.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative camera-app framework set (base + AVFoundation pixel
    /// path) for the render tests.
    fn camera_frameworks() -> Vec<Framework> {
        crate::frameworks::collect_ios_frameworks_for_test(&[
            "AVFoundation".into(),
            "CoreMedia".into(),
            "CoreVideo".into(),
        ])
    }

    #[test]
    fn pbxproj_substitutes_every_placeholder() {
        let frameworks = camera_frameworks();
        let rendered = render_pbxproj(&PbxParams {
            app_name: "CameraPreview",
            bundle_id: "ai.truday.idealyst.camerapreview",
            team: "USC735CN86",
            library_search_path: Path::new("/tmp/target/aarch64-apple-ios/release"),
            lib_name: "camera_preview_demo_ios_wrapper",
            frameworks: &frameworks,
        });
        assert!(
            !rendered.contains("{{"),
            "unsubstituted placeholder left in pbxproj:\n{rendered}",
        );
        assert!(rendered.contains("PRODUCT_BUNDLE_IDENTIFIER = ai.truday.idealyst.camerapreview;"));
        assert!(rendered.contains("DEVELOPMENT_TEAM = USC735CN86;"));
        assert!(rendered.contains("-lcamera_preview_demo_ios_wrapper"));
        assert!(rendered.contains("/tmp/target/aarch64-apple-ios/release"));
        assert!(rendered.contains("CameraPreview.app"));
        // The derived frameworks made it into the file (all four sections key
        // off the same names).
        for fw in ["UIKit", "Foundation", "AVFoundation", "CoreMedia"] {
            assert!(
                rendered.contains(&format!("{fw}.framework")),
                "{fw} missing from generated pbxproj",
            );
        }
    }

    /// The weak-linking gotcha (gotcha 1) is encoded as `ATTRIBUTES = (Weak, )`
    /// on the objc2-bound base frameworks (UIKit/Foundation). Without it dyld
    /// aborts at launch on the older OS. CoreGraphics / QuartzCore and every
    /// SDK-declared framework must stay strong (no Weak attr).
    #[test]
    fn pbxproj_weak_links_objc2_frameworks_but_not_coregraphics() {
        let frameworks = camera_frameworks();
        let rendered = render_pbxproj(&PbxParams {
            app_name: "App",
            bundle_id: "ai.example.app",
            team: "TEAM123456",
            library_search_path: Path::new("/lib"),
            lib_name: "app_ios_wrapper",
            frameworks: &frameworks,
        });
        // Only UIKit/Foundation are weak (objc2 back-deploy fix).
        for fw in ["UIKit", "Foundation"] {
            let line = build_file_line(&rendered, fw);
            assert!(
                line.contains("ATTRIBUTES = (Weak, )"),
                "{fw} must be weak-linked (back-deploy crash fix); line: {line}",
            );
        }
        // CoreGraphics/QuartzCore (base, strong) AND the SDK frameworks stay
        // strong — an SDK framework that's an objc2 newer-symbol case isn't in
        // play here, and the C-symbol ones (CoreMedia/CoreVideo) must link hard.
        for fw in ["CoreGraphics", "QuartzCore", "AVFoundation", "CoreMedia", "CoreVideo"] {
            let line = build_file_line(&rendered, fw);
            assert!(
                !line.contains("Weak"),
                "{fw} must stay strong-linked; line: {line}",
            );
        }
    }

    /// Regression for the hardcoded-frameworks bug: a screen-capture app
    /// declares ReplayKit via `[package.metadata.idealyst.ios].frameworks`, and
    /// the generated pbxproj MUST link it (strong) — otherwise
    /// `class!(RPScreenRecorder)` panics at launch. Before the fix the list was
    /// hardcoded to the camera set, so ReplayKit never appeared.
    #[test]
    fn pbxproj_links_replaykit_strong_when_declared() {
        let frameworks = crate::frameworks::collect_ios_frameworks_for_test(&[
            "ReplayKit".into(),
            "CoreMedia".into(),
            "CoreVideo".into(),
        ]);
        let rendered = render_pbxproj(&PbxParams {
            app_name: "ScreenShare",
            bundle_id: "ai.example.screenshare",
            team: "TEAM123456",
            library_search_path: Path::new("/lib"),
            lib_name: "screen_share_ios_wrapper",
            frameworks: &frameworks,
        });
        // Present in all four sections.
        assert!(rendered.contains("ReplayKit.framework in Frameworks"));
        assert!(rendered.contains("path = System/Library/Frameworks/ReplayKit.framework;"));
        // And strong-linked (no Weak attribute).
        let line = build_file_line(&rendered, "ReplayKit");
        assert!(
            !line.contains("Weak"),
            "ReplayKit must strong-link so its class loads at runtime; line: {line}",
        );
    }

    /// The pbxproj signs with AUTOMATIC + a development identity for BOTH
    /// device-install and App Store archive. Forcing `"Apple Distribution"`
    /// while `CODE_SIGN_STYLE = Automatic` makes `xcodebuild archive` fail
    /// ("the … code signing identity has been manually specified") — the
    /// distribution re-sign belongs to `-exportArchive` (`app-store-connect`
    /// method), not the archive. This guards against re-introducing that.
    #[test]
    fn pbxproj_uses_automatic_development_signing_not_manual_distribution() {
        let frameworks = camera_frameworks();
        let rendered = render_pbxproj(&PbxParams {
            app_name: "App",
            bundle_id: "ai.example.app",
            team: "TEAM123456",
            library_search_path: Path::new("/lib"),
            lib_name: "app_ios_wrapper",
            frameworks: &frameworks,
        });
        assert!(
            rendered.contains("CODE_SIGN_STYLE = Automatic"),
            "archive/build must use automatic signing",
        );
        assert!(
            !rendered.contains("Apple Distribution"),
            "must NOT hard-code an Apple Distribution identity under automatic \
             signing — that breaks `xcodebuild archive`; distribution re-signing \
             happens at -exportArchive",
        );
        assert!(rendered.contains("DEVELOPMENT_TEAM = TEAM123456;"));
    }

    /// Find the PBXBuildFile line for a framework in rendered pbxproj.
    fn build_file_line<'a>(rendered: &'a str, fw: &str) -> &'a str {
        rendered
            .lines()
            .find(|l| {
                l.contains(&format!("{fw}.framework in Frameworks")) && l.contains("PBXBuildFile")
            })
            .unwrap_or_else(|| panic!("no build-file line for {fw}"))
    }

    #[test]
    fn parses_modern_device_udid_skipping_mac_and_simulators() {
        let text = "\
== Devices ==
Nicho's iPhone (16.7.10) (00008020-000123456789ABCD)
My Mac (12345678-90AB-CDEF-1234-567890ABCDEF)

== Simulators ==
iPhone 15 (17.0) (ABCDEF01-2345-6789-ABCD-EF0123456789)
";
        assert_eq!(
            parse_first_device_udid(text).as_deref(),
            Some("00008020-000123456789ABCD"),
        );
    }

    #[test]
    fn skips_mac_line_with_only_one_paren_group() {
        // A line with a single parenthesized group (no OS version) is the Mac.
        let text = "== Devices ==\nMy Mac (some-id)\n";
        assert_eq!(parse_first_device_udid(text), None);
    }

    #[test]
    fn no_device_when_only_simulators() {
        let text = "== Devices ==\n\n== Simulators ==\niPhone 15 (17.0) (ABCD-1234)\n";
        assert_eq!(parse_first_device_udid(text), None);
    }

    #[test]
    fn legacy_40hex_udid_recognized() {
        assert!(looks_like_device_udid(
            "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4"
        ));
    }

    #[test]
    fn version_string_is_not_a_udid() {
        assert!(!looks_like_device_udid("16.7.10"));
    }

    #[test]
    fn parses_team_ou_modern_openssl_subject() {
        let subject = "subject=UID = ABC, CN = Apple Development: Jane (XXXX), OU = USC735CN86, O = Jane, C = US\n";
        assert_eq!(parse_ou_from_subject(subject).as_deref(), Some("USC735CN86"));
    }

    #[test]
    fn parses_team_ou_legacy_openssl_subject() {
        let subject = "subject= /UID=ABC/CN=Apple Development: Jane/OU=USC735CN86/O=Jane/C=US\n";
        assert_eq!(parse_ou_from_subject(subject).as_deref(), Some("USC735CN86"));
    }

    /// The device plist must NOT carry the sim-only local-networking ATS
    /// exception (that's `crate::render_info_plist`'s job for the simulator).
    #[test]
    fn device_plist_has_no_local_networking_ats() {
        let m = crate::tests_support::fake_manifest();
        let plist = render_device_info_plist(&m, "Demo", "", "").expect("render");
        assert!(!plist.contains("NSAllowsLocalNetworking"));
        assert!(plist.contains("<key>CFBundleIdentifier</key>"));
    }

    /// The xcodebuild-path plist must carry `CFBundleIconName` (the App Store
    /// requires it alongside the asset catalog, error 90713) and the full
    /// iPad-multitasking orientation set (error 90474 otherwise).
    #[test]
    fn device_plist_has_icon_name_and_all_orientations() {
        let m = crate::tests_support::fake_manifest();
        let plist = render_device_info_plist(&m, "Demo", CFBUNDLE_ICON_NAME_ENTRY, "").expect("render");
        assert!(
            plist.contains("<key>CFBundleIconName</key>\n    <string>AppIcon</string>"),
            "missing CFBundleIconName=AppIcon:\n{plist}",
        );
        for o in [
            "UIInterfaceOrientationPortrait",
            "UIInterfaceOrientationPortraitUpsideDown",
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
        ] {
            assert!(plist.contains(o), "missing orientation {o}");
        }
    }

    /// The pbxproj must reference the `Assets.xcassets` catalog from a
    /// Resources build phase so `actool` compiles the AppIcon (without this,
    /// `CFBundleIconName` points at a catalog the build never produces).
    #[test]
    fn pbxproj_references_asset_catalog_in_resources_phase() {
        let frameworks = camera_frameworks();
        let rendered = render_pbxproj(&PbxParams {
            app_name: "App",
            bundle_id: "ai.example.app",
            team: "TEAM123456",
            library_search_path: Path::new("/lib"),
            lib_name: "app_ios_wrapper",
            frameworks: &frameworks,
        });
        assert!(rendered.contains("PBXResourcesBuildPhase"), "no Resources build phase");
        assert!(
            rendered.contains("Assets.xcassets in Resources"),
            "Assets.xcassets not wired into the Resources phase",
        );
        assert!(
            rendered.contains("lastKnownFileType = folder.assetcatalog"),
            "Assets.xcassets file reference missing",
        );
        assert!(rendered.contains("ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon"));
    }

    /// `CFBundleVersion` reflects the manifest's `build_number` (it used to
    /// be hardcoded `1`). App Store Connect keys on this for build
    /// uniqueness, so a stale hardcoded value would block every upload past
    /// the first.
    #[test]
    fn plist_build_number_is_substituted() {
        let mut m = crate::tests_support::fake_manifest();
        m.app.build_number = "73".to_string();
        let plist = render_device_info_plist(&m, "Demo", "", "").expect("render");
        assert!(
            plist.contains("<key>CFBundleVersion</key>\n    <string>73</string>"),
            "CFBundleVersion must come from build_number:\n{plist}",
        );
    }
}
