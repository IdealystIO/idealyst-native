//! Direct Android app builder + launcher for `idealyst run android`.
//!
//! No Gradle. We invoke the same command-line tools Gradle does:
//!
//! ```text
//!   build-android::build  → Rust cdylib (.so for arm64-v8a)
//!   javac                 → MainActivity.java + NativeBridge.java → .class
//!   d8                    → .class → classes.dex
//!   aapt2 link            → manifest → unsigned.apk
//!   zip                   → splice classes.dex + lib/arm64-v8a/<so> into apk
//!   zip                   → splice the app's assets/ as apk assets/<path>
//!   zipalign              → page-align the .so for direct mmap
//!   apksigner             → sign (release key or the debug keystore)
//!   emulator -avd / adb   → boot a sim if none, install, launch
//! ```
//!
//! [`package`] is everything up to the signed APK; [`run`] packages and
//! then installs + launches. `idealyst build --android` calls [`package`]
//! for a release APK.
//!
//! Why no Gradle: same reason we skipped Xcode projects. The APK is
//! a build artifact derivable from the user's platform-agnostic
//! crate + a few lines of project metadata. A Gradle project would
//! be a regenerated artifact in any case; cutting it out drops a
//! large dependency surface and makes builds noticeably faster.
//!
//! Signing and `versionCode` come from the app's
//! `[package.metadata.idealyst.app.android]` (see `build_ios::android_app`):
//! a configured release keystore signs every build of the app — `run` too,
//! because Android refuses to update an install signed with a different
//! certificate — and the debug keystore is the default when none is set.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use build_ios::android_app::AndroidAppMetadata;
use build_ios::{capabilities, FrameworkSource, Manifest};

mod kotlin_runtime;
mod maven;
pub mod provisioning;
mod sdk_manifest;

use sdk_manifest::SdkManifestContributions;

const MAIN_ACTIVITY_LOCAL_JAVA: &str = include_str!("../templates/MainActivity.java");
const NATIVE_BRIDGE_LOCAL_JAVA: &str = include_str!("../templates/NativeBridge.java");
const ANDROID_MANIFEST_LOCAL_XML: &str = include_str!("../templates/AndroidManifest.xml");
const MAIN_ACTIVITY_AAS_JAVA: &str = include_str!("../templates/MainActivityRuntimeServer.java");
const NATIVE_BRIDGE_AAS_JAVA: &str = include_str!("../templates/NativeBridgeRuntimeServer.java");
const ANDROID_MANIFEST_AAS_XML: &str = include_str!("../templates/AndroidManifestAas.xml");

const MIN_SDK_VERSION: u32 = 21;
const TARGET_SDK_VERSION: u32 = 34;

/// Default emulator to boot if no device is attached and the user
/// hasn't specified one explicitly. Matches what Android Studio
/// hands out from the AVD wizard for a typical Pixel-class profile.
const DEFAULT_AVD_PREFIX: &str = "Pixel";

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Build the Rust cdylib in release mode.
    pub release: bool,
    /// Specific AVD name to boot if no device is attached. When
    /// `None`, we pick the first available AVD whose name starts
    /// with `DEFAULT_AVD_PREFIX`, falling back to the first AVD.
    pub avd: Option<String>,
    /// Whether the Android process runs the user's `app()` locally
    /// or acts as a thin client connected to an runtime-server dev-host.
    pub mode: RunMode,
    /// Where the wrapper Cargo.toml sources framework crates from.
    /// runtime-server mode requires `Workspace`.
    pub source: FrameworkSource,
    /// runtime-server-mode only: the dev-server's WebSocket port on the host
    /// Mac, resolved from `IDEALYST_RUNTIME_SERVER_PORT_FILE` by the CLI
    /// before this call. When set, we (a) run
    /// `adb reverse tcp:<port> tcp:<port>` so the emulator's localhost
    /// reaches the host's port, and (b) bake `ws://127.0.0.1:<port>`
    /// into the APK's manifest as `IdealystRuntimeServerUrl`. The
    /// Java side reads that value at boot and connects directly;
    /// missing meta-data is treated as a build error (there's no
    /// fallback discovery).
    pub runtime_server_port: Option<u16>,
    /// Cargo features to enable on the build. `idealyst dev` passes the
    /// wrapper's `dev` feature (→ `runtime-core/dev`) here so the Robot
    /// bridge auto-starts.
    pub user_features: Vec<String>,
    /// Robot relay URL (`ws://127.0.0.1:<port>`) the dev process hosts. When
    /// set, we (a) `adb reverse tcp:<port> tcp:<port>` so the emulator's
    /// localhost reaches the host relay, and (b) bake it into the manifest as
    /// `IdealystRobotRelayUrl` — MainActivity hands it to Rust before `attach`
    /// so the bridge dials out instead of self-hosting. Empty/None leaves the
    /// app self-hosting an on-device TCP bridge (the prior behavior).
    pub robot_relay_url: Option<String>,
}

/// Mirrors `run-ios::RunMode` — same trade-offs (local self-contained
/// process vs. dev-host wire client). runtime-server mode swaps both the
/// generated cdylib (`backend-android` with the `runtime-server` feature)
/// and the Java glue (manifest carries `IdealystRuntimeServerUrl`,
/// MainActivity calls `attachRuntimeServerUrl`, runs a `Handler` tick
/// into `drainRuntimeServer`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunMode {
    #[default]
    Local,
    RuntimeServer,
}

impl RunMode {
    fn is_runtime_server(self) -> bool {
        matches!(self, RunMode::RuntimeServer)
    }
}

/// What [`package`] builds. [`RunOptions`] minus the device choice, plus
/// whether the manifest is the development one.
#[derive(Clone, Debug)]
pub struct PackageOptions {
    pub release: bool,
    pub mode: RunMode,
    pub source: FrameworkSource,
    pub runtime_server_port: Option<u16>,
    pub user_features: Vec<String>,
    pub robot_relay_url: Option<String>,
    /// Allow cleartext HTTP (`android:usesCleartextTraffic`). On for
    /// `idealyst run`/`dev`, so a dev build reaches an `http://` server on the
    /// developer's machine; off for a distribution APK, which keeps Android's
    /// secure default.
    pub cleartext: bool,
}

/// A signed APK, ready to install.
#[derive(Debug)]
pub struct PackagedApk {
    pub apk: PathBuf,
    /// The app's package name (`manifest.app.bundle_id`).
    pub package: String,
    pub version_code: u32,
    pub version_name: String,
}

#[derive(Debug)]
pub struct RunArtifact {
    pub apk: PathBuf,
    /// adb serial of the device the APK was installed on
    /// (`emulator-5554`, a real device's USB serial, etc.).
    pub serial: String,
}

pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let packaged = package(
        project_dir,
        &PackageOptions {
            release: opts.release,
            mode: opts.mode,
            source: opts.source.clone(),
            runtime_server_port: opts.runtime_server_port,
            user_features: opts.user_features.clone(),
            robot_relay_url: opts.robot_relay_url.clone(),
            cleartext: true,
        },
    )?;
    let sdk = find_android_sdk()?;
    let adb = sdk.join("platform-tools/adb");
    let signed_apk = packaged.apk;

    // ── Ensure a device is online; boot an emulator if not ───────
    let serial = ensure_device(&adb, &sdk, opts.avd.as_deref())?;
    eprintln!("[run-android] using device {serial}");

    // ── Optional `adb reverse` so the device's localhost reaches the
    //    host's dev-server. Only meaningful in runtime-server mode; the
    //    same port we bake into manifest meta-data as
    //    `IdealystRuntimeServerUrl`. Harmless on physical devices because
    //    USB ADB supports reverse tunnels too. ─
    if let Some(port) = opts.runtime_server_port {
        adb_reverse(&adb, &serial, port)?;
    }
    // Same tunnel for the robot relay: the device dials `ws://127.0.0.1:<port>`
    // (baked into the manifest) and `adb reverse` forwards it to the host
    // relay. Parse the port out of the relay URL.
    if let Some(url) = &opts.robot_relay_url {
        if let Some(port) = url.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
            adb_reverse(&adb, &serial, port)?;
        }
    }

    // ── adb install + launch ────────────────────────────────────
    adb_install(&adb, &serial, &signed_apk)?;
    let component = format!("{}/.MainActivity", packaged.package);
    adb_launch(&adb, &serial, &component)?;

    // Surface the emulator window. Matches the iOS launcher, which
    // does `open -a Simulator` so the user doesn't have to alt-tab
    // out of the terminal to see the result. No-op for physical
    // devices (the qemu-system process doesn't exist) and a no-op
    // on non-macOS hosts.
    bring_emulator_to_front();

    Ok(RunArtifact {
        apk: signed_apk,
        serial,
    })
}

