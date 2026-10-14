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
//!   ios-deploy --id <UDID> --bundle <app>            (install only)
//!   iOS 17+: xcrun devicectl device process launch --device <UDID>
//!            --terminate-existing <bundle id>         (no debugger)
//!   iOS < 17: no launch — "installed — open the app on the device"
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
//!    `ATTRIBUTES = (Weak, )` on those framework build files. The other base
//!    frameworks (CoreGraphics / QuartzCore / CoreText / CoreFoundation)
//!    stay strong (old/stable). The simulator never hits this
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
//!
//! And the launcher choice — see [`launch_strategy`]. ios-deploy can only
//! launch through lldb (`--justlaunch` = debugserver launch + lldb detach),
//! which on iPadOS 16.7.8 killed the app ~0.1 s after "launched": SIGTRAP in
//! `lldb_image_notifier`, the dyld hook lldb breakpoints and fails to clear
//! on its scripted detach. So the debugger is never used to launch.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{BuildOptions, FrameworkSource, Manifest};

use crate::frameworks::{collect_ios_frameworks, pbx_ids, Framework};
use crate::{
    render_view_controller, targeted_device_family, title_case_for_executable,
    ui_device_family_plist_entry, xml_escape,
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
    /// Force a clean reinstall: pass `--uninstall` to ios-deploy so the old
    /// app (and its container) is removed before the fresh install, instead
    /// of installing over it. Wipes the app's persisted data.
    pub clean: bool,
}

#[derive(Debug)]
pub struct DeviceArtifact {
    /// The signed `.app` bundle xcodebuild produced (under derivedData).
    pub app_bundle: PathBuf,
    /// UDID of the device the app was installed on.
    pub device_udid: String,
    /// Whether the app was launched, or only installed (and why).
    pub launch: DeviceLaunch,
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
    let choice = find_device(opts.udid.as_deref())?;
    let strategy = launch_strategy(choice.device.os_version.as_deref());
    for line in choice.report(&strategy) {
        eprintln!("[run-ios --device] {line}");
    }
    let udid = choice.device.udid.clone();

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

    // ── 7. ios-deploy: install (never launch — see [`launch_strategy`]) ──
    install(&udid, &app_bundle, opts.clean)?;

    // ── 8. Launch without a debugger, where the OS allows it ─────
    let launch = match strategy {
        LaunchStrategy::DeviceCtl => launch_with_devicectl(&udid, manifest.app.require_bundle_id()?)?,
        LaunchStrategy::InstallOnly { reason } => DeviceLaunch::InstalledNotLaunched { reason },
    };

    Ok(DeviceArtifact {
        app_bundle,
        device_udid: udid,
        launch,
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
    // The app's `assets/` → `<project_root>/assets/`, which the pbxproj
    // references as a FOLDER (blue folder) in the Resources phase, so
    // xcodebuild copies it to `<App>.app/assets/` — where the UIKit backend
    // resolves `asset!` paths. Staged (not referenced in place) so symlinks
    // inside the app's dir are dropped instead of copied into a signed
    // bundle; the staging dir persists, so unchanged files aren't rewritten.
    let has_app_assets = stage_xcode_project_assets(project_dir, &project_root)?;
    std::fs::write(
        project_root.join("Info.plist"),
        render_device_info_plist(
            manifest,
            &executable_name,
            CFBUNDLE_ICON_NAME_ENTRY,
            &permission_plist,
        )?,
    )?;

    // Entitlements: universal-link domains need the associated-domains
    // entitlement signed into the app (automatic signing then adds the
    // capability to the provisioning profile). Only written when the app
    // declares domains; a stale file from an earlier build is removed so
    // dropping `domains` really drops the entitlement.
    const ENTITLEMENTS_FILE: &str = "App.entitlements";
    let entitlements_path = project_root.join(ENTITLEMENTS_FILE);
    let entitlements = match manifest.app.links.entitlements_plist() {
        Some(plist) => {
            std::fs::write(&entitlements_path, plist)
                .with_context(|| format!("write {}", entitlements_path.display()))?;
            Some(ENTITLEMENTS_FILE)
        }
        None => {
            let _ = std::fs::remove_file(&entitlements_path);
            None
        }
    };

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
            entitlements,
            app_assets: has_app_assets,
        },
    )?;

    Ok(PreparedProject {
        project_root,
        xcodeproj,
        scheme: executable_name,
    })
}

