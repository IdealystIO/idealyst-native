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
//!   zipalign              → page-align the .so for direct mmap
//!   apksigner             → sign with the debug keystore
//!   emulator -avd / adb   → boot a sim if none, install, launch
//! ```
//!
//! Why no Gradle: same reason we skipped Xcode projects. The APK is
//! a build artifact derivable from the user's platform-agnostic
//! crate + a few lines of project metadata. A Gradle project would
//! be a regenerated artifact in any case; cutting it out drops a
//! large dependency surface and makes builds noticeably faster.
//!
//! Limited to emulator + connected dev devices today. Release
//! signing (Play Store upload) needs a real keystore + version
//! management; debug keystore is the right default for development.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use build_ios::Manifest;

const MAIN_ACTIVITY_LOCAL_JAVA: &str = include_str!("../templates/MainActivity.java");
const NATIVE_BRIDGE_LOCAL_JAVA: &str = include_str!("../templates/NativeBridge.java");
const ANDROID_MANIFEST_LOCAL_XML: &str = include_str!("../templates/AndroidManifest.xml");
const MAIN_ACTIVITY_AAS_JAVA: &str = include_str!("../templates/MainActivityAas.java");
const NATIVE_BRIDGE_AAS_JAVA: &str = include_str!("../templates/NativeBridgeAas.java");
const ANDROID_MANIFEST_AAS_XML: &str = include_str!("../templates/AndroidManifestAas.xml");

const MIN_SDK_VERSION: u32 = 21;
const TARGET_SDK_VERSION: u32 = 34;

/// Default emulator to boot if no device is attached and the user
/// hasn't specified one explicitly. Matches what Android Studio
/// hands out from the AVD wizard for a typical Pixel-class profile.
const DEFAULT_AVD_PREFIX: &str = "Pixel";

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Build the Rust cdylib in release mode.
    pub release: bool,
    /// Specific AVD name to boot if no device is attached. When
    /// `None`, we pick the first available AVD whose name starts
    /// with `DEFAULT_AVD_PREFIX`, falling back to the first AVD.
    pub avd: Option<String>,
    /// Whether the Android process runs the user's `app()` locally
    /// or acts as a thin client connected to an AAS dev-host.
    pub mode: RunMode,
}

/// Mirrors `run-ios::RunMode` — same trade-offs (local self-contained
/// process vs. dev-host wire client). AAS mode swaps both the
/// generated cdylib (`backend-android` with the `aas-shell` feature)
/// and the Java glue (manifest gains multicast permission +
/// IdealystAppId; MainActivity acquires a MulticastLock, calls
/// `attachAas`, runs a `Handler` tick into `drainAas`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunMode {
    #[default]
    Local,
    Aas,
}

impl RunMode {
    fn is_aas(self) -> bool {
        matches!(self, RunMode::Aas)
    }
}

#[derive(Debug)]
pub struct RunArtifact {
    pub apk: PathBuf,
    /// adb serial of the device the APK was installed on
    /// (`emulator-5554`, a real device's USB serial, etc.).
    pub serial: String,
}

pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = build_ios::parse_manifest(&project_dir)?;
    let workspace_root = build_ios::find_workspace_root(&project_dir)?;

    // ── 1. Build the Rust cdylib ─────────────────────────────────
    let so = build_android::build(
        &project_dir,
        build_android::BuildOptions {
            release: opts.release,
            mode: match opts.mode {
                RunMode::Local => build_android::BuildMode::Local,
                RunMode::Aas => build_android::BuildMode::Aas,
            },
            ..Default::default()
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
    let adb = sdk.join("platform-tools/adb");

    // ── 3. Lay out build dir ─────────────────────────────────────
    // AAS and Local builds live in sibling dirs so their (different)
    // staged Java sources + APKs don't stomp each other.
    let app_subdir = if opts.mode.is_aas() {
        "android-aas/app"
    } else {
        "android/app"
    };
    let build_dir = workspace_root
        .join("target/idealyst")
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
    // its own `.so` and we want the AAS one to be obviously
    // distinguishable in the APK's `lib/<abi>/` directory.
    let lib_name = if opts.mode.is_aas() {
        format!("{}_android_aas_wrapper", manifest.lib_name)
    } else {
        format!("{}_android_wrapper", manifest.lib_name)
    };
    let (main_activity_tmpl, native_bridge_tmpl, manifest_tmpl) = if opts.mode.is_aas() {
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
    // The AAS manifest carries an extra IdealystAppId meta-data
    // entry that MainActivity reads at startup to know which Bonjour
    // service to filter on. Local mode doesn't need it.
    fs::write(
        &manifest_xml,
        render(manifest_tmpl, &[
            ("PACKAGE", manifest.app.require_bundle_id()?),
            ("APP_NAME", &xml_escape(&manifest.app.name)),
            ("APP_ID", &xml_escape(manifest.app.require_bundle_id()?)),
        ]),
    )?;

    // ── 5. javac → .class ───────────────────────────────────────
    let class_dir = build_dir.join("classes");
    fs::create_dir_all(&class_dir)?;
    compile_java(&java_dir, &class_dir, &android_jar)?;

    // ── 6. d8 → classes.dex ─────────────────────────────────────
    let dex_dir = build_dir.join("dex");
    fs::create_dir_all(&dex_dir)?;
    run_d8(&build_tools, &class_dir, &dex_dir, &android_jar)?;

    // ── 7. aapt2 link → manifest-only APK ───────────────────────
    let unsigned_apk = build_dir.join("unsigned.apk");
    run_aapt2_link(&build_tools, &manifest_xml, &android_jar, &unsigned_apk)?;

    // ── 8. zip in classes.dex + lib/<abi>/<so> ──────────────────
    splice_into_apk(&unsigned_apk, &dex_dir, &so, &build_dir)?;

    // ── 9. zipalign ─────────────────────────────────────────────
    let aligned_apk = build_dir.join("aligned.apk");
    run_zipalign(&build_tools, &unsigned_apk, &aligned_apk)?;

    // ── 10. apksigner sign ──────────────────────────────────────
    let signed_apk = build_dir.join(format!("{}.apk", manifest.name));
    let keystore = home_dir()?.join(".android/debug.keystore");
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
    run_apksigner(&build_tools, &aligned_apk, &keystore, &signed_apk)?;

    // ── 11. Ensure a device is online; boot an emulator if not ───
    let serial = ensure_device(&adb, &sdk, opts.avd.as_deref())?;
    eprintln!("[run-android] using device {serial}");

    // ── 12. adb install + launch ────────────────────────────────
    adb_install(&adb, &serial, &signed_apk)?;
    let component = format!("{}/.MainActivity", manifest.app.require_bundle_id()?);
    adb_launch(&adb, &serial, &component)?;

    Ok(RunArtifact {
        apk: signed_apk,
        serial,
    })
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

fn compile_java(java_dir: &Path, class_dir: &Path, android_jar: &Path) -> Result<()> {
    // Collect all .java files under java_dir.
    let mut sources: Vec<PathBuf> = Vec::new();
    visit_files(java_dir, "java", &mut sources)?;
    if sources.is_empty() {
        anyhow::bail!("no .java files found under {}", java_dir.display());
    }
    eprintln!("[run-android] javac → {}", class_dir.display());
    let mut cmd = Command::new("javac");
    cmd.arg("-classpath")
        .arg(android_jar)
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

fn run_d8(
    build_tools: &Path,
    class_dir: &Path,
    dex_dir: &Path,
    android_jar: &Path,
) -> Result<()> {
    let mut sources: Vec<PathBuf> = Vec::new();
    visit_files(class_dir, "class", &mut sources)?;
    if sources.is_empty() {
        anyhow::bail!("no .class files under {}", class_dir.display());
    }
    eprintln!("[run-android] d8 → {}", dex_dir.display());
    let mut cmd = Command::new(build_tools.join("d8"));
    cmd.arg("--lib")
        .arg(android_jar)
        .arg("--min-api")
        .arg(MIN_SDK_VERSION.to_string())
        .arg("--output")
        .arg(dex_dir);
    for s in &sources {
        cmd.arg(s);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", build_tools.join("d8").display()))?;
    if !status.success() {
        anyhow::bail!("d8 exited with {status}");
    }
    Ok(())
}

fn run_aapt2_link(
    build_tools: &Path,
    manifest_xml: &Path,
    android_jar: &Path,
    out_apk: &Path,
) -> Result<()> {
    eprintln!("[run-android] aapt2 link → {}", out_apk.display());
    let status = Command::new(build_tools.join("aapt2"))
        .arg("link")
        .arg("--manifest")
        .arg(manifest_xml)
        .arg("-I")
        .arg(android_jar)
        .arg("--min-sdk-version")
        .arg(MIN_SDK_VERSION.to_string())
        .arg("--target-sdk-version")
        .arg(TARGET_SDK_VERSION.to_string())
        .arg("-o")
        .arg(out_apk)
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

fn run_apksigner(
    build_tools: &Path,
    input: &Path,
    keystore: &Path,
    output: &Path,
) -> Result<()> {
    eprintln!("[run-android] apksigner sign → {}", output.display());
    // The debug keystore's password is `android` by convention. If
    // a user has rotated it we'll surface the apksigner failure;
    // they can regenerate by deleting `~/.android/debug.keystore`
    // (Android Studio re-creates on next open).
    let status = Command::new(build_tools.join("apksigner"))
        .arg("sign")
        .arg("--ks")
        .arg(keystore)
        .arg("--ks-pass")
        .arg("pass:android")
        .arg("--key-pass")
        .arg("pass:android")
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