/// Build the Rust cdylib and assemble, align and sign the APK. No device
/// involved.
pub fn package(project_dir: &Path, opts: &PackageOptions) -> Result<PackagedApk> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = build_ios::parse_manifest(&project_dir)?;

    // Resolve signing and the versionCode before the cdylib build, so
    // a missing keystore password or an unorderable version fails fast.
    let bundle_id = manifest.app.require_bundle_id()?;
    let signing = resolve_signing(&project_dir, &manifest.app.android, &home_dir()?, |k| {
        std::env::var_os(k)
    })?;
    let version_code = manifest
        .app
        .android
        .resolved_version_code(&manifest.app.version)?;

    // ── 1. Build the Rust cdylib ─────────────────────────────────
    let so = build_android::build(
        &project_dir,
        build_android::BuildOptions {
            release: opts.release,
            api_level: 21,
            mode: match opts.mode {
                RunMode::Local => build_android::BuildMode::Local,
                RunMode::RuntimeServer => build_android::BuildMode::RuntimeServer,
            },
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
        },
    )?;

    // ── 2. Resolve Android SDK + tools ──────────────────────────
    let sdk = find_android_sdk()?;
    let build_tools = pick_latest_dir(&sdk.join("build-tools"))
        .context("no build-tools installed (run `sdkmanager 'build-tools;36.0.0'`)")?;
    let platform = pick_latest_platform(&sdk.join("platforms"))
        .context("no numbered Android platform installed (run `sdkmanager 'platforms;android-35'`)")?;
    let android_jar = platform.join("android.jar");
    if !android_jar.is_file() {
        anyhow::bail!(
            "expected {} to exist; install via `sdkmanager 'platforms;android-X'`",
            android_jar.display(),
        );
    }
    // ── 3. Lay out build dir ─────────────────────────────────────
    // runtime-server and Local builds live in sibling dirs so their (different)
    // staged Java sources + APKs don't stomp each other.
    let app_subdir = if opts.mode.is_runtime_server() {
        "android-runtime-server/app"
    } else {
        "android/app"
    };
    let build_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join(app_subdir);
    if build_dir.exists() {
        fs::remove_dir_all(&build_dir)
            .with_context(|| format!("clear stale {}", build_dir.display()))?;
    }
    fs::create_dir_all(&build_dir)?;

    // ── 4. Generate Java sources + manifest ──────────────────────
    let java_dir = build_dir.join("java");
    let java_pkg_dir = java_dir.join(manifest.app.require_bundle_id()?.replace('.', "/"));
    fs::create_dir_all(&java_pkg_dir)?;
    // The cdylib name differs by mode because each wrapper produces
    // its own `.so` and we want the runtime-server one to be obviously
    // distinguishable in the APK's `lib/<abi>/` directory.
    let lib_name = if opts.mode.is_runtime_server() {
        format!("{}_android_aas_wrapper", manifest.lib_name)
    } else {
        format!("{}_android_wrapper", manifest.lib_name)
    };
    let (main_activity_tmpl, native_bridge_tmpl, manifest_tmpl) = if opts.mode.is_runtime_server() {
        (
            MAIN_ACTIVITY_AAS_JAVA,
            NATIVE_BRIDGE_AAS_JAVA,
            ANDROID_MANIFEST_AAS_XML,
        )
    } else {
        (
            MAIN_ACTIVITY_LOCAL_JAVA,
            NATIVE_BRIDGE_LOCAL_JAVA,
            ANDROID_MANIFEST_LOCAL_XML,
        )
    };
    fs::write(
        java_pkg_dir.join("MainActivity.java"),
        render(main_activity_tmpl, &[
            ("PACKAGE", manifest.app.require_bundle_id()?),
            ("LIB_NAME", &lib_name),
        ]),
    )?;
    fs::write(
        java_pkg_dir.join("NativeBridge.java"),
        render(native_bridge_tmpl, &[("PACKAGE", manifest.app.require_bundle_id()?)]),
    )?;
    let manifest_xml = build_dir.join("AndroidManifest.xml");
    // The runtime-server manifest carries an `IdealystAppId` meta-data entry
    // (Bonjour filter key) plus an optional `IdealystRuntimeServerUrl` override.
    // When `runtime_server_port` is set, we bake `ws://127.0.0.1:<port>` so the
    // emulator skips discovery and connects directly through the
    // `adb reverse` tunnel set up just before install. Empty string
    // means "fall through to Bonjour at runtime" — right shape for
    // physical devices on the same Wi-Fi as the dev Mac.
    let aas_url = match opts.runtime_server_port {
        Some(p) => format!("ws://127.0.0.1:{p}"),
        None => String::new(),
    };

    // Generate the project's app icon into a project-`res/` dir.
    // Returned flat (compiled below) goes into aapt2 link alongside
    // the AAR flats; `icon_attrs` populates the manifest template's
    // `{{ICON_ATTRS}}` placeholder so `android:icon` only appears
    // when there's actually an icon to reference.
    let project_res = build_dir.join("project-res");
    let project_icon_flat = build_dir.join("project-res.flata");
    let icon_attrs =
        sync_android_icons_into_res(&project_dir, &project_res, &build_tools, &project_icon_flat)?;

    // One `cargo metadata` over the wrapper's dependency graph feeds every
    // SDK-declared input: manifest fragments + resources here, and (below)
    // the Kotlin runtime sources and AndroidX artifacts.
    let wrapper_manifest = so.wrapper_dir.join("Cargo.toml");
    let metadata = kotlin_runtime::cargo_metadata(&wrapper_manifest)?;
    let sdk_contrib = SdkManifestContributions::from_metadata(&metadata)?;

    // Capability-derived `<uses-permission>` + `<service>` entries, discovered
    // from the same dependency graph.
    let (mut uses_permissions, services) =
        android_manifest_entries(&wrapper_manifest, &manifest.app.permissions);

    // The Robot bridge (auto-started whenever the `robot`/`dev` feature is on)
    // binds a localhost TCP listener, and Android only permits creating a
    // socket — even a loopback one — with the INTERNET permission. The
    // runtime-server manifest already declares INTERNET for its websocket; the
    // local-mode manifest declares no permissions, so without this the bridge
    // fails to bind (EACCES) and host tooling / MCP can't reach the app.
    if local_robot_build_needs_internet(opts.mode.is_runtime_server(), &opts.user_features)
        && !uses_permissions.contains("android.permission.INTERNET")
    {
        if !uses_permissions.is_empty() {
            uses_permissions.push('\n');
        }
        uses_permissions
            .push_str("<uses-permission android:name=\"android.permission.INTERNET\" />");
    }

    fs::write(
        &manifest_xml,
        render_manifest(
            manifest_tmpl,
            &ManifestInputs {
                package: bundle_id,
                app_name: &manifest.app.name,
                aas_url: &aas_url,
                robot_relay_url: opts.robot_relay_url.as_deref().unwrap_or(""),
                idealyst_env: &forwarded_env(|k| std::env::var(k).ok()),
                icon_attrs: &icon_attrs,
                uses_permissions: &uses_permissions,
                services: &services,
                // Only the local template carries the splice; the
                // runtime-server shell forwards no links (its Activity has no
                // link handling).
                intent_filters: &manifest.app.links.android_intent_filters(),
                cleartext: opts.cleartext,
                android: &manifest.app.android,
                sdk: &sdk_contrib,
            },
        ),
    )?;

    // SDK-declared resource files (`metadata.idealyst.android.res`), compiled
    // into their own `.flata` and linked beside the project's icon flat.
    let sdk_res = build_dir.join("sdk-res");
    let sdk_res_flat = build_dir.join("sdk-res.flata");
    if sdk_contrib.stage_res(&sdk_res)? {
        aapt2_compile_dir(&build_tools, &sdk_res, &sdk_res_flat, "SDK resources")?;
    }

    // ── 5. Prepare AAR resources + Kotlin runtime ────────────────
    // The backend ships a small Kotlin runtime (RustDrawerLayout,
    // RustNavigator, listener shims, …) that the JNI code reaches via
    // `env.find_class(...)`. kotlinc compiles those alongside the
    // user's Java sources, with the needed androidx classes.jars on
    // the classpath. We also extract each AAR's `res/` and compile it
    // to a `.flata` so the aapt2 link step can produce a real resource
    // table + R.java files (DrawerLayout's `obtainStyledAttributes`
    // refuses zero IDs, so stub R classes don't work).
    // The wrapper's Cargo.toml is what `cargo metadata` walks to find
    // any third-party SDK Kotlin runtime sources + AndroidX
    // requirements (declared in each SDK's
    // `[package.metadata.idealyst.android]`).
    let mut runtime = kotlin_runtime::build_runtime(
        &build_dir,
        &android_jar,
        &build_tools,
        &wrapper_manifest,
        &metadata,
    )?;
    // Splice the project's compiled icon flat into the aapt2-link
    // resource set. Empty when no icons were generated — `aapt2
    // compile` only produces a flat when there's input, and
    // `sync_android_icons_into_res` returns a fresh-empty path
    // string in that case which we filter out.
    if project_icon_flat.is_file() {
        runtime.aar_resource_flats.push(project_icon_flat.clone());
    }
    if sdk_res_flat.is_file() {
        runtime.aar_resource_flats.push(sdk_res_flat.clone());
    }

    // ── 6. aapt2 link → APK + generated R.java ───────────────────
    let unsigned_apk = build_dir.join("unsigned.apk");
    run_aapt2_link(
        &build_tools,
        &manifest_xml,
        &android_jar,
        &runtime.aar_resource_flats,
        &runtime.aar_extra_packages,
        &runtime.r_java_dir,
        &AppVersion {
            code: version_code,
            name: &manifest.app.version,
        },
        &unsigned_apk,
    )?;

    // ── 7. javac (user .java + generated R.java + SDK .java) ─────
    let class_dir = build_dir.join("classes");
    fs::create_dir_all(&class_dir)?;
    let mut java_classpath = vec![android_jar.clone()];
    java_classpath.push(runtime.kotlin_class_dir.clone());
    java_classpath.extend(runtime.androidx_jars.iter().cloned());
    java_classpath.push(runtime.kotlin_stdlib_jar.clone());
    let mut java_input_dirs = vec![java_dir.clone(), runtime.r_java_dir.clone()];
    if let Some(ref ext_java) = runtime.extension_java_dir {
        // Third-party SDKs that contribute `.java` (e.g. via
        // `metadata.idealyst.android.runtime_java`) get compiled in
        // the same javac invocation as the user's project Java + the
        // AAR-generated R.java. Same classpath, same -d output.
        java_input_dirs.push(ext_java.clone());
    }
    compile_java_dirs(&java_input_dirs, &class_dir, &java_classpath)?;

    // ── 8. d8 → classes.dex ─────────────────────────────────────
    // Hand d8 everything that needs to land in the APK as dex:
    //   - user-Java + R.java .class files
    //   - kotlin-runtime .class files
    //   - androidx classes.jars (one per artifact)
    //   - kotlin-stdlib.jar
    let dex_dir = build_dir.join("dex");
    fs::create_dir_all(&dex_dir)?;
    let mut dex_inputs: Vec<PathBuf> = Vec::new();
    dex_inputs.push(class_dir.clone());
    dex_inputs.push(runtime.kotlin_class_dir.clone());
    dex_inputs.extend(runtime.androidx_jars.iter().cloned());
    dex_inputs.push(runtime.kotlin_stdlib_jar.clone());
    run_d8(&build_tools, &dex_inputs, &dex_dir, &android_jar)?;

    // ── 8. zip in classes.dex + lib/<abi>/<so> ──────────────────
    splice_into_apk(&unsigned_apk, &dex_dir, &so, &build_dir)?;

    // ── 8b. zip in the app's `assets/` as APK `assets/<path>` ────
    // Staged beside (not inside) `build_dir`, which is wiped every run:
    // the staging copy persists, so unchanged files are not rewritten.
    let apk_assets_root = build_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("android build dir has no parent"))?
        .join(APK_ASSETS_STAGING);
    if stage_apk_assets(&project_dir, &apk_assets_root)? {
        splice_assets_into_apk(&unsigned_apk, &apk_assets_root)?;
    }

    // ── 9. zipalign ─────────────────────────────────────────────
    let aligned_apk = build_dir.join("aligned.apk");
    run_zipalign(&build_tools, &unsigned_apk, &aligned_apk)?;

    // ── 10. apksigner sign ──────────────────────────────────────
    let signed_apk = build_dir.join(format!("{}.apk", manifest.name));
    run_apksigner(&build_tools, &aligned_apk, &signing, &signed_apk)?;

    Ok(PackagedApk {
        apk: signed_apk,
        package: bundle_id.to_string(),
        version_code,
        version_name: manifest.app.version.clone(),
    })
}