/// Stage the app's `assets/` at `<project_root>/assets/` for the pbxproj's
/// folder reference. Returns whether there were any (no reference without).
fn stage_xcode_project_assets(project_dir: &Path, project_root: &Path) -> Result<bool> {
    build_ios::app_assets::stage_app_assets(
        project_dir,
        &project_root.join(build_ios::app_assets::ASSETS_DIR),
    )
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
    let bundle_id = manifest.app.require_bundle_id()?;
    let url_types = manifest.app.links.plist_url_types(bundle_id);
    let extra_entries = [icon_entries, permission_entries, url_types.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n    ");
    Ok(INFO_PLIST_TMPL
        .replace("{{APP_NAME}}", &xml_escape(&manifest.app.name))
        .replace("{{BUNDLE_ID}}", &xml_escape(bundle_id))
        .replace("{{EXECUTABLE}}", &xml_escape(executable_name))
        .replace("{{VERSION}}", &xml_escape(&manifest.app.version))
        .replace("{{BUILD_NUMBER}}", &xml_escape(&manifest.app.build_number))
        .replace("{{UI_DEVICE_FAMILY}}", &ui_device_family_plist_entry())
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
    /// The entitlements file (project-relative) to sign with, when the app
    /// declares any — today, universal-link domains
    /// (`com.apple.developer.associated-domains`). `None` ⇒ no
    /// `CODE_SIGN_ENTITLEMENTS`, so a link-less app's provisioning profile
    /// is not asked for a capability it doesn't use.
    entitlements: Option<&'a str>,
    /// Whether `<project_root>/assets/` was staged from the app's
    /// `assets/`. Only then is the folder reference emitted — a reference
    /// to a missing folder fails the build.
    app_assets: bool,
}

/// Fixed IDs for the app-assets folder reference (build file, file ref).
/// Same `A5E7…` family as the asset catalog's, distinct suffixes.
const APP_ASSETS_BUILD_FILE_ID: &str = "A5E700000000000000000004";
const APP_ASSETS_FILE_REF_ID: &str = "A5E700000000000000000005";

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
    let (assets_build_file, assets_file_ref, assets_phase_file, assets_group_child) =
        render_app_assets_sections(params.app_assets);
    PBXPROJ_TMPL
        .replace("{{APP_NAME}}", params.app_name)
        .replace("{{BUNDLE_ID}}", params.bundle_id)
        .replace("{{DEVELOPMENT_TEAM}}", params.team)
        .replace(
            "{{CODE_SIGN_ENTITLEMENTS}}",
            &params
                .entitlements
                .map(|f| format!("\n\t\t\t\tCODE_SIGN_ENTITLEMENTS = \"{f}\";"))
                .unwrap_or_default(),
        )
        .replace(
            "{{LIBRARY_SEARCH_PATH}}",
            &params.library_search_path.display().to_string(),
        )
        .replace("{{LIB_NAME}}", params.lib_name)
        .replace("{{TARGETED_DEVICE_FAMILY}}", &targeted_device_family())
        .replace("{{FRAMEWORK_BUILD_FILES}}", &build_files)
        .replace("{{FRAMEWORK_FILE_REFS}}", &file_refs)
        .replace("{{FRAMEWORK_PHASE_FILES}}", &phase_files)
        .replace("{{FRAMEWORK_GROUP_CHILDREN}}", &group_children)
        .replace("{{APP_ASSETS_BUILD_FILE}}", &assets_build_file)
        .replace("{{APP_ASSETS_FILE_REF}}", &assets_file_ref)
        .replace("{{APP_ASSETS_PHASE_FILE}}", &assets_phase_file)
        .replace("{{APP_ASSETS_GROUP_CHILD}}", &assets_group_child)
}

/// The four pbxproj lines that put `assets/` in the bundle as a folder
/// reference (`lastKnownFileType = folder`): Xcode copies a folder
/// reference's whole tree, structure intact, into the bundle root — unlike
/// a group, which would flatten every file into `<App>.app/`. All empty
/// when the app has no `assets/`.
fn render_app_assets_sections(app_assets: bool) -> (String, String, String, String) {
    if !app_assets {
        return Default::default();
    }
    let (bf, fr) = (APP_ASSETS_BUILD_FILE_ID, APP_ASSETS_FILE_REF_ID);
    (
        format!(
            "\t\t{bf} /* assets in Resources */ = {{isa = PBXBuildFile; fileRef = {fr} /* assets */; }};\n"
        ),
        format!(
            "\t\t{fr} /* assets */ = {{isa = PBXFileReference; lastKnownFileType = folder; path = assets; sourceTree = \"<group>\"; }};\n"
        ),
        format!("\t\t\t\t{bf} /* assets in Resources */,\n"),
        format!("\t\t\t\t{fr} /* assets */,\n"),
    )
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

/// One physical device from `xcrun xctrace list devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub udid: String,
    /// `None` when the person passed a `--udid` that xctrace doesn't list.
    pub name: Option<String>,
    /// The OS version xctrace reports, e.g. `16.7.8`. `None` when unknown.
    pub os_version: Option<String>,
    /// Listed under `== Devices Offline ==` (paired, not reachable now).
    pub offline: bool,
}

/// The device a run resolved to, plus the other connected devices it
/// passed over when the person didn't choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    pub device: Device,
    pub explicit: bool,
    pub other_connected: Vec<Device>,
}

