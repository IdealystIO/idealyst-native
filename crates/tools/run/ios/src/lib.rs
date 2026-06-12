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
//! Two run modes:
//!
//! - **Local** — the default. The Rust staticlib mounts the user's
//!   `app()` locally; the iOS process is self-contained.
//! - **runtime-server**   — the iOS process is a thin client. The staticlib is
//!   the framework's `runtime-server-ios` crate (a generic runtime-server-client
//!   shell that imports `dev-client + backend-ios-mobile` but **not** the
//!   user's project — see `templates/runtime-server-ios/`). It connects
//!   to a running runtime-server dev-host's WebSocket and replays wire commands
//!   against IosBackend.
//!
//! runtime-server mode shares everything except the staticlib + Swift glue.
//! Bundle ID, app name, splash, simulator orchestration are all
//! identical to Local mode — same project metadata, same flow.
//!
//! This module is the **simulator** path. The **physical-device** path
//! lives in [`device`]: it can't reuse the raw-swiftc step because
//! code-signing requires an Xcode project + `xcodebuild` auto-provisioning
//! (provisioning profile + signing identity + entitlements). The device
//! path generates a `.xcodeproj` directly (no `xcodegen` dependency),
//! builds + signs with `xcodebuild`, and installs with `ios-deploy`. See
//! `device.rs` and [[project_ios_device_deploy_from_cli]].

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use build_ios::{capabilities, BuildOptions, FrameworkSource, Manifest};

/// Physical-device build/sign/install/launch path. The simulator path
/// (the rest of this crate) compiles Swift with raw `swiftc`; the device
/// path needs an Xcode project + `xcodebuild` for code-signing. See
/// [`device::run`] and [[project_ios_device_deploy_from_cli]].
pub mod device;
pub mod frameworks;
/// App Store Connect distribution path for `idealyst publish ios`:
/// distribution-signed archive → exported `.ipa` → optional upload. Reuses
/// the device path's [`device::prepare_xcode_project`] layout machinery; the
/// only divergence is the signing identity and the archive/export commands.
pub mod publish;

/// Embedded Swift sources + plist template. Tiny and identical for
/// every project (modulo splash substitution), so we ship them as
/// `include_str!` rather than generating from scratch.
///
/// `pub(crate)` so the [`device`] submodule reuses the exact same Swift
/// glue + plist template as the simulator path — the device build is the
/// same app, just signed and assembled via xcodebuild instead of swiftc.
pub(crate) const APP_DELEGATE_SWIFT: &str = include_str!("../templates/AppDelegate.swift");
const VIEW_CONTROLLER_LOCAL_SWIFT: &str = include_str!("../templates/ViewController.swift");
const VIEW_CONTROLLER_AAS_SWIFT: &str = include_str!("../templates/ViewControllerRuntimeServer.swift");
pub(crate) const BRIDGING_HEADER_LOCAL_H: &str = include_str!("../templates/BridgingHeader.h");
const BRIDGING_HEADER_AAS_H: &str = include_str!("../templates/BridgingHeaderRuntimeServer.h");
pub(crate) const INFO_PLIST_TMPL: &str = include_str!("../templates/Info.plist.tmpl");

/// The runtime-server-mode iOS staticlib is `backend-ios-mobile` itself, built
/// with its `runtime-server` feature. That feature compiles in the
/// `#[no_mangle] ios_main` / `ios_teardown` symbols defined in
/// `backend_ios_mobile::aas`, which Xcode's linker pulls into the app
/// binary to satisfy Swift's `_ios_main` reference.
///
/// There used to be a thin wrapper crate (`runtime-server-ios`) whose
/// only job was to keep those symbols alive via a link anchor —
/// removed once we confirmed that `backend-ios-mobile`'s own
/// staticlib build already exports them (verified with `nm`).
///
/// The `_LIB` constant is the staticlib *filename* stem (i.e.
/// `libbackend_ios.a`), which is preserved by `[lib] name =
/// "backend_ios"` in that crate so the Xcode link step doesn't have
/// to know about the package rename.
/// The runtime-server shell crate built + linked for `--ios` (default
/// runtime-server mode). This is `backend-ios-rs-shell`, NOT
/// `backend-ios-mobile`: the shell sits ABOVE the backend so it can
/// depend on (and register) the first-party SDK crates
/// (drawer-navigator / codeblock / table), which themselves
/// depend on `backend-ios-mobile`. Bundling those SDK handlers into
/// the fixed RS client is what makes native SDK chrome (the Drawer
/// navigator) render over the wire on device. The shell re-exports the
/// same `ios_main` / `ios_teardown` C symbols and keeps `[lib] name =
/// "backend_ios"`, so the Swift glue + `-l` flag are unchanged.
const IOS_AAS_SHELL_PACKAGE: &str = "backend-ios-rs-shell";
const IOS_AAS_SHELL_LIB: &str = "backend_ios";