/// Best-effort: focus the Android emulator's window so it surfaces
/// when launch finishes. macOS only — the emulator runs as
/// `qemu-system-aarch64` (Apple Silicon) or `qemu-system-x86_64`
/// (Intel), so we match by name prefix via System Events. Silent on
/// failure: a missing process or denied automation permission
/// shouldn't break the launch.
fn bring_emulator_to_front() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let script = r#"tell application "System Events"
        set procs to (every process whose name starts with "qemu-system")
        repeat with p in procs
            set frontmost of p to true
        end repeat
    end tell"#;
    let _ = Command::new("osascript")
        .args(["-e", script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

// ---------------------------------------------------------------------------
// SDK / tool resolution
// ---------------------------------------------------------------------------

fn find_android_sdk() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("ANDROID_HOME") {
        let p = PathBuf::from(h);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if let Ok(h) = std::env::var("ANDROID_SDK_ROOT") {
        let p = PathBuf::from(h);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if let Some(home) = home_dir().ok() {
        let candidates = [
            home.join("Library/Android/sdk"), // macOS default
            home.join("Android/Sdk"),         // Linux default
            home.join("AppData/Local/Android/Sdk"), // Windows-style; harmless on others
        ];
        for c in candidates {
            if c.is_dir() {
                return Ok(c);
            }
        }
    }
    anyhow::bail!(
        "couldn't find the Android SDK. Set ANDROID_HOME or install via Android Studio."
    )
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow::anyhow!("neither $HOME nor %USERPROFILE% is set"))
}

/// Pick the platform directory with the highest numeric API level,
/// e.g. `android-36` > `android-23`. Filters out preview/alphabetic
/// platforms (`android-P`, `android-Tiramisu`, etc.) because their
/// `android.jar` is often missing or moves between revisions.
fn pick_latest_platform(parent: &Path) -> Result<PathBuf> {
    let mut numbered: Vec<(u32, PathBuf)> = fs::read_dir(parent)
        .with_context(|| format!("read {}", parent.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let api = name.strip_prefix("android-")?.parse::<u32>().ok()?;
            Some((api, e.path()))
        })
        .collect();
    numbered.sort_by_key(|(api, _)| *api);
    numbered
        .pop()
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow::anyhow!("no `android-<N>` directory under {}", parent.display()))
}

/// Pick the lexicographically largest subdirectory (which for
/// build-tools means "latest version" since they're all
/// `<major>.<minor>.<patch>` numeric).
fn pick_latest_dir(parent: &Path) -> Result<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(parent)
        .with_context(|| format!("read {}", parent.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries
        .last()
        .map(|e| e.path())
        .ok_or_else(|| anyhow::anyhow!("no subdirectories under {}", parent.display()))
}

// ---------------------------------------------------------------------------
// Build pipeline
// ---------------------------------------------------------------------------

fn compile_java_dirs(java_dirs: &[PathBuf], class_dir: &Path, classpath: &[PathBuf]) -> Result<()> {
    let mut sources: Vec<PathBuf> = Vec::new();
    for d in java_dirs {
        if d.is_dir() {
            visit_files(d, "java", &mut sources)?;
        }
    }
    if sources.is_empty() {
        anyhow::bail!(
            "no .java files found under {:?}",
            java_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
        );
    }
    eprintln!("[run-android] javac → {}", class_dir.display());
    let cp = classpath
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    let mut cmd = Command::new("javac");
    cmd.arg("-classpath")
        .arg(&cp)
        .arg("-d")
        .arg(class_dir)
        // Targeting JDK 8 bytecode matches d8's expectations across
        // every modern build-tools revision. javac 21+ produces 8
        // bytecode if `--release 8` is passed.
        .arg("--release")
        .arg("8");
    for s in &sources {
        cmd.arg(s);
    }
    let status = cmd
        .status()
        .with_context(|| "spawn javac — is a JDK on your PATH?")?;
    if !status.success() {
        anyhow::bail!("javac exited with {status}");
    }
    Ok(())
}

fn visit_files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_files(&path, ext, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    Ok(())
}

/// Dex everything that needs to land in the APK. `inputs` is a mix of
/// directories (each walked for .class files) and .jar files (passed
/// to d8 directly).
fn run_d8(
    build_tools: &Path,
    inputs: &[PathBuf],
    dex_dir: &Path,
    android_jar: &Path,
) -> Result<()> {
    let mut class_files: Vec<PathBuf> = Vec::new();
    let mut jar_files: Vec<PathBuf> = Vec::new();
    for p in inputs {
        if p.is_dir() {
            visit_files(p, "class", &mut class_files)?;
        } else if p.is_file() {
            jar_files.push(p.clone());
        } else {
            anyhow::bail!("d8 input {} doesn't exist", p.display());
        }
    }
    if class_files.is_empty() && jar_files.is_empty() {
        anyhow::bail!("no .class or .jar inputs for d8");
    }
    eprintln!(
        "[run-android] d8 → {} ({} class files, {} jars)",
        dex_dir.display(),
        class_files.len(),
        jar_files.len(),
    );
    let mut cmd = Command::new(build_tools.join("d8"));
    cmd.arg("--lib")
        .arg(android_jar)
        .arg("--min-api")
        .arg(MIN_SDK_VERSION.to_string())
        .arg("--output")
        .arg(dex_dir);
    for s in &class_files {
        cmd.arg(s);
    }
    for j in &jar_files {
        cmd.arg(j);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", build_tools.join("d8").display()))?;
    if !status.success() {
        anyhow::bail!("d8 exited with {status}");
    }
    Ok(())
}

/// Generate the Android app icons into a project-`res/` tree, compile
/// the tree with `aapt2 compile --dir`, and return the manifest-attr
/// snippet (`android:icon=…`) to splice into `<application>`.
///
/// Returns `Ok(String::new())` and writes nothing when the project's
/// manifest has no `[package.metadata.idealyst.app.icon]` block.
/// `out_flat` is only created in the configured case — callers check
/// its existence before pushing it into the aapt2-link resource set.
fn sync_android_icons_into_res(
    project_dir: &Path,
    res_dir: &Path,
    build_tools: &Path,
    out_flat: &Path,
) -> Result<String> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        return Ok(String::new());
    };
    let block = config.resolved_for(icon_gen::Target::Android);
    if icon_gen::sync_android_icons(Some(&block), res_dir)?.is_none() {
        return Ok(String::new());
    }
    aapt2_compile_dir(build_tools, res_dir, out_flat, "project icons")?;
    // Leading space + attr pair so the template can splice this
    // directly into the `<application … {{ICON_ATTRS}}>` opening
    // tag without breaking when icons aren't configured (empty
    // string in that case).
    Ok(" android:icon=\"@mipmap/ic_launcher\" android:roundIcon=\"@mipmap/ic_launcher_round\""
        .to_string())
}

#[allow(clippy::too_many_arguments)]
fn run_aapt2_link(
    build_tools: &Path,
    manifest_xml: &Path,
    android_jar: &Path,
    aar_flats: &[PathBuf],
    extra_packages: &[String],
    r_java_out_dir: &Path,
    version: &AppVersion<'_>,
    out_apk: &Path,
) -> Result<()> {
    eprintln!(
        "[run-android] aapt2 link ({} AAR flats, {} extra pkgs) → {}",
        aar_flats.len(),
        extra_packages.len(),
        out_apk.display()
    );
    fs::create_dir_all(r_java_out_dir)?;
    let mut cmd = Command::new(build_tools.join("aapt2"));
    cmd.arg("link")
        .arg("--manifest")
        .arg(manifest_xml)
        .arg("-I")
        .arg(android_jar)
        .arg("--min-sdk-version")
        .arg(MIN_SDK_VERSION.to_string())
        .arg("--target-sdk-version")
        .arg(TARGET_SDK_VERSION.to_string())
        .arg("--version-code")
        .arg(version.code.to_string())
        .arg("--version-name")
        .arg(version.name)
        .arg("--java")
        .arg(r_java_out_dir)
        .arg("--auto-add-overlay");
    for flat in aar_flats {
        cmd.arg("-R").arg(flat);
    }
    if !extra_packages.is_empty() {
        // aapt2 accepts a colon-separated list of extra packages —
        // each gets its own emitted R.java alongside the main package's.
        cmd.arg("--extra-packages").arg(extra_packages.join(":"));
    }
    cmd.arg("-o").arg(out_apk);
    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", build_tools.join("aapt2").display()))?;
    if !status.success() {
        anyhow::bail!("aapt2 link exited with {status}");
    }
    Ok(())
}

/// `aapt2 link` produces an APK containing only the manifest (and
/// resources, if we had any). We splice in `classes.dex` at the
/// APK root and the native `.so` at `lib/<abi>/` using the system
/// `zip` utility — easier than pulling in the `zip` crate just for
/// adding two files.
fn splice_into_apk(
    apk: &Path,
    dex_dir: &Path,
    so: &build_android::BuildArtifact,
    build_dir: &Path,
) -> Result<()> {
    // Stage `lib/<abi>/<libname>.so` so `zip -j` won't keep us
    // dragging the absolute path into the archive.
    let staging = build_dir.join("staging");
    let abi_dir = staging.join("lib").join(so.abi);
    fs::create_dir_all(&abi_dir)?;
    let so_dest = abi_dir.join(
        so.dylib
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("dylib path has no filename"))?,
    );
    fs::copy(&so.dylib, &so_dest)?;
    fs::copy(dex_dir.join("classes.dex"), staging.join("classes.dex"))?;

    eprintln!("[run-android] zipping classes.dex + lib/{}/{}.so into {}",
        so.abi,
        so.dylib.file_stem().and_then(|s| s.to_str()).unwrap_or("?"),
        apk.display());
    // `zip <archive> classes.dex lib/<abi>/<name>.so`, run with cwd
    // = staging so the stored paths are relative.
    let status = Command::new("zip")
        .arg("-r")
        .arg(apk)
        .arg("classes.dex")
        .arg(format!("lib/{}", so.abi))
        .current_dir(&staging)
        .status()
        .with_context(|| "spawn zip — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("zip exited with {status}");
    }
    Ok(())
}