impl DeviceChoice {
    /// Lines to print before the build: the target device, how the app will
    /// be started on it, and (implicit choice only) the other connected
    /// devices and the flag that picks one.
    pub fn report(&self, strategy: &LaunchStrategy) -> Vec<String> {
        let d = &self.device;
        let os = d
            .os_version
            .as_deref()
            .map(|v| format!("iOS {v}"))
            .unwrap_or_else(|| "iOS version unknown".to_string());
        let mut lines = vec![format!(
            "target device: {} ({}), {os}{}",
            d.name.as_deref().unwrap_or("not listed by `xcrun xctrace list devices`"),
            d.udid,
            if d.offline { ", OFFLINE" } else { "" },
        )];
        lines.push(match strategy {
            LaunchStrategy::DeviceCtl => {
                "will install with ios-deploy, then launch with `xcrun devicectl` (no debugger)"
                    .to_string()
            }
            LaunchStrategy::InstallOnly { .. } => {
                "will install only — open the app on the device yourself (no debugger-free \
                 launch on this iOS version)"
                    .to_string()
            }
        });
        if !self.explicit && !self.other_connected.is_empty() {
            let n = self.other_connected.len();
            lines.push(format!(
                "{n} other device{} connected: {}",
                if n == 1 { " is" } else { "s are" },
                self.other_connected
                    .iter()
                    .map(|o| format!("{} ({})", o.name.as_deref().unwrap_or("?"), o.udid))
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
            lines.push("pass `--udid <UDID>` to choose which one to run on".to_string());
        }
        lines
    }
}

/// Run `xcrun xctrace list devices` and choose from it ([`choose_device`]).
/// We parse `xctrace` rather than `devicectl` because `devicectl` marks
/// older devices (iPhone X / iOS 16.7) "unavailable" — `xctrace` still lists
/// them and the classic install path (ios-deploy) works.
fn find_device(wanted: Option<&str>) -> Result<DeviceChoice> {
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
    choose_device(&parse_xctrace_devices(&String::from_utf8_lossy(&out.stdout)), wanted)
}

/// Choose the device to install on.
///
/// - `wanted = Some(s)`: the listed device whose UDID (case-insensitive) or
///   name equals `s`. A UDID xctrace doesn't list is still used as given
///   (xcodebuild / ios-deploy report it if it's wrong), with the OS version
///   unknown — which means install-only.
/// - `wanted = None`: the first ONLINE device. Offline devices (paired but
///   not reachable) are never picked implicitly.
pub fn choose_device(devices: &[Device], wanted: Option<&str>) -> Result<DeviceChoice> {
    let online: Vec<&Device> = devices.iter().filter(|d| !d.offline).collect();
    let device = match wanted.map(str::trim) {
        Some(w) => devices
            .iter()
            .find(|d| d.udid.eq_ignore_ascii_case(w))
            .or_else(|| devices.iter().find(|d| d.name.as_deref() == Some(w)))
            .cloned()
            .unwrap_or_else(|| Device {
                udid: w.to_string(),
                name: None,
                os_version: None,
                offline: false,
            }),
        None => match online.first() {
            Some(d) => (*d).clone(),
            None => anyhow::bail!(
                "no connected iPhone or iPad found. Plug in a device, unlock it, and trust \
                 this Mac (tap \"Trust\" on the device). Verify it shows up under \
                 `xcrun xctrace list devices` (Devices section), then re-run. To target a \
                 specific device, pass `--udid <UDID>`."
            ),
        },
    };
    let other_connected = online
        .into_iter()
        .filter(|d| d.udid != device.udid)
        .cloned()
        .collect();
    Ok(DeviceChoice { device, explicit: wanted.is_some(), other_connected })
}

/// Parse the physical devices out of `xctrace list devices` output:
/// ```text
/// == Devices ==
/// MacBook Pro (12345678-90AB-CDEF-1234-567890ABCDEF)
/// Nicho's iPad (16.7.8) (177c4b6460b71dfd58765484740a9f581419f5ad)
///
/// == Devices Offline ==
/// iPad (18.4.1) (00008103-001A2B3C4D5E802E)
///
/// == Simulators ==
/// iPhone 15 (17.0) (XXXX-...)
/// ```
/// A device line is `Name (version) (udid)`: the LAST parenthesized group
/// is the UDID, the one before it the OS version. The Mac has no version
/// group (and a 4-hyphen UUID), so it's skipped. Parsing stops at the
/// Simulators divider so a simulator is never picked. Pulled out for unit
/// testing without a device.
fn parse_xctrace_devices(text: &str) -> Vec<Device> {
    let mut out = Vec::new();
    let mut offline = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("== Simulators") {
            break;
        }
        if trimmed.starts_with("==") {
            offline = trimmed.contains("Offline");
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let groups: Vec<&str> = collect_paren_groups(trimmed);
        if groups.len() < 2 {
            continue;
        }
        let udid = groups[groups.len() - 1];
        if !looks_like_device_udid(udid) {
            continue;
        }
        let version = groups[groups.len() - 2];
        let name_end = trimmed
            .rfind(&format!("({version})"))
            .unwrap_or(trimmed.len());
        out.push(Device {
            udid: udid.to_string(),
            name: Some(trimmed[..name_end].trim().to_string()),
            os_version: Some(version.to_string()),
            offline,
        });
    }
    out
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

/// How the app is started on the device once installed.
///
/// Why not ios-deploy's launch: ios-deploy has no launch that avoids the
/// debugger. `--justlaunch` (and `--debug`) start the app under debugserver
/// and drive it from lldb, then detach. lldb sets a breakpoint on
/// `lldb_image_notifier` (the hook dyld calls on every image load); when its
/// scripted detach leaves that trap in place the app dies on the next image
/// load — on an iPadOS 16.7.8 iPad it was gone ~0.1 s after ios-deploy
/// printed `success` (crash report: SIGTRAP in `lldb_image_notifier`).
/// `--nolldb` still launches under debugserver. So we install with ios-deploy
/// and never launch through it.
///
/// The debugger-free launch is CoreDevice: `xcrun devicectl device process
/// launch` asks the device to open the app the way SpringBoard would (no
/// `--start-stopped`, no `--console`, nothing attached). CoreDevice only
/// talks to iOS 17+: `devicectl list devices` shows an iOS 16 device as
/// `unavailable` even while connected, and `process launch` on its UDID
/// fails with "The specified device was not found" (CoreDeviceError 1000) —
/// both observed with devicectl 518.33 and a USB-connected iPad on 16.7.16.
/// Below 17 there is no debugger-free remote launch in Apple's tools, so the
/// run installs and says so instead of claiming a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchStrategy {
    /// iOS 17+: launch with `xcrun devicectl device process launch`.
    DeviceCtl,
    /// iOS < 17, or the version is unknown: install only.
    InstallOnly { reason: String },
}

/// The first iOS release CoreDevice (`devicectl`) can launch apps on.
const DEVICECTL_MIN_IOS_MAJOR: u32 = 17;

/// Pick the launch strategy for a device's OS version (as xctrace reports
/// it, e.g. `16.7.8`). See [`LaunchStrategy`] for why.
pub fn launch_strategy(os_version: Option<&str>) -> LaunchStrategy {
    let major = os_version.and_then(|v| v.split('.').next()?.trim().parse::<u32>().ok());
    match (major, os_version) {
        (Some(m), _) if m >= DEVICECTL_MIN_IOS_MAJOR => LaunchStrategy::DeviceCtl,
        (Some(_), Some(v)) => LaunchStrategy::InstallOnly {
            reason: format!(
                "iOS {v} can't be launched from the Mac without a debugger (`xcrun devicectl` \
                 launches iOS {DEVICECTL_MIN_IOS_MAJOR}+ only, and a debugger launch kills the app \
                 when it detaches)"
            ),
        },
        _ => LaunchStrategy::InstallOnly {
            reason: "the device's iOS version is unknown (it isn't listed by `xcrun xctrace \
                     list devices`), so no debugger-free launch was attempted"
                .to_string(),
        },
    }
}

/// What happened after the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceLaunch {
    /// Launched with `devicectl` (no debugger attached).
    Launched,
    /// Installed; the person has to open it on the device.
    InstalledNotLaunched { reason: String },
}

impl DeviceLaunch {
    /// The one-line result the CLI prints.
    pub fn headline(&self) -> &'static str {
        match self {
            DeviceLaunch::Launched => "installed + launched",
            DeviceLaunch::InstalledNotLaunched { .. } => "installed — open the app on the device",
        }
    }