/// Whether the iOS process runs the user's app locally or acts as a
/// thin client connected to an runtime-server dev-host.
///
/// runtime-server carries the dev-server URL (`ws://host:port`) which
/// the CLI bakes into `Info.plist` as `IdealystDevEndpoint`. The
/// Swift glue reads that key and passes it to `ios_main`; if the
/// dev-server later restarts on a different port the wrapper has to
/// be rebuilt via `idealyst dev`.
#[derive(Clone, Debug)]
pub enum RunMode {
    Local,
    /// `endpoint`: full `ws://host:port` URL. Empty string means
    /// "no endpoint baked" — useful for one-shot installs of an
    /// AAS binary that will be wired up out-of-band. The wrapper
    /// won't connect to anything in that case.
    RuntimeServer { endpoint: String },
}

impl RunMode {
    fn is_runtime_server(&self) -> bool {
        matches!(self, RunMode::RuntimeServer { .. })
    }

    fn endpoint(&self) -> &str {
        match self {
            RunMode::Local => "",
            RunMode::RuntimeServer { endpoint } => endpoint.as_str(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Build the Rust staticlib in release mode. Swift always
    /// compiles with `-O` regardless — Swift's debug build is
    /// painfully slow on iOS and these sources are trivial.
    pub release: bool,
    /// Selects between the local-mount path (default) and the runtime-server
    /// client path. Both produce a working `.app` for the simulator;
    /// runtime-server just swaps the staticlib + the Swift glue that mounts it.
    pub mode: RunMode,
    /// Where the wrapper Cargo.toml sources framework crates from.
    /// runtime-server mode requires this to be `Workspace` because the runtime-server shell
    /// crate is built directly out of the framework workspace.
    pub source: FrameworkSource,
    /// Cargo features to enable on the build. `idealyst dev` passes
    /// `runtime-core/dev` here so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
    /// Force a clean reinstall: `simctl uninstall` the app before the
    /// fresh `install`, so even SpringBoard's executable cache is dropped.
    /// Wipes the app's persisted data (UserDefaults / Keychain / sandbox).
    /// The default flow already `terminate`s the running process before
    /// installing, which fixes the common "stale running binary" case
    /// without losing data; `clean` is the bigger hammer for when an
    /// install-over still resurfaces the old build.
    pub clean: bool,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// The `.app` bundle that was launched.
    pub app_bundle: PathBuf,
    /// UDID of the simulator the app is running on.
    pub simulator_udid: String,
    /// Mode the app was built in. runtime-server .app bundles only do something
    /// useful if a dev-host is also running on the configured URL.
    pub mode: RunMode,
}

pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = build_ios::parse_manifest(&project_dir)?;

    // ── 1. Produce the staticlib for the chosen mode ─────────────
    let target_triple = build_ios::pick_target(false);
    // Capability-derived Info.plist permission keys. Only the local mode
    // has a wrapper to walk (and only local mode runs the app's own code
    // on-device — runtime-server mode runs it on the dev host, so the
    // device process needs no app permissions).
    let mut permission_plist = String::new();
    // The wrapper manifest (local mode only) is what `cargo metadata` walks to
    // derive the linked frameworks from the dep graph. runtime-server mode runs
    // the app on the dev host, so the device process needs only the base set.
    let mut wrapper_manifest: Option<PathBuf> = None;
    let (lib_dir, lib_name) = match &opts.mode {
        RunMode::Local => {
            let artifact = build_ios::build(
                &project_dir,
                BuildOptions {
                    release: opts.release,
                    device: false,
                    source: opts.source.clone(),
                    user_features: opts.user_features.clone(),
                },
            )?;
            let manifest_path = artifact.wrapper_dir.join("Cargo.toml");
            permission_plist =
                ios_permission_entries(&manifest_path, &manifest.app.permissions);
            wrapper_manifest = Some(manifest_path);
            let dir = artifact
                .staticlib
                .parent()
                .expect("staticlib has parent")
                .to_path_buf();
            let name = format!("{}_ios_wrapper", manifest.lib_name);
            (dir, name)
        }
        RunMode::RuntimeServer { .. } => {
            // runtime-server mode statically links the runtime-server shell crate out of
            // the framework workspace — there's no path through git.
            let workspace_root = opts.source.workspace_root().ok_or_else(|| anyhow::anyhow!(
                "runtime-server mode requires the idealyst framework workspace on disk \
                 (we build `backend-ios-mobile` with the `runtime-server` feature here); \
                 either run from inside a checkout or set IDEALYST_FRAMEWORK_PATH."
            ))?;
            build_runtime_server_shell(workspace_root, target_triple, opts.release)?;
            let profile = if opts.release { "release" } else { "debug" };
            let dir = workspace_root
                .join("target")
                .join(target_triple)
                .join(profile);
            (dir, IOS_AAS_SHELL_LIB.to_string())
        }
    };

    // ── 2. Lay out the bundle dir ────────────────────────────────
    let ios_subdir = if opts.mode.is_runtime_server() { "ios-runtime-server" } else { "ios" };
    let bundle_root = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join(ios_subdir);
    let swift_dir = bundle_root.join("swift");
    let executable_name = title_case_for_executable(&manifest.name);
    let app_bundle = bundle_root.join(format!("{executable_name}.app"));

    if app_bundle.exists() {
        fs::remove_dir_all(&app_bundle)
            .with_context(|| format!("clear stale {} before rebuild", app_bundle.display()))?;
    }
    fs::create_dir_all(&app_bundle).with_context(|| format!("create {}", app_bundle.display()))?;
    fs::create_dir_all(&swift_dir).with_context(|| format!("create {}", swift_dir.display()))?;

    // ── 3. Write Swift sources + bridging header ─────────────────
    fs::write(swift_dir.join("AppDelegate.swift"), APP_DELEGATE_SWIFT)?;
    fs::write(
        swift_dir.join("ViewController.swift"),
        render_view_controller(&manifest, &opts.mode),
    )?;
    fs::write(
        swift_dir.join("BridgingHeader.h"),
        match &opts.mode {
            RunMode::Local => BRIDGING_HEADER_LOCAL_H,
            RunMode::RuntimeServer { .. } => BRIDGING_HEADER_AAS_H,
        },
    )?;

    // ── 4. swiftc: compile Swift + link executable ───────────────
    // Frameworks are DERIVED from the dep graph (base set + each SDK's
    // `[package.metadata.idealyst.ios].frameworks`), not hardcoded — a
    // screen-capture sim build must link ReplayKit or its class fails to load.
    // runtime-server mode has no wrapper, so it links just the base set.
    let frameworks = match &wrapper_manifest {
        Some(m) => frameworks::collect_ios_frameworks(m)?,
        None => frameworks::collect_ios_frameworks(Path::new("/nonexistent"))?,
    };
    let exe_path = app_bundle.join(&executable_name);
    compile_and_link(&swift_dir, &lib_dir, &lib_name, &exe_path, &frameworks)?;

    // ── 5. App-icon PNGs (optional) ──────────────────────────────
    // Generated directly into the .app bundle root because iOS
    // resolves `CFBundleIcons.CFBundleIconFiles` stems against
    // exactly that location, picking the matching `@1x` / `@2x` /
    // `@3x` PNG based on the runtime device density.
    let icon_plist_entries = sync_ios_icons_into_bundle(&project_dir, &app_bundle)?;

    // ── 6. Info.plist + PkgInfo ──────────────────────────────────
    fs::write(
        app_bundle.join("Info.plist"),
        render_info_plist(
            &manifest,
            &executable_name,
            &opts.mode,
            &icon_plist_entries,
            &permission_plist,
        )?,
    )?;
    fs::write(app_bundle.join("PkgInfo"), b"APPL????")?;

    // ── 6. Simulator: boot, then run the (re)install plan ─────────
    let udid = ensure_simulator_booted()?;
    let bundle_id = manifest.app.require_bundle_id()?;
    for step in reinstall_plan(opts.clean) {
        match step {
            SimStep::Terminate => terminate_app(&udid, bundle_id),
            SimStep::Uninstall => uninstall_app(&udid, bundle_id),
            SimStep::Install => install_app(&udid, &app_bundle)?,
            SimStep::Launch => launch_app(&udid, bundle_id)?,
        }
    }

    Ok(RunArtifact {
        app_bundle,
        simulator_udid: udid,
        mode: opts.mode,
    })
}

// ---------------------------------------------------------------------------
// runtime-server shell build
// ---------------------------------------------------------------------------

/// Cargo-build the workspace's iOS runtime-server shell crate for the chosen
/// target. The shell is a fixed, framework-side crate — no wrapper
/// generation here, because the runtime-server client doesn't depend on user
/// code (the user's `app()` runs on the dev-host, not in the iOS
/// process). One staticlib services every project; the per-project
/// metadata flows in through the .app bundle (Info.plist, splash,
/// runtime-server URL) and through the Swift glue.
fn build_runtime_server_shell(workspace_root: &Path, target: &str, release: bool) -> Result<()> {
    let manifest = workspace_root.join("Cargo.toml");
    let mut cmd = Command::new("cargo");
    // The shell crate enables `backend-ios-mobile/runtime-server`
    // unconditionally via its dep declaration, so there's no
    // `--features runtime-server` to pass here (the old
    // `backend-ios-mobile` package had its own such feature).
    cmd.args(["build", "--manifest-path"])
        .arg(&manifest)
        .args(["-p", IOS_AAS_SHELL_PACKAGE, "--target", target]);
    if release {
        cmd.arg("--release");
    }
    eprintln!(
        "[run-ios] cargo build -p {IOS_AAS_SHELL_PACKAGE} --target {target}{}",
        if release { " --release" } else { "" },
    );
    let status = cmd
        .status()
        .with_context(|| "spawn cargo to build the runtime-server shell")?;
    if !status.success() {
        anyhow::bail!("cargo build of {IOS_AAS_SHELL_PACKAGE} exited with {status}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Swift compile + link
// ---------------------------------------------------------------------------

fn compile_and_link(
    swift_dir: &Path,
    lib_dir: &Path,
    lib_name: &str,
    output: &Path,
    frameworks: &[frameworks::Framework],
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

    let mut cmd = Command::new("xcrun");
    cmd.args(["-sdk", "iphonesimulator", "swiftc"])
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
        .arg(format!("-l{lib_name}"));

    // Frameworks the Rust staticlib needs at link time, DERIVED from the dep
    // graph (base set + each SDK's declared frameworks) rather than hardcoded.
    // UIKit/Foundation must weak-link (objc2 back-deploy fix — see
    // `frameworks`); CoreGraphics/QuartzCore and the SDK frameworks
    // (CoreMedia/CoreVideo C-symbol path, ReplayKit/AVFoundation classes)
    // strong-link. `-weak_framework` is the swiftc/ld spelling of the pbxproj
    // `ATTRIBUTES = (Weak, )`.
    for fw in frameworks {
        if fw.weak {
            cmd.args(["-Xlinker", "-weak_framework", "-Xlinker", &fw.name]);
        } else {
            cmd.args(["-framework", &fw.name]);
        }
    }

    let status = cmd.status().with_context(|| "spawn xcrun swiftc")?;

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

/// Generate the iOS icon set into `app_bundle` and return the
/// CFBundleIcons XML snippet to splice into Info.plist. No-op when
/// the project has no `[package.metadata.idealyst.app.icon]` block
/// — returns an empty string and writes nothing.
pub(crate) fn sync_ios_icons_into_bundle(project_dir: &Path, app_bundle: &Path) -> Result<String> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        return Ok(String::new());
    };
    let block = config.resolved_for(icon_gen::Target::Ios);
    let Some(outs) = icon_gen::sync_ios_icons(Some(&block), app_bundle)? else {
        return Ok(String::new());
    };

    // CFBundleIconFiles is a stem list — iOS auto-resolves the
    // matching `@2x` / `@3x` file based on device density. Dedupe
    // here so we don't list `AppIcon60x60` twice for its 2x and 3x
    // entries.
    let mut stems: Vec<&str> = outs.entries.iter().map(|e| e.plist_stem.as_str()).collect();
    stems.sort();
    stems.dedup();
    let stem_xml: String = stems
        .iter()
        .map(|s| format!("            <string>{}</string>", xml_escape(s)))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "<key>CFBundleIcons</key>\n    <dict>\n        \
         <key>CFBundlePrimaryIcon</key>\n        <dict>\n            \
         <key>CFBundleIconFiles</key>\n            <array>\n{stem_xml}\n            \
         </array>\n        </dict>\n    </dict>"
    ))
}

/// Generate an `Assets.xcassets/AppIcon` asset catalog under `dest_dir` (the
/// xcodebuild project root). Unlike [`sync_ios_icons_into_bundle`] (loose
/// PNGs for the hand-assembled simulator bundle), the catalog is what App
/// Store ingestion requires — `actool` compiles it and sets
/// `CFBundleIconName`. ALWAYS writes a catalog: if the project declares no
/// `[package.metadata.idealyst.app.icon]` block, every slot gets a generated
/// placeholder, so a valid icon ships by default. Used by the device-install
/// and App Store archive paths via [`device::prepare_xcode_project`].
pub(crate) fn sync_ios_asset_catalog_into_project(
    project_dir: &Path,
    dest_dir: &Path,
) -> Result<()> {
    let config = icon_gen::load_config_from_manifest(project_dir)?;
    let block = config.map(|c| c.resolved_for(icon_gen::Target::Ios));
    icon_gen::sync_ios_asset_catalog(block.as_ref(), dest_dir)?;
    Ok(())
}

/// Discover the capabilities the app's dependency graph declares, resolve
/// them against the app's reason strings, and render the iOS Info.plist
/// usage-description entries. Warnings (generic-reason fallback, unknown
/// capability) and a one-line-per-permission report are printed so an
/// auto-added permission is never invisible. A discovery error degrades to
/// no entries with a warning rather than failing the run — a permission
/// gap shouldn't block `idealyst dev`.
pub(crate) fn ios_permission_entries(
    wrapper_manifest: &Path,
    app_reasons: &std::collections::BTreeMap<String, String>,
) -> String {
    let discovered = match capabilities::discover(wrapper_manifest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: could not discover app capabilities: {e}");
            return String::new();
        }
    };
    if discovered.is_empty() {
        return String::new();
    }
    let resolved = capabilities::resolve(&discovered, app_reasons);
    for w in &resolved.warnings {
        eprintln!("warning: {w}");
    }
    for r in &resolved.report {
        println!("  iOS permission: {r}");
    }
    resolved
        .ios_plist
        .iter()
        .map(|(k, v)| {
            format!(
                "<key>{}</key>\n    <string>{}</string>",
                xml_escape(k),
                xml_escape(v)
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

fn render_info_plist(
    manifest: &Manifest,
    executable_name: &str,
    mode: &RunMode,
    icon_entries: &str,
    permission_entries: &str,
) -> Result<String> {
    // No more network-discovery advertisement: the Robot bridge writes
    // a per-process `~/.idealyst/apps/<name>-<pid>.json` registration
    // file the MCP server scans, and the dev-server URL is baked into
    // `IdealystDevEndpoint` for the runtime-server client. iOS no
    // longer needs `NSBonjourServices` / `NSLocalNetworkUsageDescription`
    // — every previously broadcast-driven flow now goes through
    // file-based discovery.

    let endpoint_entry = match mode {
        RunMode::Local => String::new(),
        RunMode::RuntimeServer { .. } => format!(
            "<key>IdealystDevEndpoint</key>\n    <string>{endpoint}</string>",
            endpoint = xml_escape(mode.endpoint()),
        ),
    };
    // ATS local-networking exception. App Transport Security otherwise blocks
    // cleartext HTTP, which stops a dev build from reaching a server function /
    // `#[sse]` host running over plain `http://` on the host machine (the iOS
    // simulator shares the host's loopback, so `http://127.0.0.1:<port>` is the
    // usual dev target). `NSAllowsLocalNetworking` relaxes ATS for loopback and
    // `.local` hosts ONLY — it does not permit arbitrary-internet cleartext.
    //
    // This is dev-scoped by construction: `render_info_plist` only runs in the
    // `idealyst run`/`dev` path (keyed off `RunMode`), never release/distribution
    // packaging, so production builds keep full ATS.
    let dev_ats_entry = "<key>NSAppTransportSecurity</key>\n    <dict>\n        \
        <key>NSAllowsLocalNetworking</key>\n        <true/>\n    </dict>";
    // The Info.plist template has one `{{EXTRA_PLIST_ENTRIES}}`
    // splice point, so multiple plist additions (icon block,
    // runtime-server endpoint, dev ATS exception) get concatenated here.
    // Newline + 4-space indent keeps the rendered plist consistent with the
    // template's existing entries.
    let extra_entries = [
        icon_entries,
        endpoint_entry.as_str(),
        permission_entries,
        dev_ats_entry,
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join("\n    ");
    // Resolve bundle id eagerly so misconfigured manifests fail before
    // we render the plist (used to gate-keep IdealystAppId; now just
    // a sanity check).
    let _ = manifest.app.require_bundle_id()?;
    Ok(INFO_PLIST_TMPL
        .replace("{{APP_NAME}}", &xml_escape(&manifest.app.name))
        .replace("{{BUNDLE_ID}}", &xml_escape(manifest.app.require_bundle_id()?))
        .replace("{{EXECUTABLE}}", &xml_escape(executable_name))
        .replace("{{VERSION}}", &xml_escape(&manifest.app.version))
        .replace("{{BUILD_NUMBER}}", &xml_escape(&manifest.app.build_number))
        .replace("{{EXTRA_PLIST_ENTRIES}}", &extra_entries))
}

/// Fill the ViewController template with the project's splash
/// settings (and, in runtime-server mode, the dev-server URL). Title text is
/// Swift-string-escaped (we need to survive embedding in a `"..."`
/// literal, so backslashes and quotes get escaped). Colors are
/// validated by Swift's own `Scanner` at runtime — passing a
/// malformed hex shows magenta so it's obvious.
pub(crate) fn render_view_controller(manifest: &Manifest, mode: &RunMode) -> String {
    let template = match mode {
        RunMode::Local => VIEW_CONTROLLER_LOCAL_SWIFT,
        RunMode::RuntimeServer { .. } => VIEW_CONTROLLER_AAS_SWIFT,
    };
    // Splash substitution only — the runtime-server URL travels via
    // Info.plist's `IdealystDevEndpoint`, set by `render_info_plist`.
    template
        .replace("{{SPLASH_BG}}", &swift_escape(&manifest.app.splash.background))
        .replace("{{SPLASH_TITLE}}", &swift_escape(&manifest.app.splash.title))
        .replace(
            "{{SPLASH_TITLE_COLOR}}",
            &swift_escape(&manifest.app.splash.title_color),
        )
        .replace(
            "{{SPLASH_DURATION_MS}}",
            &manifest.app.splash.duration_ms.to_string(),
        )
}

fn swift_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn xml_escape(s: &str) -> String {
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

/// One step in the simulator (re)install sequence run before the app comes
/// up. Modeled as data so [`reinstall_plan`] is a pure, testable function.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum SimStep {
    Terminate,
    Uninstall,
    Install,
    Launch,
}

/// Ordered simulator operations for a (re)install.
///
/// We ALWAYS `Terminate` first: `simctl install` does replace the on-disk
/// binary, but if the previous instance is still running, `launch` just
/// re-foregrounds the OLD process — the exact "I killed it by hand and it
/// still didn't update" trap (killing from the UI doesn't help; SpringBoard
/// keeps the process). `clean` additionally `Uninstall`s so SpringBoard
/// can't serve a cached executable at all, at the cost of the app's data.
fn reinstall_plan(clean: bool) -> Vec<SimStep> {
    let mut steps = vec![SimStep::Terminate];
    if clean {
        steps.push(SimStep::Uninstall);
    }
    steps.push(SimStep::Install);
    steps.push(SimStep::Launch);
    steps
}

/// Terminate any running instance of `bundle_id` on the simulator. Best
/// effort: `simctl terminate` exits non-zero when the app isn't running
/// ("found nothing to terminate"), which is the common case and not an
/// error for us — so we don't propagate the status.
fn terminate_app(udid: &str, bundle_id: &str) {
    eprintln!("[run-ios] simctl terminate {bundle_id} on {udid}");
    let _ = Command::new("xcrun")
        .args(["simctl", "terminate", udid, bundle_id])
        .output();
}

/// Uninstall `bundle_id` from the simulator so the next install is fully
/// fresh (clears SpringBoard's executable cache AND the app sandbox). Best
/// effort: a non-zero exit when the app isn't installed is expected, so we
/// log but don't fail the run.
fn uninstall_app(udid: &str, bundle_id: &str) {
    eprintln!("[run-ios] simctl uninstall {bundle_id} on {udid} (--clean)");
    let _ = Command::new("xcrun")
        .args(["simctl", "uninstall", udid, bundle_id])
        .output();
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
pub(crate) fn title_case_for_executable(s: &str) -> String {
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

/// Shared test fixtures for this crate, reused by the [`device`] submodule's
/// unit tests. Only compiled under `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod tests_support {
    use build_ios::{AppMetadata, Manifest, SplashConfig, WebMetadata};

    /// A minimal manifest with a bundle id and the splash disabled —
    /// enough to exercise plist / project rendering without touching disk.
    pub(crate) fn fake_manifest() -> Manifest {
        Manifest {
            name: "demo".to_string(),
            lib_name: "demo".to_string(),
            app: AppMetadata {
                name: "Demo".to_string(),
                bundle_id: Some("ai.example.demo".to_string()),
                version: "0.0.1".to_string(),
                build_number: "1".to_string(),
                splash: SplashConfig {
                    background: "#000000".to_string(),
                    title: "Demo".to_string(),
                    title_color: "#ffffff".to_string(),
                    duration_ms: 0,
                },
                targets: Vec::new(),
                server_bin: None,
                server_manifest: None,
                server_port: 3000,
                web: WebMetadata::default(),
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_ios::{AppMetadata, Manifest, SplashConfig};

    fn fake_manifest() -> Manifest {
        Manifest {
            name: "demo".to_string(),
            lib_name: "demo".to_string(),
            app: AppMetadata {
                name: "Demo".to_string(),
                bundle_id: Some("ai.example.demo".to_string()),
                version: "0.0.1".to_string(),
                build_number: "1".to_string(),
                splash: SplashConfig {
                    background: "#000000".to_string(),
                    title: "Demo".to_string(),
                    title_color: "#ffffff".to_string(),
                    duration_ms: 0,
                },
                targets: Vec::new(),
                server_bin: None,
                server_manifest: None,
                server_port: 3000,
                web: Default::default(),
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }

    /// The dev/run Info.plist must carry the `NSAllowsLocalNetworking` ATS
    /// exception so a dev build can reach a cleartext `http://127.0.0.1` server
    /// function / `#[sse]` host (the iOS simulator shares the host loopback).
    /// It must NOT blanket-allow arbitrary cleartext loads — that would relax
    /// ATS for the whole internet, not just local dev.
    #[test]
    fn dev_plist_allows_local_networking_but_not_arbitrary_loads() {
        for mode in [
            RunMode::Local,
            RunMode::RuntimeServer {
                endpoint: "ws://127.0.0.1:4000".to_string(),
            },
        ] {
            let plist = render_info_plist(&fake_manifest(), "demo", &mode, "", "")
                .expect("render plist");
            assert!(
                plist.contains("<key>NSAppTransportSecurity</key>"),
                "ATS dict missing for {mode:?}"
            );
            assert!(
                plist.contains("<key>NSAllowsLocalNetworking</key>"),
                "local-networking exception missing for {mode:?}"
            );
            assert!(
                !plist.contains("NSAllowsArbitraryLoads"),
                "must not blanket-allow arbitrary cleartext for {mode:?}"
            );
            // Exactly one ATS dict — no accidental duplication across the
            // concatenated extra-entry list.
            assert_eq!(plist.matches("NSAppTransportSecurity").count(), 1);
        }
    }

    /// Default (`--clean` off) must still terminate the running instance
    /// before installing, or `launch` re-foregrounds the stale process and
    /// the app appears not to update even after a manual kill. Order matters:
    /// Terminate strictly precedes Install, which precedes Launch.
    #[test]
    fn reinstall_plan_default_terminates_before_install() {
        assert_eq!(
            reinstall_plan(false),
            vec![SimStep::Terminate, SimStep::Install, SimStep::Launch],
        );
    }

    /// `--clean` inserts an Uninstall between Terminate and Install so
    /// SpringBoard's cached executable is dropped, guaranteeing a fresh binary.
    #[test]
    fn reinstall_plan_clean_uninstalls_between_terminate_and_install() {
        assert_eq!(
            reinstall_plan(true),
            vec![
                SimStep::Terminate,
                SimStep::Uninstall,
                SimStep::Install,
                SimStep::Launch,
            ],
        );
    }
}