/// Staging dir (sibling of `app/` and `wrapper/`) holding `assets/`.
const APK_ASSETS_STAGING: &str = "apk-assets";

/// Extensions stored uncompressed in the APK. These formats are already
/// compressed (deflating them again gains nothing), and a STORED entry is
/// what `AssetManager.openFd` needs — media players read assets through a
/// file descriptor, which a deflated entry cannot provide.
const APK_STORED_ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "mp3", "mp4", "m4a", "m4v", "aac", "ogg", "opus",
    "webm", "mkv", "woff", "woff2", "zip", "gz", "br",
];

/// Mirror the app crate's `assets/` into `<root>/assets/`. Returns whether
/// the app has assets. See [`build_ios::app_assets`].
fn stage_apk_assets(project_dir: &Path, root: &Path) -> Result<bool> {
    build_ios::app_assets::stage_app_assets(
        project_dir,
        &root.join(build_ios::app_assets::ASSETS_DIR),
    )
}

/// `zip` arguments that add `<root>/assets/` to `apk`, run with cwd
/// `root` so each entry is stored as `assets/<path>`. Android serves APK
/// entry `assets/<path>` as `AssetManager.open("<path>")` — the path the
/// backend's `asset!` loader opens.
fn apk_assets_zip_args(apk: &Path) -> Vec<std::ffi::OsString> {
    let stored = APK_STORED_ASSET_EXTENSIONS
        .iter()
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(":");
    vec![
        // `-n`: store (don't deflate) these suffixes; `-r`: recurse;
        // `-y`: store a symlink as a link, never its target (staging has
        // none — `sync_dir` drops them — this is belt and braces).
        "-n".into(),
        stored.into(),
        "-r".into(),
        "-y".into(),
        apk.into(),
        build_ios::app_assets::ASSETS_DIR.into(),
    ]
}