    /// Extra lines explaining an install-only result.
    pub fn details(&self) -> Vec<String> {
        match self {
            DeviceLaunch::Launched => Vec::new(),
            DeviceLaunch::InstalledNotLaunched { reason } => vec![
                format!("not launched: {reason}."),
                "If iOS says \"Untrusted Developer\" when you open it: Settings → General → \
                 VPN & Device Management → your Apple Development profile → Trust."
                    .to_string(),
            ],
        }
    }
}

/// `ios-deploy` arguments for an install that does NOT launch. No
/// `--justlaunch` / `--debug` / `--nolldb` / `--noinstall`: every ios-deploy
/// launch goes through debugserver (see [`LaunchStrategy`]).
fn ios_deploy_install_args(udid: &str, app_bundle: &Path, clean: bool) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> =
        vec!["--id".into(), udid.into(), "--bundle".into(), app_bundle.into()];
    // `--uninstall` makes ios-deploy remove the existing app (and its
    // container) before copying the new bundle, rather than installing over
    // it — the device-side equivalent of the simulator's `--clean` path.
    if clean {
        args.push("--uninstall".into());
    }
    args
}

/// Install via `ios-deploy`. Chosen over `xcrun devicectl device install`
/// because devicectl fails on older devices ("error 1010 usage assertion");
/// ios-deploy uses the classic lockdown/AFC path that works on iPhone X /
/// iOS 16.7. Success is read from the output ([`classify_ios_deploy_install`]).
fn install(udid: &str, app_bundle: &Path, clean: bool) -> Result<()> {
    ensure_ios_deploy_present()?;
    eprintln!(
        "[run-ios --device] ios-deploy --id {udid} --bundle {}{}",
        app_bundle.display(),
        if clean { " --uninstall (--clean)" } else { "" },
    );
    let out = Command::new("ios-deploy")
        .args(ios_deploy_install_args(udid, app_bundle, clean))
        .output()
        .with_context(|| "spawn ios-deploy")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Surface ios-deploy's progress so the user sees install state.
    if !stdout.trim().is_empty() {
        eprint!("{stdout}");
    }
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    match classify_ios_deploy_install(&stdout, &stderr) {
        InstallOutcome::Installed => Ok(()),
        InstallOutcome::Failed { detail } => anyhow::bail!(
            "ios-deploy did not install the app ({}):\n  {detail}",
            out.status
        ),
    }
}

/// What an `ios-deploy` install run did, read from its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    Installed,
    Failed { detail: String },
}

/// Classify an `ios-deploy` install from its output. `Installed package`
/// (or the `InstallComplete` progress step) is the proof of install; the
/// exit status is not consulted — ios-deploy's `[ !! ]` lines carry the
/// reason when it fails.
pub(crate) fn classify_ios_deploy_install(stdout: &str, stderr: &str) -> InstallOutcome {
    let lines: Vec<&str> = stdout.lines().chain(stderr.lines()).map(str::trim).collect();
    if lines
        .iter()
        .any(|l| l.contains("Installed package") || l.contains("InstallComplete"))
    {
        return InstallOutcome::Installed;
    }
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("error:") || l.starts_with("[ !! ]"))
        .collect();
    InstallOutcome::Failed {
        detail: if errors.is_empty() {
            "no `Installed package` line in ios-deploy's output".to_string()
        } else {
            errors.join("\n  ")
        },
    }
}

/// `xcrun` arguments for the debugger-free launch. `--terminate-existing`
/// replaces an instance left running from the previous install; no
/// `--start-stopped` (that waits for a debugger) and no `--console` (that
/// keeps devicectl attached until the app exits).
fn devicectl_launch_args(udid: &str, bundle_id: &str) -> Vec<String> {
    ["devicectl", "device", "process", "launch", "--device", udid, "--terminate-existing", bundle_id]
        .map(String::from)
        .to_vec()
}

fn launch_with_devicectl(udid: &str, bundle_id: &str) -> Result<DeviceLaunch> {
    let args = devicectl_launch_args(udid, bundle_id);
    eprintln!("[run-ios --device] xcrun {}", args.join(" "));
    let out = Command::new("xcrun")
        .args(&args)
        .output()
        .with_context(|| "spawn xcrun devicectl")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !text.trim().is_empty() {
        eprint!("{text}");
    }
    classify_devicectl_launch(&text, out.status.success())
}

/// Phrases iOS uses when it refuses to launch an app whose developer profile
/// the user hasn't trusted ("Unable to launch <id> because it has an invalid
/// code signature, inadequate entitlements or its profile has not been
/// explicitly trusted by the user").
const UNTRUSTED_MARKERS: &[&str] = &[
    "has not been explicitly trusted",
    "invalid code signature",
    "inadequate entitlements",
];