/// Splice the staged `assets/` into the APK. Done with `zip` (like
/// `classes.dex`) rather than `aapt2 link -A`: aapt2's default
/// `--ignore-assets` pattern silently drops dotfiles and `_`-prefixed
/// directories, so an `asset!` path that works on every other platform
/// would 404 on Android.
fn splice_assets_into_apk(apk: &Path, root: &Path) -> Result<()> {
    eprintln!("[run-android] zipping assets/ into {}", apk.display());
    let status = Command::new("zip")
        .args(apk_assets_zip_args(apk))
        .current_dir(root)
        .status()
        .with_context(|| "spawn zip — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("zip (assets) exited with {status}");
    }
    Ok(())
}

fn run_zipalign(build_tools: &Path, input: &Path, output: &Path) -> Result<()> {
    eprintln!("[run-android] zipalign → {}", output.display());
    // `-p` page-aligns the `.so` so the kernel can mmap it directly
    // without an intermediate copy. `-f` overwrites the output. `4`
    // is the standard alignment in bytes for everything else.
    let status = Command::new(build_tools.join("zipalign"))
        .args(["-p", "-f", "4"])
        .arg(input)
        .arg(output)
        .status()
        .with_context(|| format!("spawn {}", build_tools.join("zipalign").display()))?;
    if !status.success() {
        anyhow::bail!("zipalign exited with {status}");
    }
    Ok(())
}

/// `versionCode` + `versionName`, stamped by `aapt2 link`.
struct AppVersion<'a> {
    code: u32,
    name: &'a str,
}

/// The key an APK is signed with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SigningKey {
    /// `~/.android/debug.keystore`, password `android` by convention.
    Debug { keystore: PathBuf },
    /// The app's configured release key; passwords come from the named env
    /// vars, which apksigner reads itself (`env:VAR`).
    Release {
        keystore: PathBuf,
        alias: String,
        store_password_env: String,
        key_password_env: Option<String>,
    },
}

/// The app's release key when `[package.metadata.idealyst.app.android.signing]`
/// is set (its keystore must exist and its password env vars must be set),
/// else the debug keystore under `home`.
fn resolve_signing(
    project_dir: &Path,
    android: &AndroidAppMetadata,
    home: &Path,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<SigningKey> {
    let Some(signing) = &android.signing else {
        let keystore = home.join(".android/debug.keystore");
        if !keystore.is_file() {
            anyhow::bail!(
                "{} not found. Android Studio creates this on first launch — \
                 open Studio once, or generate manually: \
                 keytool -genkeypair -keystore ~/.android/debug.keystore -storepass android \
                 -keypass android -keyalg RSA -alias androiddebugkey -dname 'CN=Android Debug' \
                 -validity 10000",
                keystore.display(),
            );
        }
        return Ok(SigningKey::Debug { keystore });
    };
    let keystore = signing.keystore_path(project_dir);
    if !keystore.is_file() {
        anyhow::bail!(
            "release keystore {} (from [package.metadata.idealyst.app.android.signing]) not found",
            keystore.display()
        );
    }
    for var in std::iter::once(&signing.store_password_env).chain(signing.key_password_env.iter()) {
        if env(var).is_none() {
            anyhow::bail!(
                "${var} is not set; it holds a password for the release keystore {} \
                 ([package.metadata.idealyst.app.android.signing])",
                keystore.display()
            );
        }
    }
    Ok(SigningKey::Release {
        keystore,
        alias: signing.alias.clone(),
        store_password_env: signing.store_password_env.clone(),
        key_password_env: signing.key_password_env.clone(),
    })
}

/// `apksigner sign` arguments for `key`, minus `--out` and the input APK.
fn apksigner_key_args(key: &SigningKey) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec!["sign".into(), "--ks".into()];
    match key {
        // The debug keystore's password is `android` by convention. If a user
        // has rotated it we surface the apksigner failure; they can regenerate
        // by deleting `~/.android/debug.keystore` (Android Studio re-creates it
        // on next open).
        SigningKey::Debug { keystore } => {
            args.push(keystore.into());
            args.extend(["--ks-pass", "pass:android", "--key-pass", "pass:android"].map(Into::into));
        }
        SigningKey::Release {
            keystore,
            alias,
            store_password_env,
            key_password_env,
        } => {
            args.push(keystore.into());
            args.push("--ks-key-alias".into());
            args.push(alias.into());
            args.push("--ks-pass".into());
            args.push(format!("env:{store_password_env}").into());
            // No `--key-pass`: apksigner then uses the keystore password for
            // the key, the usual single-password keystore.
            if let Some(var) = key_password_env {
                args.push("--key-pass".into());
                args.push(format!("env:{var}").into());
            }
        }
    }
    args
}

fn run_apksigner(build_tools: &Path, input: &Path, key: &SigningKey, output: &Path) -> Result<()> {
    let which = match key {
        SigningKey::Debug { .. } => "debug keystore".to_string(),
        SigningKey::Release { keystore, alias, .. } => {
            format!("release key {alias} in {}", keystore.display())
        }
    };
    eprintln!("[run-android] apksigner sign ({which}) → {}", output.display());
    let status = Command::new(build_tools.join("apksigner"))
        .args(apksigner_key_args(key))
        .arg("--out")
        .arg(output)
        .arg(input)
        .status()
        .with_context(|| format!("spawn {}", build_tools.join("apksigner").display()))?;
    if !status.success() {
        anyhow::bail!("apksigner exited with {status}");
    }
    Ok(())
}

/// `aapt2 compile --dir <res_dir> -o <out_flat>`.
fn aapt2_compile_dir(build_tools: &Path, res_dir: &Path, out_flat: &Path, what: &str) -> Result<()> {
    eprintln!("[run-android] aapt2 compile ({what}) → {}", out_flat.display());
    let status = Command::new(build_tools.join("aapt2"))
        .arg("compile")
        .arg("--dir")
        .arg(res_dir)
        .arg("-o")
        .arg(out_flat)
        .status()
        .with_context(|| format!("spawn aapt2 compile for {what}"))?;
    if !status.success() {
        anyhow::bail!("aapt2 compile ({what}) exited with {status}");
    }
    Ok(())
}

/// Everything substituted into an `AndroidManifest*.xml` template.
struct ManifestInputs<'a> {
    package: &'a str,
    app_name: &'a str,
    aas_url: &'a str,
    robot_relay_url: &'a str,
    idealyst_env: &'a str,
    icon_attrs: &'a str,
    uses_permissions: &'a str,
    services: &'a str,
    intent_filters: &'a str,
    cleartext: bool,
    android: &'a AndroidAppMetadata,
    sdk: &'a SdkManifestContributions,
}

fn render_manifest(template: &str, i: &ManifestInputs<'_>) -> String {
    let sdk_root = i.sdk.render_root(i.package);
    let sdk_application = i.sdk.render_application(i.package);
    render(template, &[
        ("PACKAGE", i.package),
        ("APP_NAME", &xml_escape(i.app_name)),
        ("APP_ID", &xml_escape(i.package)),
        ("AAS_URL", &xml_escape(i.aas_url)),
        ("ROBOT_RELAY_URL", &xml_escape(i.robot_relay_url)),
        ("IDEALYST_ENV", &xml_escape(i.idealyst_env)),
        ("ICON_ATTRS", i.icon_attrs),
        ("USES_PERMISSIONS", i.uses_permissions),
        ("SERVICES", i.services),
        ("INTENT_FILTERS", i.intent_filters),
        ("CLEARTEXT", if i.cleartext { "true" } else { "false" }),
        ("MAIN_ACTIVITY_ATTRS", &main_activity_attrs(i.android)),
        ("HOME_INTENT_FILTER", home_intent_filter(i.android)),
        ("SDK_MANIFEST_ROOT", &sdk_root),
        ("SDK_MANIFEST_APPLICATION", &sdk_application),
    ])
}