/// Classify a `devicectl device process launch` run. devicectl exits
/// non-zero on failure and prints `ERROR: …` with the underlying
/// FrontBoard / CoreDevice reason, which picks the message:
///
/// - untrusted profile → an error pointing at the Trust setting;
/// - locked device ("could not be, unlocked") → an error saying to unlock;
/// - CoreDevice can't reach the device ("was not found" / "unavailable") →
///   the install still happened, so it's an install-only result, not a
///   failed run;
/// - anything else → an error with devicectl's text.
pub(crate) fn classify_devicectl_launch(output: &str, exit_ok: bool) -> Result<DeviceLaunch> {
    if exit_ok {
        return Ok(DeviceLaunch::Launched);
    }
    let detail = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n  ");
    let lower = output.to_ascii_lowercase();
    if UNTRUSTED_MARKERS.iter().any(|m| lower.contains(m)) {
        anyhow::bail!(
            "the app installed but iOS refused to launch it:\n  {detail}\n\n\
             The device doesn't trust your developer certificate yet. On the device, open \
             Settings → General → VPN & Device Management, select your Apple Development \
             profile, tap Trust, then run this command again."
        );
    }
    if lower.contains("could not be, unlocked") || lower.contains("device is locked") {
        anyhow::bail!(
            "the app installed but the device is locked, so it couldn't be launched. Unlock \
             the device and run this command again."
        );
    }
    if lower.contains("was not found") || lower.contains("unavailable") {
        return Ok(DeviceLaunch::InstalledNotLaunched {
            reason: format!("`xcrun devicectl` couldn't reach the device:\n  {detail}"),
        });
    }
    anyhow::bail!("the app installed but `xcrun devicectl` did not launch it:\n  {detail}")
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
            entitlements: None,
            app_assets: false,
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

    /// `asset!("images/logo.png")` on a device / App Store build reads
    /// `<App>.app/assets/images/logo.png`: the app's `assets/` is staged next
    /// to the `.xcodeproj` and referenced as a FOLDER in the Resources phase,
    /// which xcodebuild copies to the bundle root with its tree intact.
    #[test]
    fn ios_bundle_ships_assets_dir_via_xcode_folder_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        std::fs::create_dir_all(project.join("assets/images")).unwrap();
        std::fs::write(project.join("assets/images/logo.png"), b"png").unwrap();
        let project_root = tmp.path().join("ios-device");
        assert!(stage_xcode_project_assets(&project, &project_root).unwrap());
        assert!(project_root.join("assets/images/logo.png").is_file());

        let frameworks = camera_frameworks();
        let params = |app_assets| PbxParams {
            app_name: "Demo",
            bundle_id: "ai.example.demo",
            team: "USC735CN86",
            library_search_path: Path::new("/tmp/lib"),
            lib_name: "demo_ios_wrapper",
            frameworks: &frameworks,
            entitlements: None,
            app_assets,
        };
        let with = render_pbxproj(&params(true));
        assert!(!with.contains("{{"), "unsubstituted placeholder:\n{with}");
        let file_ref = format!(
            "{APP_ASSETS_FILE_REF_ID} /* assets */ = {{isa = PBXFileReference; \
             lastKnownFileType = folder; path = assets; sourceTree = \"<group>\"; }};"
        );
        assert!(with.contains(&file_ref), "folder reference missing:\n{with}");
        // In the Resources phase (build file) and the main group.
        let resources = &with[with.find("isa = PBXResourcesBuildPhase").unwrap()..];
        let resources = &resources[..resources.find("};").unwrap()];
        assert!(resources.contains(&format!("{APP_ASSETS_BUILD_FILE_ID} /* assets in Resources */")));
        assert!(with.contains(&format!(
            "{APP_ASSETS_BUILD_FILE_ID} /* assets in Resources */ = {{isa = PBXBuildFile; fileRef = {APP_ASSETS_FILE_REF_ID}"
        )));

        let without = render_pbxproj(&params(false));
        assert!(!without.contains("{{"));
        assert!(!without.contains(APP_ASSETS_FILE_REF_ID), "no assets → no folder reference");
    }

    /// Universal links need the associated-domains entitlement SIGNED into
    /// the app: both build configurations must point at the entitlements
    /// file, and a link-less app must not reference one at all.
    #[test]
    fn pbxproj_signs_with_entitlements_only_when_declared() {
        let frameworks = camera_frameworks();
        let params = |entitlements| PbxParams {
            app_name: "Demo",
            bundle_id: "ai.example.demo",
            team: "USC735CN86",
            library_search_path: Path::new("/tmp/lib"),
            lib_name: "demo_ios_wrapper",
            frameworks: &frameworks,
            entitlements,
            app_assets: false,
        };
        let with = render_pbxproj(&params(Some("App.entitlements")));
        assert_eq!(
            with.matches("CODE_SIGN_ENTITLEMENTS = \"App.entitlements\";").count(),
            2,
            "Debug and Release both sign with the entitlements"
        );
        let without = render_pbxproj(&params(None));
        assert!(!without.contains("CODE_SIGN_ENTITLEMENTS"));
        assert!(!without.contains("{{"));
    }

    /// Custom schemes reach the device / App Store plist (the OS routes a
    /// `myapp://` URL only to an app whose Info.plist declares it).
    #[test]
    fn device_plist_declares_custom_url_schemes() {
        let mut m = crate::tests_support::fake_manifest();
        assert!(!render_device_info_plist(&m, "Demo", "", "").unwrap().contains("CFBundleURLTypes"));
        m.app.links.schemes = vec!["demo".into()];
        let plist = render_device_info_plist(&m, "Demo", "", "").unwrap();
        assert!(plist.contains("<key>CFBundleURLTypes</key>"), "{plist}");
        assert!(plist.contains("<string>demo</string>"));
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
            entitlements: None,
            app_assets: false,
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
            entitlements: None,
            app_assets: false,
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
            entitlements: None,
            app_assets: false,
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

    /// Trimmed from real `xcrun xctrace list devices` output (Xcode 26):
    /// the Mac line carries a 4-hyphen UUID and no version, the online
    /// iPad a legacy 40-hex UDID, and paired-but-unreachable devices sit
    /// under `== Devices Offline ==`.
    const XCTRACE: &str = "\
== Devices ==
MacBook Pro (12345678-90AB-CDEF-1234-567890ABCDEF)
Nicho's iPad (16.7.8) (177c4b6460b71dfd58765484740a9f581419f5ad)
Test iPhone (18.5) (00008030-001A2B3C4D5E802E)

== Devices Offline ==
Old iPad (17.7.8) (b306675160c57ebd167081fe3144b0924f971569)

== Simulators ==
iPhone 15 (17.0) (ABCDEF01-2345-6789-ABCD-EF0123456789)
";

    #[test]
    fn parses_devices_skipping_mac_and_simulators() {
        let devices = parse_xctrace_devices(XCTRACE);
        assert_eq!(
            devices.iter().map(|d| d.udid.as_str()).collect::<Vec<_>>(),
            vec![
                "177c4b6460b71dfd58765484740a9f581419f5ad",
                "00008030-001A2B3C4D5E802E",
                "b306675160c57ebd167081fe3144b0924f971569",
            ],
        );
        assert_eq!(devices[0].name.as_deref(), Some("Nicho's iPad"));
        assert_eq!(devices[0].os_version.as_deref(), Some("16.7.8"));
        assert!(!devices[0].offline);
        assert!(devices[2].offline);
    }

    #[test]
    fn skips_mac_line_with_only_one_paren_group() {
        // A line with a single parenthesized group (no OS version) is the Mac.
        assert!(parse_xctrace_devices("== Devices ==\nMy Mac (some-id)\n").is_empty());
    }

    #[test]
    fn no_device_when_only_simulators() {
        let text = "== Devices ==\n\n== Simulators ==\niPhone 15 (17.0) (ABCD-1234)\n";
        assert!(parse_xctrace_devices(text).is_empty());
        assert!(choose_device(&[], None).is_err());
    }

    /// Without `--udid`: the first online device, and the report names the
    /// other connected one plus the flag. Offline devices are never picked.
    #[test]
    fn implicit_device_choice_reports_others_and_the_flag() {
        let devices = parse_xctrace_devices(XCTRACE);
        let choice = choose_device(&devices, None).unwrap();
        assert_eq!(choice.device.udid, "177c4b6460b71dfd58765484740a9f581419f5ad");
        assert_eq!(
            choice.other_connected.iter().map(|d| d.udid.as_str()).collect::<Vec<_>>(),
            vec!["00008030-001A2B3C4D5E802E"],
            "offline devices aren't 'connected'",
        );
        let report = choice.report(&launch_strategy(Some("16.7.8"))).join("\n");
        assert!(
            report.contains("Nicho's iPad (177c4b6460b71dfd58765484740a9f581419f5ad), iOS 16.7.8"),
            "{report}"
        );
        assert!(report.contains("will install only"), "{report}");
        assert!(report.contains("Test iPhone (00008030-001A2B3C4D5E802E)"), "{report}");
        assert!(report.contains("--udid <UDID>"), "{report}");
    }

    #[test]
    fn offline_only_is_not_picked_implicitly() {
        let devices: Vec<Device> =
            parse_xctrace_devices(XCTRACE).into_iter().filter(|d| d.offline).collect();
        assert!(choose_device(&devices, None).is_err());
    }

    #[test]
    fn explicit_device_by_udid_or_name() {
        let devices = parse_xctrace_devices(XCTRACE);
        let by_udid = choose_device(&devices, Some("00008030-001a2b3c4d5e802e")).unwrap();
        assert_eq!(by_udid.device.os_version.as_deref(), Some("18.5"));
        assert!(by_udid.explicit);
        assert_eq!(by_udid.report(&LaunchStrategy::DeviceCtl).len(), 2, "no 'others' noise");
        let by_name = choose_device(&devices, Some("Test iPhone")).unwrap();
        assert_eq!(by_name.device.udid, "00008030-001A2B3C4D5E802E");
    }

    /// A `--udid` xctrace doesn't list is still used, with the OS unknown —
    /// which must mean install-only, never a launch attempt.
    #[test]
    fn unlisted_udid_passes_through_as_install_only() {
        let choice =
            choose_device(&parse_xctrace_devices(XCTRACE), Some("00008110-0000000000000001")).unwrap();
        assert_eq!(choice.device.udid, "00008110-0000000000000001");
        assert_eq!(choice.device.os_version, None);
        assert!(matches!(launch_strategy(None), LaunchStrategy::InstallOnly { .. }));
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
            entitlements: None,
            app_assets: false,
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

    /// The device build declares the same device families as the simulator
    /// build: `TARGETED_DEVICE_FAMILY` in the pbxproj (both configurations)
    /// and `UIDeviceFamily` in the Info.plist, both from
    /// [`crate::DEVICE_FAMILIES`].
    #[test]
    fn device_build_targets_iphone_and_ipad_from_shared_source() {
        let frameworks = camera_frameworks();
        let rendered = render_pbxproj(&PbxParams {
            app_name: "Demo",
            bundle_id: "ai.example.demo",
            team: "ABCDE12345",
            library_search_path: Path::new("/tmp/lib"),
            lib_name: "demo_ios_wrapper",
            frameworks: &frameworks,
            entitlements: None,
            app_assets: false,
        });
        let setting = format!("TARGETED_DEVICE_FAMILY = \"{}\";", crate::targeted_device_family());
        assert_eq!(rendered.matches(&setting).count(), 2, "Debug + Release:\n{rendered}");
        assert!(!rendered.contains("{{"), "unsubstituted placeholder");

        let plist = render_device_info_plist(&crate::tests_support::fake_manifest(), "Demo", "", "")
            .expect("render");
        assert!(plist.contains(&crate::ui_device_family_plist_entry()), "{plist}");
        assert!(!plist.contains("{{"), "unsubstituted placeholder:\n{plist}");
    }

    // ── launch strategy ─────────────────────────────────────────────

    /// Regression: on an iPadOS 16.7.8 iPad, `run ios --device` printed
    /// "installed + launched" and the app died ~0.1 s later (SIGTRAP in
    /// `lldb_image_notifier`) because ios-deploy launched it under lldb.
    /// iOS 16 must get an install-only plan, the ios-deploy call must carry
    /// no launch flag, and the result must not claim a launch.
    #[test]
    fn regression_ios16_device_is_never_launched_through_the_debugger() {
        let strategy = launch_strategy(Some("16.7.8"));
        let LaunchStrategy::InstallOnly { reason } = &strategy else {
            panic!("iOS 16 must not be launched: {strategy:?}");
        };
        assert!(reason.contains("16.7.8"), "{reason}");

        for clean in [false, true] {
            let args: Vec<String> = ios_deploy_install_args("UDID", Path::new("/x/X.app"), clean)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            for launch_flag in
                ["--justlaunch", "-L", "--debug", "-d", "--nolldb", "-N", "--noinstall", "-m"]
            {
                assert!(!args.iter().any(|a| a == launch_flag), "{launch_flag} in {args:?}");
            }
            assert_eq!(args.iter().any(|a| a == "--uninstall"), clean, "{args:?}");
        }

        let result = DeviceLaunch::InstalledNotLaunched { reason: reason.clone() };
        assert_eq!(result.headline(), "installed — open the app on the device");
        let details = result.details().join("\n");
        assert!(
            details.contains("16.7.8") && details.contains("VPN & Device Management"),
            "{details}"
        );
    }

    #[test]
    fn ios17_and_later_launch_with_devicectl() {
        for v in ["17.0", "17.5.1", "18.5", "26.0"] {
            assert_eq!(launch_strategy(Some(v)), LaunchStrategy::DeviceCtl, "{v}");
        }
        for v in ["16.7.16", "15.8", "12.5.7"] {
            assert!(matches!(launch_strategy(Some(v)), LaunchStrategy::InstallOnly { .. }), "{v}");
        }
        assert!(matches!(launch_strategy(Some("garbage")), LaunchStrategy::InstallOnly { .. }));
        assert_eq!(DeviceLaunch::Launched.headline(), "installed + launched");
    }

    #[test]
    fn devicectl_launch_attaches_no_debugger() {
        let args = devicectl_launch_args("00008030-001A2B3C4D5E802E", "com.x");
        assert_eq!(&args[..4], ["devicectl", "device", "process", "launch"]);
        assert!(
            args.windows(2).any(|w| w == ["--device", "00008030-001A2B3C4D5E802E"]),
            "{args:?}"
        );
        assert!(args.contains(&"--terminate-existing".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("com.x"));
        assert!(!args.iter().any(|a| a == "--start-stopped" || a == "--console"), "{args:?}");
    }

    // ── devicectl launch classification ─────────────────────────────

    #[test]
    fn devicectl_success_is_launched() {
        let out = "Launched application with com.x bundle identifier.\n";
        assert_eq!(classify_devicectl_launch(out, true).unwrap(), DeviceLaunch::Launched);
    }

    /// The untrusted-profile refusal must fail with the Trust pointer, never
    /// report a launch.
    #[test]
    fn devicectl_untrusted_profile_is_failure_with_trust_pointer() {
        let out = "ERROR: The application failed to launch. (com.apple.dt.CoreDeviceError error 10002 (0x271A))\n\
                   \x20      BundleIdentifier = com.x\n\
                   ----------------------------------------\n\
                   The operation couldn't be completed. Unable to launch com.x because it has an invalid code \
                   signature, inadequate entitlements or its profile has not been explicitly trusted by the user. \
                   (FBSOpenApplicationErrorDomain error 3 (0x03))\n";
        let err = classify_devicectl_launch(out, false).unwrap_err().to_string();
        assert!(err.contains("VPN & Device Management") && err.contains("Trust"), "{err}");
    }

    #[test]
    fn devicectl_locked_device_is_failure() {
        let out = "ERROR: The application failed to launch. (com.apple.dt.CoreDeviceError error 10002 (0x271A))\n\
                   Unable to launch com.x because the device was not, or could not be, unlocked. \
                   (FBSOpenApplicationErrorDomain error 7 (0x07))\n";
        let err = classify_devicectl_launch(out, false).unwrap_err().to_string();
        assert!(err.contains("device is locked"), "{err}");
    }

    /// devicectl can't see the device (captured from devicectl 518.33 with an
    /// iOS 16 iPad): the install already happened, so this is install-only.
    #[test]
    fn devicectl_device_not_found_is_install_only() {
        let out = "ERROR: The specified device was not found. (Name: 177c4b6460b71dfd58765484740a9f581419f5ad) \
                   (com.apple.dt.CoreDeviceError error 1000 (0x3E8))\n";
        let result = classify_devicectl_launch(out, false).unwrap();
        let DeviceLaunch::InstalledNotLaunched { reason } = &result else {
            panic!("{result:?}");
        };
        assert!(reason.contains("CoreDeviceError error 1000"), "{reason}");
    }

    #[test]
    fn devicectl_other_failure_is_an_error() {
        let out = "ERROR: Something else went wrong. (com.apple.dt.CoreDeviceError error 4 (0x04))\n";
        let err = classify_devicectl_launch(out, false).unwrap_err().to_string();
        assert!(err.contains("Something else went wrong"), "{err}");
    }

    // ── ios-deploy install classification ───────────────────────────
    //
    // Captured from `ios-deploy 1.12.2 --id <udid> --bundle X.app`
    // (paths/UDIDs shortened). Without a launch flag ios-deploy stops after
    // the install phase.

    const DEPLOY_INSTALL_PHASE: &str = "\
[....] Waiting for iOS device to be connected
[....] Using 00008030-001A2B3C4D5E802E (D421AP, iPhone 11 Pro, iphoneos, arm64e, 17.5, 21F79) a.k.a. 'Test iPhone'.
------ Install phase ------
[  0%] Found 00008030-001A2B3C4D5E802E (D421AP, iPhone 11 Pro, iphoneos, arm64e, 17.5, 21F79) a.k.a. 'Test iPhone' connected through USB, beginning install
[  5%] Copying /tmp/build/Build/Products/Debug-iphoneos/X.app/META-INF/ to device
[ 52%] CreatingStagingDirectory
[ 80%] InstallingApplication
[ 95%] GeneratingApplicationMap
[100%] InstallComplete
[100%] Installed package /tmp/build/Build/Products/Debug-iphoneos/X.app
";

    #[test]
    fn ios_deploy_install_success() {
        assert_eq!(classify_ios_deploy_install(DEPLOY_INSTALL_PHASE, ""), InstallOutcome::Installed);
    }

    #[test]
    fn ios_deploy_install_error_is_install_failure() {
        let stdout = "[....] Waiting for iOS device to be connected\n\
                      ------ Install phase ------\n\
                      [  0%] Found 00008030 connected through USB, beginning install\n";
        let stderr = "[ !! ] Error 0xe8008015: A valid provisioning profile for this executable was not found. \
                      AMDeviceSecureInstallApplication(0, device, url, options, install_callback, 0)\n";
        let InstallOutcome::Failed { detail } = classify_ios_deploy_install(stdout, stderr) else {
            panic!("expected Failed");
        };
        assert!(detail.contains("0xe8008015"), "{detail}");
    }

    #[test]
    fn ios_deploy_no_output_is_install_failure() {
        assert!(matches!(classify_ios_deploy_install("", ""), InstallOutcome::Failed { .. }));
    }
}