/// Extra attributes on the main `<activity>` opening tag, each with a
/// leading space (empty when the app sets none).
fn main_activity_attrs(android: &AndroidAppMetadata) -> String {
    match android.lock_task_mode {
        Some(mode) => format!(" android:lockTaskMode=\"{}\"", mode.manifest_value()),
        None => String::new(),
    }
}

/// The HOME intent filter for an app declared as a home screen. A separate
/// filter from the LAUNCHER one: a filter matches only when all its
/// categories match, and HOME intents carry `DEFAULT` too.
fn home_intent_filter(android: &AndroidAppMetadata) -> &'static str {
    if android.home_launcher {
        "<intent-filter>
                <action android:name=\"android.intent.action.MAIN\" />
                <category android:name=\"android.intent.category.HOME\" />
                <category android:name=\"android.intent.category.DEFAULT\" />
            </intent-filter>"
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// Device / emulator orchestration
// ---------------------------------------------------------------------------

fn ensure_device(adb: &Path, sdk: &Path, prefer_avd: Option<&str>) -> Result<String> {
    if let Some(serial) = first_ready_device(adb)? {
        return Ok(serial);
    }

    // Nothing online — boot an emulator.
    let avd = pick_avd(sdk, prefer_avd)?;
    eprintln!("[run-android] booting emulator '{avd}'…");
    let emulator = sdk.join("emulator/emulator");
    if !emulator.is_file() {
        anyhow::bail!(
            "{} not found. Install via Android Studio's SDK Manager or `sdkmanager emulator`.",
            emulator.display(),
        );
    }
    // Spawn detached so the emulator stays up after this process
    // exits. The user can `adb emu kill` to stop it.
    Command::new(emulator)
        .arg("-avd")
        .arg(&avd)
        // `-no-snapshot-save` avoids overwriting whatever snapshot
        // exists when the user later closes the emulator window;
        // makes repeated `run android` invocations idempotent.
        .arg("-no-snapshot-save")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| "spawn emulator")?;

    eprintln!("[run-android] waiting for emulator boot…");
    wait_for_boot(adb, Duration::from_secs(180))
}

fn first_ready_device(adb: &Path) -> Result<Option<String>> {
    let out = Command::new(adb)
        .args(["devices"])
        .output()
        .with_context(|| format!("spawn {}", adb.display()))?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        // Format: "<serial>\t<state>"
        let mut parts = line.split_whitespace();
        let serial = match parts.next() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let state = parts.next().unwrap_or("");
        if state == "device" {
            return Ok(Some(serial.to_string()));
        }
    }
    Ok(None)
}

fn pick_avd(sdk: &Path, prefer: Option<&str>) -> Result<String> {
    let emulator = sdk.join("emulator/emulator");
    let out = Command::new(&emulator)
        .arg("-list-avds")
        .output()
        .with_context(|| format!("spawn {} -list-avds", emulator.display()))?;
    if !out.status.success() {
        anyhow::bail!("{} -list-avds failed", emulator.display());
    }
    let avds: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if avds.is_empty() {
        anyhow::bail!(
            "no AVDs configured — create one with Android Studio's Device Manager \
             or `avdmanager create avd`"
        );
    }
    if let Some(name) = prefer {
        if avds.iter().any(|a| a == name) {
            return Ok(name.to_string());
        }
        anyhow::bail!("AVD {:?} not found. Available: {}", name, avds.join(", "));
    }
    // Prefer something Pixel-shaped over a TV/Wear profile.
    if let Some(p) = avds.iter().find(|a| a.starts_with(DEFAULT_AVD_PREFIX)) {
        return Ok(p.clone());
    }
    Ok(avds[0].clone())
}

fn wait_for_boot(adb: &Path, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    // Phase 1: wait for any emulator to appear in `adb devices`.
    let serial = loop {
        if let Some(s) = first_ready_device(adb)? {
            // We specifically want an emulator-* serial since a
            // physical device may have shown up earlier as well.
            if s.starts_with("emulator-") {
                break s;
            }
            // Otherwise pick the first device and proceed.
            break s;
        }
        if Instant::now() > deadline {
            anyhow::bail!("emulator never appeared in `adb devices` within {:?}", timeout);
        }
        thread::sleep(Duration::from_millis(500));
    };
    // Phase 2: wait for boot_completed.
    while Instant::now() < deadline {
        let prop = Command::new(adb)
            .args(["-s", &serial, "shell", "getprop", "sys.boot_completed"])
            .output();
        if let Ok(o) = prop {
            if o.status.success() {
                let v = String::from_utf8_lossy(&o.stdout);
                if v.trim() == "1" {
                    return Ok(serial);
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
    anyhow::bail!("emulator booted to ADB but never reported sys.boot_completed=1")
}

fn adb_install(adb: &Path, serial: &str, apk: &Path) -> Result<()> {
    eprintln!("[run-android] adb install {} → {serial}", apk.display());
    let status = Command::new(adb)
        .args(["-s", serial, "install", "-r"])
        .arg(apk)
        .status()
        .with_context(|| format!("spawn {} install", adb.display()))?;
    if !status.success() {
        anyhow::bail!("adb install exited with {status}");
    }
    Ok(())
}

/// Set up a reverse port tunnel: `port` on the device's loopback
/// forwards to the same port on the host's loopback. Lets an
/// emulator (whose own network is a QEMU NAT that can't see the
/// host's LAN) reach the host's dev-server via `ws://127.0.0.1:port`
/// inside the app.
///
/// `adb reverse` also works over USB ADB on physical devices, so
/// this is safe to run for any connected device — the corresponding
/// `IdealystRuntimeServerUrl` we bake into the manifest works in both cases.
fn adb_reverse(adb: &Path, serial: &str, port: u16) -> Result<()> {
    let spec = format!("tcp:{port}");
    eprintln!("[run-android] adb reverse {spec} {spec} on {serial}");
    let status = Command::new(adb)
        .args(["-s", serial, "reverse"])
        .arg(&spec)
        .arg(&spec)
        .status()
        .with_context(|| format!("spawn {} reverse", adb.display()))?;
    if !status.success() {
        anyhow::bail!("adb reverse exited with {status}");
    }
    Ok(())
}

/// Whether a build needs the INTERNET permission injected so the Robot
/// bridge can bind its localhost listener.
///
/// Only **local-mode** builds with the `robot`/`dev` feature active need
/// it: runtime-server builds already declare INTERNET for their
/// websocket, and a production local build (no robot feature) shouldn't
/// be granted needless network access. Without this, the bridge's
/// `TcpListener::bind` fails with EACCES on Android and host tooling /
/// MCP can't reach the app — see the live-verification of the `device_frame`
/// / `adb input tap` path that surfaced this gap.
/// The dev-tooling variables an Android app gets from the dev process's
/// environment (desktop apps inherit them as child processes).
const FORWARDED_ENV: &[&str] = &["IDEALYST_TEST_DRIVER"];

/// `KEY=VALUE;…` for each [`FORWARDED_ENV`] variable that is set — the
/// `IdealystEnv` manifest meta-data MainActivity replays through
/// `NativeBridge.setEnv` before attach.
fn forwarded_env(get: impl Fn(&str) -> Option<String>) -> String {
    FORWARDED_ENV
        .iter()
        .filter_map(|k| get(k).map(|v| format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join(";")
}

fn local_robot_build_needs_internet(is_runtime_server: bool, user_features: &[String]) -> bool {
    !is_runtime_server
        && user_features
            .iter()
            .any(|f| f.contains("dev") || f.contains("robot"))
}

fn adb_launch(adb: &Path, serial: &str, component: &str) -> Result<()> {
    eprintln!("[run-android] adb shell am start -n {component}");
    let status = Command::new(adb)
        .args(["-s", serial, "shell", "am", "start", "-n", component])
        .status()
        .with_context(|| format!("spawn {} shell am start", adb.display()))?;
    if !status.success() {
        anyhow::bail!("adb am start exited with {status}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tiny templating helpers
// ---------------------------------------------------------------------------

/// Discover the capabilities the app's dependency graph declares and render
/// the Android manifest fragments they expand into: the `<uses-permission>`
/// entries (`{{USES_PERMISSIONS}}` placeholder) and the `<service>` entries
/// (`{{SERVICES}}` placeholder — e.g. the screen_capture mediaProjection
/// foreground service). Warnings (generic reason, unknown capability) and a
/// per-permission report are printed so an auto-added entry is visible. A
/// discovery error degrades to empty fragments with a warning rather than
/// failing the run. Returns `(uses_permissions, services)`.
fn android_manifest_entries(
    wrapper_manifest: &Path,
    app_reasons: &std::collections::BTreeMap<String, String>,
) -> (String, String) {
    let discovered = match capabilities::discover(wrapper_manifest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: could not discover app capabilities: {e}");
            return (String::new(), String::new());
        }
    };
    if discovered.is_empty() {
        return (String::new(), String::new());
    }
    let resolved = capabilities::resolve(&discovered, app_reasons);
    for w in &resolved.warnings {
        eprintln!("warning: {w}");
    }
    for r in &resolved.report {
        println!("  Android permission: {r}");
    }
    let uses_permissions = resolved
        .android_permissions
        .iter()
        .map(|p| format!("<uses-permission android:name=\"{}\" />", xml_escape(p)))
        .collect::<Vec<_>>()
        .join("\n    ");
    // A capability-declared service is internal (not launched from outside the
    // app), so `android:exported="false"`. `foregroundServiceType` is what the
    // OS checks against the started-foreground type — mediaProjection here.
    let services = resolved
        .android_services
        .iter()
        .map(|s| {
            format!(
                "<service android:name=\"{}\" android:foregroundServiceType=\"mediaProjection\" android:exported=\"false\" />",
                xml_escape(s)
            )
        })
        .collect::<Vec<_>>()
        .join("\n        ");
    (uses_permissions, services)
}

fn render(template: &str, vars: &[(&str, &str)]) -> String {
    // Build a HashMap so duplicates don't cause repeated linear
    // scans across the template. Tiny inputs, irrelevant, but
    // keeps the code uniform.
    let lookup: HashMap<&str, &str> = vars.iter().copied().collect();
    let mut out = template.to_string();
    for (k, v) in lookup {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

#[allow(dead_code)]
fn manifest_into_owned(m: &Manifest) -> &Manifest {
    // No-op preserved as a marker — we never need to clone the
    // shared Manifest, but build-ios may grow more fields and
    // having a focal point makes the diff obvious.
    m
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    /// `idealyst test` sets `IDEALYST_TEST_DRIVER` so an app's in-app
    /// self-test stands down, but an Android app has no environment to
    /// inherit: the self-test ran under the host suite and changed the
    /// state it asserted on. The variable now rides the manifest.
    #[test]
    fn regression_the_test_driver_flag_reaches_the_app_manifest() {
        use super::forwarded_env;
        let env = |k: &str| (k == "IDEALYST_TEST_DRIVER").then(|| "1".to_string());
        assert_eq!(forwarded_env(env), "IDEALYST_TEST_DRIVER=1");
        assert_eq!(forwarded_env(|_| None), "", "nothing set, nothing forwarded");
        assert!(
            include_str!("../templates/AndroidManifest.xml").contains("{{IDEALYST_ENV}}"),
            "the local manifest carries the meta-data"
        );
        assert!(include_str!("../templates/MainActivity.java").contains("NativeBridge.setEnv("));
    }

    /// `permissions::request` hung for the whole session on the first ask:
    /// the generated Activity didn't override `onRequestPermissionsResult`,
    /// so the answer never reached the SDK. It must forward to the runtime
    /// registry the SDK's Kotlin shim registers with.
    #[test]
    fn regression_main_activity_forwards_permission_results() {
        let activity = include_str!("../templates/MainActivity.java");
        assert!(activity.contains("public void onRequestPermissionsResult("));
        assert!(activity.contains(
            "io.idealyst.runtime.RustPermissionsResult.dispatch(requestCode, permissions, grantResults)"
        ));
        let registry = include_str!(
            "../../../../backend/android/mobile/runtime/kotlin/io/idealyst/runtime/RustPermissionsResult.kt"
        );
        assert!(registry.contains("object RustPermissionsResult"));
        assert!(registry.contains(
            "fun dispatch(requestCode: Int, permissions: Array<out String>, grantResults: IntArray): Boolean"
        ));
    }

    use super::{local_robot_build_needs_internet, render, ANDROID_MANIFEST_LOCAL_XML};
    use super::{apksigner_key_args, render_manifest, resolve_signing, ManifestInputs, SigningKey};
    use super::{AndroidAppMetadata, SdkManifestContributions};
    use super::{splice_assets_into_apk, stage_apk_assets, APK_ASSETS_STAGING};
    use std::fs;
    use std::process::Command;

    /// The launch Activity is `singleTask` (a warm link must reach the
    /// running instance's `onNewIntent`, not mount a second app) and the
    /// app's link filters are spliced inside it.
    #[test]
    fn local_manifest_declares_link_filters_on_a_single_task_activity() {
        let raw = r#"
            [package]
            name = "demo"
            version = "0.0.1"
            [package.metadata.idealyst.app]
            bundle_id = "ai.example.demo"
            [package.metadata.idealyst.app.links]
            schemes = ["demo"]
            domains = ["demo.example.com"]
        "#;
        let tmp = std::env::temp_dir().join(format!("run-android-links-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), raw).unwrap();
        let manifest = build_ios::parse_manifest(&tmp).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        let xml = render(
            ANDROID_MANIFEST_LOCAL_XML,
            &[("INTENT_FILTERS", &manifest.app.links.android_intent_filters())],
        );
        let activity = &xml[xml.find("<activity").unwrap()..xml.find("</activity>").unwrap()];
        assert!(activity.contains("android:launchMode=\"singleTask\""));
        assert!(activity.contains("<data android:scheme=\"demo\" />"), "{activity}");
        assert!(activity.contains("android:host=\"demo.example.com\""));
        assert!(activity.contains("android:autoVerify=\"true\""));
        assert!(!xml.contains("{{INTENT_FILTERS}}"));
    }

    /// The dev/run manifest must enable cleartext HTTP so a dev build can
    /// reach a server-function / `#[sse]` host running over `http://` on the
    /// developer's machine (the emulator's `http://10.0.2.2:<port>` loopback
    /// alias); a release APK keeps Android's secure default.
    #[test]
    fn cleartext_is_on_for_dev_and_off_for_release() {
        let meta = AndroidAppMetadata::default();
        let sdk = SdkManifestContributions::default();
        let dev = render_manifest(ANDROID_MANIFEST_LOCAL_XML, &inputs(&meta, &sdk, true));
        let release = render_manifest(ANDROID_MANIFEST_LOCAL_XML, &inputs(&meta, &sdk, false));
        assert!(dev.contains("android:usesCleartextTraffic=\"true\""), "{dev}");
        assert!(release.contains("android:usesCleartextTraffic=\"false\""), "{release}");
    }

    fn inputs<'a>(
        android: &'a AndroidAppMetadata,
        sdk: &'a SdkManifestContributions,
        cleartext: bool,
    ) -> ManifestInputs<'a> {
        ManifestInputs {
            package: "com.example.kiosk",
            app_name: "Kiosk & Co",
            aas_url: "",
            robot_relay_url: "",
            idealyst_env: "",
            icon_attrs: "",
            uses_permissions: "",
            services: "",
            intent_filters: "",
            cleartext,
            android,
            sdk,
        }
    }

    const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";

    /// Every placeholder in both templates is filled and the result is a
    /// well-formed manifest, with the app's kiosk attributes on the main
    /// activity and SDK fragments under the right parents.
    #[test]
    fn manifest_carries_kiosk_attributes_and_sdk_components_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("manifest")).unwrap();
        fs::write(
            tmp.path().join("manifest/app.xml"),
            r#"<receiver android:name="io.fake.Admin" android:permission="android.permission.BIND_DEVICE_ADMIN" android:exported="true" />"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("manifest/root.xml"),
            r#"<uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED" />"#,
        )
        .unwrap();
        let sdk = SdkManifestContributions::from_metadata(&serde_json::json!({ "packages": [{
            "name": "fake-sdk",
            "manifest_path": tmp.path().join("Cargo.toml").display().to_string(),
            "metadata": { "idealyst": { "android": {
                "manifest_application": ["manifest/app.xml"],
                "manifest_root": ["manifest/root.xml"],
            } } },
        }]}))
        .unwrap();
        let kiosk = AndroidAppMetadata {
            lock_task_mode: Some(build_ios::android_app::LockTaskMode::IfWhitelisted),
            home_launcher: true,
            ..Default::default()
        };

        for template in [ANDROID_MANIFEST_LOCAL_XML, super::ANDROID_MANIFEST_AAS_XML] {
            let xml = render_manifest(template, &inputs(&kiosk, &sdk, true));
            assert!(!xml.contains("{{"), "unfilled placeholder:\n{xml}");
            let doc = roxmltree::Document::parse(&xml).unwrap_or_else(|e| panic!("{e}\n{xml}"));
            fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Vec<roxmltree::Node<'a, 'i>> {
                n.children().filter(|c| c.has_tag_name(tag)).collect()
            }
            let manifest = doc.root_element();
            let perms = child(manifest, "uses-permission");
            assert!(perms.iter().any(|p| p.attribute((ANDROID_NS, "name"))
                == Some("android.permission.RECEIVE_BOOT_COMPLETED")));
            let app = child(manifest, "application")[0];
            let receivers = child(app, "receiver");
            assert_eq!(receivers.len(), 1, "{xml}");
            assert_eq!(receivers[0].attribute((ANDROID_NS, "name")), Some("io.fake.Admin"));
            let activity = child(app, "activity")[0];
            assert_eq!(activity.attribute((ANDROID_NS, "lockTaskMode")), Some("if_whitelisted"));
            let has_category = |f: &roxmltree::Node<'_, '_>, c: &str| {
                child(*f, "category").iter().any(|n| n.attribute((ANDROID_NS, "name")) == Some(c))
            };
            let filters = child(activity, "intent-filter");
            assert!(filters.iter().any(|f| has_category(f, "android.intent.category.LAUNCHER")));
            assert!(filters.iter().any(|f| has_category(f, "android.intent.category.HOME")
                && has_category(f, "android.intent.category.DEFAULT")));
        }

        // An app that sets neither gets neither.
        let plain = render_manifest(
            ANDROID_MANIFEST_LOCAL_XML,
            &inputs(&AndroidAppMetadata::default(), &SdkManifestContributions::default(), true),
        );
        assert!(!plain.contains("lockTaskMode"));
        assert!(!plain.contains("category.HOME"));
        roxmltree::Document::parse(&plain).unwrap();
    }

    fn release_meta(key_env: Option<&str>) -> AndroidAppMetadata {
        AndroidAppMetadata {
            signing: Some(build_ios::android_app::AndroidSigning {
                keystore: "keys/release.jks".into(),
                alias: "rel".into(),
                store_password_env: "STORE_PW".into(),
                key_password_env: key_env.map(str::to_string),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn release_signing_reads_passwords_from_the_environment_only() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("keys")).unwrap();
        fs::write(tmp.path().join("keys/release.jks"), b"ks").unwrap();
        let env = |k: &str| (k == "STORE_PW").then(|| std::ffi::OsString::from("secret"));

        let key = resolve_signing(tmp.path(), &release_meta(None), tmp.path(), env).unwrap();
        let args: Vec<String> = apksigner_key_args(&key)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert!(args.windows(2).any(|w| w == ["--ks-key-alias", "rel"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--ks-pass", "env:STORE_PW"]), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("secret")), "a password leaked into argv");
        assert!(!args.iter().any(|a| a == "--key-pass"));

        let err = resolve_signing(tmp.path(), &release_meta(Some("KEY_PW")), tmp.path(), env)
            .unwrap_err();
        assert!(format!("{err}").contains("$KEY_PW is not set"), "{err}");
        let err = resolve_signing(tmp.path(), &release_meta(None), tmp.path(), |_| None).unwrap_err();
        assert!(format!("{err}").contains("$STORE_PW"), "{err}");

        fs::remove_file(tmp.path().join("keys/release.jks")).unwrap();
        let err = resolve_signing(tmp.path(), &release_meta(None), tmp.path(), env).unwrap_err();
        assert!(format!("{err}").contains("not found"), "{err}");
    }

    #[test]
    fn no_signing_block_uses_the_debug_keystore() {
        let tmp = tempfile::tempdir().unwrap();
        let none = AndroidAppMetadata::default();
        assert!(resolve_signing(tmp.path(), &none, tmp.path(), |_| None).is_err());
        fs::create_dir_all(tmp.path().join(".android")).unwrap();
        fs::write(tmp.path().join(".android/debug.keystore"), b"ks").unwrap();
        let key = resolve_signing(tmp.path(), &none, tmp.path(), |_| None).unwrap();
        assert_eq!(
            key,
            SigningKey::Debug { keystore: tmp.path().join(".android/debug.keystore") }
        );
        let args: Vec<String> =
            apksigner_key_args(&key).into_iter().map(|a| a.into_string().unwrap()).collect();
        assert!(args.windows(2).any(|w| w == ["--ks-pass", "pass:android"]));
    }

    /// Regression: a local-mode dev/robot build must get INTERNET injected so
    /// the Robot bridge can bind its loopback listener (Android denies socket
    /// creation without it). Runtime-server builds already declare it; a
    /// production local build (no robot feature) must NOT be granted it.
    #[test]
    fn internet_injected_only_for_local_robot_builds() {
        let dev = vec!["runtime-core/dev".to_string()];
        let robot = vec!["robot".to_string()];
        let none: Vec<String> = vec![];

        // Local + robot/dev feature → needs it.
        assert!(local_robot_build_needs_internet(false, &dev));
        assert!(local_robot_build_needs_internet(false, &robot));

        // Local, no robot feature (production local build) → must NOT add it.
        assert!(!local_robot_build_needs_internet(false, &none));

        // Runtime-server mode already declares INTERNET → never injected here,
        // even with the dev feature on.
        assert!(!local_robot_build_needs_internet(true, &dev));
    }

    /// `asset!("images/logo.png")` on Android is
    /// `AssetManager.open("images/logo.png")`, i.e. APK entry
    /// `assets/images/logo.png`. Stages the tree, then (when `zip` /
    /// `unzip` are on PATH) splices it into a real archive and checks the
    /// entry name and that already-compressed media is STORED.
    #[test]
    fn android_apk_stages_assets_for_asset_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        fs::create_dir_all(project.join("assets/images")).unwrap();
        fs::write(project.join("assets/images/logo.png"), b"png-bytes").unwrap();
        fs::write(project.join("assets/data.json"), b"{}").unwrap();
        let root = tmp.path().join("android").join(APK_ASSETS_STAGING);
        assert!(stage_apk_assets(&project, &root).unwrap());
        assert!(root.join("assets/images/logo.png").is_file());

        let has = |tool: &str| Command::new(tool).arg("-v").output().is_ok();
        if !(has("zip") && has("unzip")) {
            eprintln!("zip/unzip not on PATH — skipping the archive half");
            return;
        }
        let apk = tmp.path().join("unsigned.apk");
        // Seed a non-empty archive, as aapt2 link's manifest-only APK is.
        fs::write(tmp.path().join("AndroidManifest.xml"), b"<manifest/>").unwrap();
        let ok = Command::new("zip")
            .arg(&apk)
            .arg("AndroidManifest.xml")
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success();
        assert!(ok);
        splice_assets_into_apk(&apk, &root).unwrap();
        let listing = Command::new("unzip").arg("-v").arg(&apk).output().unwrap();
        let listing = String::from_utf8_lossy(&listing.stdout);
        let line = |name: &str| {
            listing
                .lines()
                .find(|l| l.trim_end().ends_with(name))
                .unwrap_or_else(|| panic!("{name} missing from APK:\n{listing}"))
                .to_string()
        };
        assert!(line(" assets/images/logo.png").contains("Stored"), "{listing}");
        line(" assets/data.json");
        line(" AndroidManifest.xml");
    }

    #[test]
    fn android_project_without_assets_splices_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(APK_ASSETS_STAGING);
        assert!(!stage_apk_assets(tmp.path(), &root).unwrap());
    }
}
