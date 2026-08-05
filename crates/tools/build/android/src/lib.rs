//! Android build orchestration for `idealyst build android`.
//!
//! Same shape as [`build-ios`](../build_ios/index.html): the user's
//! app crate is platform-agnostic (just `pub fn app() -> Element`),
//! and we generate an ephemeral wrapper crate that adds the
//! Android-specific bits — `cdylib` crate-type, `jni` glue,
//! `backend-android` dep, and JNI entry points the Kotlin side
//! calls via `NativeBridge.attach(context, root)`. The wrapper
//! lives at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/android/wrapper/
//! ```
//!
//! Unlike iOS, Android cross-compilation needs the NDK's linker —
//! a stock `cargo build --target aarch64-linux-android` will fail
//! at link time without it. We sidestep `cargo-ndk` (which a user
//! might not have installed) by generating the wrapper's own
//! `.cargo/config.toml` that points `linker` + `ar` at the NDK's
//! Clang wrapper directly. `ANDROID_NDK_HOME` is the one
//! environment variable we require; we resolve the prebuilt
//! toolchain directory from it.
//!
//! ## JNI symbol naming
//!
//! The Kotlin side defines `package <bundle_id>; object NativeBridge`
//! and expects native methods named
//! `Java_<bundle_id_mangled>_NativeBridge_attach` (and `..._detach`).
//! We bake the bundle-id-derived prefix into the generated wrapper
//! by running it through [`bundle_id_to_jni_package`], which applies
//! the JNI Native Method Naming spec: `.` → `_` (segment separator),
//! `_` → `_1` (underscore in a segment), `$` → `_00024` (inner-class
//! marker). So `com.example.nicho_portfolio` becomes
//! `com_example_nicho_1portfolio`, matching what the Kotlin runtime
//! mangler produces for `NativeBridge.attach` in that package.
//!
//! What's still TODO before this produces a runnable Android app:
//!
//! - APK assembly. Today this returns the path to the `.so`; a
//!   future `run-android` will mirror `run-ios`: generate Kotlin
//!   sources + AndroidManifest.xml, run `aapt2`/`d8`/`apksigner`
//!   directly, push to device via `adb`.
//! - Additional architectures (`armeabi-v7a`, `x86_64`) for
//!   broader emulator/device coverage. Today: arm64-v8a only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build with `--release`. Default: debug. Release builds are
    /// dramatically smaller (the framework + backend-android come
    /// to ~60MB in debug, ~5MB in release) and worth running before
    /// any actual device install testing.
    pub release: bool,
    /// Minimum Android API level the produced `.so` targets. NDK
    /// ships per-API clang wrappers (`aarch64-linux-android21-clang`,
    /// `..-android24-clang`, etc.); we pick the matching one.
    /// Android 21 (Lollipop) covers >99% of devices in 2026.
    pub api_level: u32,
    /// Whether the wrapper hosts the user's `app()` locally
    /// ([`BuildMode::Local`]) or acts as a thin runtime-server client
    /// ([`BuildMode::RuntimeServer`]). runtime-server-mode wrappers don't depend on the
    /// user crate at all — they delegate to
    /// `backend_android::runtime_server::{attach, drain, detach}` and let the
    /// dev-host run the reactive tree.
    pub mode: BuildMode,
    /// Where the wrapper Cargo.toml sources framework crates from.
    /// runtime-server mode requires `Workspace` because the runtime-server-host build crate
    /// must come out of the framework workspace's `target/`.
    pub source: FrameworkSource,
    /// Cargo features to enable on the cargo invocation. Forwarded
    /// as `--features <list>`. Used by `idealyst dev` to pass `dev`
    /// (→ `runtime-core/dev` + `runtime-shared/robot`) so the Robot
    /// bridge auto-starts.
    pub user_features: Vec<String>,
}

/// Which kind of cdylib wrapper to generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    /// User's `app()` runs in-process. Wrapper depends on the user
    /// crate and exports `Java_<pkg>_NativeBridge_attach(ctx, root)`
    /// + `_detach()`.
    Local,
    /// Thin runtime-server client. Wrapper imports `backend-android` with
    /// `runtime-server` on, doesn't depend on the user crate, exports
    /// `Java_<pkg>_NativeBridge_attachRuntimeServer(ctx, root, appId)` +
    /// `_drainRuntimeServer()` + `_detach()`. UI runs on the dev-host;
    /// commands stream over a WebSocket discovered via Bonjour.
    RuntimeServer,
}

// No `Default` impl — `source` has no sensible default; the CLI
// constructs it via `FrameworkSource::detect`.

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced `lib<project>_android.so`. Ready to be
    /// packaged into an APK under `lib/<abi>/`.
    pub dylib: PathBuf,
    /// Rust target triple the cdylib was built for.
    pub target_triple: &'static str,
    /// Android ABI directory name the `.so` belongs in inside an
    /// APK (`arm64-v8a`, `armeabi-v7a`, …). Matches `target_triple`.
    pub abi: &'static str,
    /// Wrapper crate directory.
    pub wrapper_dir: PathBuf,
    /// JNI package prefix the generated wrapper exports (e.g.
    /// `ai_truday_idealyst_docs`). The Kotlin side's package must
    /// match this (with dots in place of underscores) for the
    /// runtime lookup to find `NativeBridge.attach` / `detach`.
    pub jni_package: String,
}

pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    // runtime-server mode reaches into the framework workspace's `target/` for
    // the host's hotpatch builder (`build-runtime-server`); it can't be wired
    // through git deps. Fail loudly rather than producing a wrapper
    // that won't compile.
    if matches!(opts.mode, BuildMode::RuntimeServer) && !opts.source.is_workspace() {
        anyhow::bail!(
            "android runtime-server mode requires an in-tree idealyst framework checkout. \
             Run from inside the workspace, or set IDEALYST_FRAMEWORK_PATH."
        );
    }

    let ndk_home = std::env::var("ANDROID_NDK_HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!(
            "ANDROID_NDK_HOME is not set — point it at your NDK install \
             (e.g. ~/Library/Android/sdk/ndk/<version>/)"
        ))?;
    if !ndk_home.is_dir() {
        anyhow::bail!(
            "ANDROID_NDK_HOME={} does not exist or isn't a directory",
            ndk_home.display()
        );
    }
    let toolchain_bin = ndk_toolchain_bin(&ndk_home)?;

    let jni_package = bundle_id_to_jni_package(manifest.app.require_bundle_id()?)?;
    let target_triple = "aarch64-linux-android";
    let abi = "arm64-v8a";

    // runtime-server and Local wrappers live in sibling dirs so cargo's build
    // cache doesn't get confused by their (different) feature
    // resolutions of `backend-android`.
    let wrapper_subdir = match opts.mode {
        BuildMode::Local => "android/wrapper",
        BuildMode::RuntimeServer => "android-runtime-server/wrapper",
    };
    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join(wrapper_subdir);
    generate_wrapper(
        &wrapper_dir,
        &project_dir,
        &opts.source,
        &manifest,
        &jni_package,
        &toolchain_bin,
        target_triple,
        opts.api_level,
        opts.mode,
    )?;

    cargo_build(&wrapper_dir, target_triple, opts.release, &opts.user_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let dylib_name = match opts.mode {
        BuildMode::Local => format!("lib{}_android_wrapper.so", manifest.lib_name),
        BuildMode::RuntimeServer => format!("lib{}_android_aas_wrapper.so", manifest.lib_name),
    };
    // Wrapper's `.cargo/config.toml` redirects build output to the
    // resolved target dir (workspace's `target/` in-tree; the project's
    // own `target/` for external consumers). Sharing avoids
    // re-compiling deps that cargo already has cached for this source.
    let dylib = opts
        .source
        .cargo_target_dir(&project_dir)
        .join(target_triple)
        .join(profile)
        .join(&dylib_name);

    if !dylib.is_file() {
        anyhow::bail!(
            "cargo build reported success but {} was not produced",
            dylib.display(),
        );
    }

    Ok(BuildArtifact {
        dylib,
        target_triple,
        abi,
        wrapper_dir,
        jni_package,
    })
}

// ---------------------------------------------------------------------------
// NDK toolchain resolution
// ---------------------------------------------------------------------------

/// Locate the prebuilt toolchain `bin/` directory inside an NDK
/// install. NDK r25+ ships `darwin-x86_64` (runs via Rosetta on
/// Apple Silicon) and newer revisions ship `darwin-arm64` natively.
/// We probe both; whichever exists wins. Linux/Windows hosts get
/// the analogous probe.
fn ndk_toolchain_bin(ndk_home: &Path) -> Result<PathBuf> {
    let prebuilt = ndk_home.join("toolchains/llvm/prebuilt");
    // Listed in preference order — match the host arch first when
    // the NDK ships a native build, otherwise fall back to x86_64
    // (which works under Rosetta on Apple Silicon).
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["darwin-arm64", "darwin-x86_64"]
    } else if cfg!(target_os = "linux") {
        &["linux-x86_64"]
    } else if cfg!(target_os = "windows") {
        &["windows-x86_64"]
    } else {
        &[]
    };
    for c in candidates {
        let p = prebuilt.join(c).join("bin");
        if p.is_dir() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "could not find a prebuilt toolchain under {}. \
         Tried: {:?}. Is the NDK install complete?",
        prebuilt.display(),
        candidates
    )
}

// ---------------------------------------------------------------------------
// JNI package derivation
// ---------------------------------------------------------------------------

/// Translate a reverse-DNS bundle id (`com.example.nicho_portfolio`)
/// into the JNI symbol prefix (`com_example_nicho_1portfolio`),
/// honoring the JNI Native Method Naming spec:
///
/// - `_` → `_1`   (so a Java package segment `nicho_portfolio`
///   doesn't collide with the package-separator we emit for `.`).
/// - `$` → `_00024`  (inner-class separator; never appears in a
///   package name in practice, included for spec completeness).
/// - `.` → `_`   (package separator).
///
/// Escapes are applied BEFORE the dot→underscore pass so the
/// emitted `_` for a `.` stays a bare `_` (the segment separator),
/// while `_` characters that came from the bundle id itself end
/// up as `_1`. Without this both look the same on the Rust side
/// and `Java_com_example_nicho_portfolio_NativeBridge_attachRuntimeServer`
/// fails to dlsym because the Kotlin runtime mangles the same
/// method as `Java_com_example_nicho_1portfolio_NativeBridge_…`.
///
/// Pre-fix this function rejected `_`/`$` outright and asked the
/// author to rename their bundle id — fine for greenfield projects
/// but not viable for established packages.
fn bundle_id_to_jni_package(bundle_id: &str) -> Result<String> {
    if bundle_id.is_empty() {
        anyhow::bail!("bundle id is empty");
    }
    let escaped: String = bundle_id
        .chars()
        .flat_map(|c| match c {
            '_' => "_1".chars().collect::<Vec<_>>(),
            '$' => "_00024".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect();
    Ok(escaped.replace('.', "_"))
}

// ---------------------------------------------------------------------------
// Wrapper generation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    jni_package: &str,
    toolchain_bin: &Path,
    target_triple: &str,
    api_level: u32,
    mode: BuildMode,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;
    fs::create_dir_all(wrapper_dir.join(".cargo"))
        .with_context(|| format!("create {}", wrapper_dir.join(".cargo").display()))?;

    let wrapper_name = match mode {
        BuildMode::Local => format!("{}-android-wrapper", manifest.name),
        // NB: the package name MUST translate to the `.so` filename the
        // build + run sides expect (`lib{lib_name}_android_aas_wrapper.so`).
        // Cargo derives the dylib name from the package name with `-` → `_`,
        // so using `-android-runtime-server-wrapper` would emit
        // `lib<name>_android_runtime_server_wrapper.so` instead — paths
        // diverge and the post-build artifact-existence check fails with
        // "cargo build reported success but … was not produced". Naming
        // it `-android-aas-wrapper` here matches the expected `.so`.
        BuildMode::RuntimeServer => format!("{}-android-aas-wrapper", manifest.name),
    };
    // `runtime-core` + `runtime-shared` as DIRECT deps so the wrapper's
    // own `dev` feature can map onto them (cargo resolves `<dep>/<feat>`
    // only for direct dependencies): the facade carries the catalog
    // anchor + emission gate, runtime-shared the substrate names the JNI
    // bridge spells directly (`robot::bridge`, `set_initial_path`).
    let runtime_core_dep = source.dep("crates/runtime/core", &[]);
    let shared_dep = source.dep("crates/runtime/shared", &[]);
    // `async-driver` brings in `spawn_async` + the executor wiring
    // backend-android installs through `install_render_loop`. Without
    // it the framework's `spawn_async` is a no-op and any async path
    // (the website's `Simulator` calling `host_wgpu::mount(...).await`,
    // SDK code awaiting GPU init) silently never resolves — the
    // simulator's wgpu surface stays blank. Mirrors the iOS wrapper,
    // whose backend-ios-mobile dep enables `async-driver` for the same
    // reason.
    //
    // `new-core` compiles the backend's boot path
    // (`newcore::start`/`stop` + the Host/caps impls and flush driver)
    // that `attach` below calls into. The feature is vacuous once
    // backend-android-mobile makes its contents unconditional; drop it
    // from this list at that point.
    let bandroid_local_dep =
        source.dep("crates/backend/android/mobile", &["async-driver"]);
    let bandroid_aas_dep = source.dep(
        "crates/backend/android/mobile",
        &["runtime-server", "async-driver"],
    );
    // First-party SDKs bundled into the runtime-server client so their
    // native chrome (Drawer navigator, code block, table) renders over
    // the wire on device. They depend on `backend-android-mobile`, so
    // the backend can't depend on them (Cargo cycle) — they're pulled
    // in here at the generated wrapper, which sits above both. The
    // wrapper calls each SDK's `register` via
    // `attach_with_url_with_register`. Mirrors the per-app web wrapper's
    // `register_extensions` and the iOS `backend-ios-rs-shell`.
    let drawer_navigator_dep = source.dep("crates/sdk/client/navigators/drawer", &[]);
    let codeblock_dep = source.dep("crates/sdk/client/codeblock", &[]);
    let table_dep = source.dep("crates/sdk/client/table", &[]);

    let cargo_toml = match mode {
        BuildMode::Local => format!(
            r#"# GENERATED by `idealyst build android`. Do not edit — rewritten
# every build. Run `idealyst scaffold android` to materialize an
# editable copy of this wrapper into your repo (once that command
# lands).

# Empty `[workspace]` declares this wrapper as a standalone project
# even though it physically lives under the main workspace's
# `target/idealyst/...`. Without it, cargo refuses to build because
# the parent Cargo.toml has `[workspace]` and would otherwise claim
# this directory as a member.
[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[lib]
# `cdylib` is what `System.loadLibrary("..._android_wrapper")` on the
# Kotlin side resolves. Loading the .so also fires the Rust runtime's
# `JNI_OnLoad` (supplied by the `jni` crate), which caches the
# `JavaVM` so the backend can attach threads on demand.
crate-type = ["cdylib"]

[features]
# `idealyst dev` builds with `--features dev`: the catalog + automation
# surface (`runtime-core/dev`) plus the bridge TRANSPORT
# (`runtime-shared/robot`), which is what makes
# `robot::bridge::set_app_identity` in the JNI bridge resolve.
dev = ["runtime-core/dev", "runtime-shared/robot"]

[dependencies]
runtime-core = {runtime_core_dep}
runtime-shared = {shared_dep}
{user_name} = {user_dep}

[target.'cfg(target_os = "android")'.dependencies]
backend-android-mobile = {bandroid_dep}
jni = "0.21"
log = "0.4"
"#,
            runtime_core_dep = runtime_core_dep,
            shared_dep = shared_dep,
            bandroid_dep = bandroid_local_dep,
            user_name = manifest.name,
            // Plain path dep on the user crate: the app's own defaults
            // select its feature set, and there is one core.
            user_dep = format!("{{ path = \"{}\" }}", project_dir.display()),
        ),
        BuildMode::RuntimeServer => format!(
            r#"# GENERATED by `idealyst build android` (runtime-server mode). Do not edit —
# rewritten every build. Unlike the local wrapper, this one doesn't
# depend on the user crate at all: the runtime-server client renders whatever
# the dev-host sends over the wire, not the in-process `app()`.

[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[features]
# Same dev surface as the local wrapper (see its `[features]` block).
dev = ["runtime-core/dev", "runtime-shared/robot"]

[dependencies]
runtime-core = {runtime_core_dep}
runtime-shared = {shared_dep}

[target.'cfg(target_os = "android")'.dependencies]
# `runtime-server` feature on backend-android compiles in the cross-platform
# `RuntimeServerShell` from dev-client and exposes `backend_android::runtime_server::{{attach,
# drain, detach}}` Rust helpers — the JNI bridge below is a thin
# trampoline over those.
backend-android-mobile = {bandroid_dep}
# First-party SDKs bundled into the runtime-server client so their
# native chrome renders over the wire on device. The JNI bridge below
# registers them on the RS client backend via
# `attach_with_url_with_register`.
drawer-navigator = {drawer_navigator_dep}
codeblock = {codeblock_dep}
table = {table_dep}
jni = "0.21"
log = "0.4"
"#,
            runtime_core_dep = runtime_core_dep,
            shared_dep = shared_dep,
            bandroid_dep = bandroid_aas_dep,
            drawer_navigator_dep = drawer_navigator_dep,
            codeblock_dep = codeblock_dep,
            table_dep = table_dep,
        ),
    };

    let lib_rs = match mode {
        BuildMode::Local => format!(
            r#"//! GENERATED by `idealyst build android`. JNI bridge that mounts
//! `{lib}::app()` underneath the Android `ViewGroup` the Kotlin side
//! hands us via `NativeBridge.attach(context, root)`. The exported
//! symbols' fully-qualified names — `Java_{jni}_NativeBridge_attach`
//! and `..._detach` — must match the Kotlin side's
//! `package {kotlin_pkg}; object NativeBridge`.

#![cfg(target_os = "android")]

use backend_android::AndroidBackend;
use jni::objects::{{JClass, JObject, JString}};
use jni::JNIEnv;
use std::cell::RefCell;
use std::rc::Rc;

/// Attach the framework to an Android `Context` + a parent
/// `ViewGroup`. Idempotent: re-calling tears the previous tree down
/// before building a new one.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_attach<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
    root: JObject<'local>,
) {{
    let attach_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        let context_global = env.new_global_ref(&context).expect("new_global_ref context");
        let root_global = env.new_global_ref(&root).expect("new_global_ref root");

        // Register the project's identity for the Robot bridge mDNS
        // advertisement. Bridge thread (started inside `mount`) reads
        // from here when binding + advertising. No-op without `dev`.
        #[cfg(feature = "dev")]
        {{
            ::runtime_shared::robot::bridge::set_app_identity(
                ::runtime_shared::robot::bridge::AppIdentity {{
                    name: "{app_name}".to_string(),
                    bundle_id: Some("{bundle_id}".to_string()),
                    project_root: ::std::option::Option::None,
                }},
            );
        }}

        let backend = Rc::new(RefCell::new(AndroidBackend::new(
            context_global,
            root_global,
        )));
        // Install a `Weak` self-handle so cross-platform animation
        // drivers (the welcome example's `drive_av` and equivalents)
        // can reach the backend through `backend_android::set_animated_*`
        // without threading the `Rc<RefCell<>>` through every closure.
        // Same shape as `backend_ios::install_global_self`. Must be
        // called BEFORE `mount`, because `app()` subscribes the AVs
        // synchronously inside `mount`.
        backend_android::install_global_self(Rc::downgrade(&backend));
        // Main-Looper-backed scheduler so `after_ms` /
        // `schedule_microtask` delay correctly. Without it
        // `after_ms` fires the callback synchronously at call
        // time, which breaks the long-press recognizer and
        // every other timer-driven feature.
        backend_android::install_scheduler();
        // Install the Choreographer-driven raf-loop so
        // `runtime_core::driver::render_loop(...)` callbacks fire
        // each vsync. Required by anything that drives per-frame
        // work through the driver: the website's `Simulator`
        // component (host-android-mobile's wgpu render-loop), the
        // welcome scaffold's planet animation, etc. Without it
        // those callbacks register but never run — symptom: the
        // embedded wgpu preview stays blank because draw_frame is
        // never invoked.
        backend_android::install_render_loop();
        // Mount the project's `scene_app()` scene tree through the
        // vocabulary registry. `newcore::start` owns everything from
        // "backend is wired to the host ViewGroup" onward and retains
        // the mounted app in its own slot (torn down by `newcore::stop`
        // in `detach`). The user crate's registry-generic
        // `register_scene_extensions` seam runs after
        // `register_builtins` — this is how SDK payload handlers
        // register their per-backend implementations, and an
        // unregistered payload panics at realize.
        backend_android::newcore::start(
            backend,
            {lib}::register_scene_extensions,
            {lib}::scene_app,
        );

        log::info!("idealyst: attach complete");
    }}));
    // A swallowed panic here means "app launches to a blank screen
    // with NO log line" — the worst debugging surface there is (a
    // silent no-mount hid a real boot bug during the scene-boot
    // bring-up). Log the payload; the process stays alive (unwinding
    // across the JNI boundary is UB, so we cannot rethrow).
    if let Err(payload) = attach_result {{
        let msg = payload
            .downcast_ref::<&'static str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<panic payload not a string>".to_string());
        log::error!("idealyst: attach PANICKED — nothing was mounted: {{msg}}");
    }}
}}

/// Detach the active mount. `newcore::stop` drops the realized tree,
/// the flush driver, and the world — releasing every signal/effect and
/// the per-element click callbacks with them.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_detach<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::newcore::stop();
}}

/// MainActivity.onConfigurationChanged trampoline. Triggers a
/// framework layout pass so rotation / multi-window / density
/// changes reflow against the host root's new dimensions. The
/// manifest's `android:configChanges` declaration keeps the Activity
/// alive across these events; this notify is how the framework
/// hears about them.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_notifyConfigChanged<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::notify_config_changed();
}}

/// Cold-start deep-link trampoline. `MainActivity.onCreate` calls this
/// with the launch `Intent`'s data-URI PATH (e.g. `/encounters/abc`)
/// BEFORE `attach`, so the navigator walker's synchronous initial mount
/// resolves the deep-linked screen and reconstructs the back stack. When
/// the Activity launched without a `VIEW`/app-link intent, the Java side
/// never calls this and behavior is unchanged.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_setLaunchPath<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
) {{
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        // `get_string` errors on a null jstring, so this also handles the
        // "no deep link" case where Java passes null.
        if let Ok(js) = env.get_string(&path) {{
            let s = js.to_str().unwrap_or("").to_string();
            if !s.is_empty() {{
                runtime_shared::set_initial_path(Some(s));
            }}
        }}
    }}));
}}

/// Export the robot relay URL (baked into the manifest meta-data by
/// `idealyst dev` + reached via `adb reverse`) so the Robot bridge's walker
/// DIALS the host relay instead of self-hosting a device-local TCP listener.
/// Called from MainActivity BEFORE `attach`, so it's set before `mount` runs
/// the walker. NOT gated on the wrapper's `dev` feature — it just sets an
/// env var, a no-op in release where the meta-data is absent so Java
/// never calls it.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_setRobotRelayUrl<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    url: JString<'local>,
) {{
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        if let Ok(js) = env.get_string(&url) {{
            let s = js.to_str().unwrap_or("").to_string();
            if !s.is_empty() {{
                ::std::env::set_var("IDEALYST_ROBOT_RELAY_URL", s);
            }}
        }}
    }}));
}}
"#,
            lib = manifest.lib_name,
            jni = jni_package,
            kotlin_pkg = manifest.app.require_bundle_id()?,
            app_name = manifest.name,
            bundle_id = manifest.app.require_bundle_id()?,
        ),
        BuildMode::RuntimeServer => format!(
            r#"//! GENERATED by `idealyst build android` (runtime-server mode). JNI trampolines
//! over `backend_android::runtime_server::{{attach_with_url, drain, detach}}`.
//! The dev-host runs the user's reactive tree; this side just feeds
//! incoming wire commands into an `RuntimeServerClient<AndroidBackend>` on the
//! UI thread.

#![cfg(target_os = "android")]

use jni::objects::{{JClass, JObject, JString}};
use jni::JNIEnv;

/// Stand up the runtime-server client against the dev-server URL the
/// CLI baked into the manifest's `IdealystRuntimeServerUrl` meta-data
/// entry. The Activity reads that value from the manifest and passes
/// it here. mDNS / Bonjour is no longer used.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_attachRuntimeServerUrl<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
    root: JObject<'local>,
    url: JString<'local>,
) {{
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        let url_str: String = match env.get_string(&url) {{
            Ok(s) => s.into(),
            Err(e) => {{
                log::error!("attachRuntimeServerUrl: bad url JString: {{}}", e);
                return;
            }}
        }};
        // No client-side SDK registration. Under runtime v2 an SDK
        // primitive is a scene payload whose handler installs on a
        // `runtime_scene::Registry<H>`, and in runtime-server mode the
        // scene is realized on the HOST against
        // `Registry<WireRecordingBackend>` (the dev-server sidecar's
        // `register_scene_extensions_recorder` seam). So the handler
        // already ran server-side and what arrives here is ordinary
        // primitive commands — there is nothing for the client to
        // register. The old body called each SDK's
        // `register(&mut AndroidBackend)`, which was the old-core
        // `Element::External` client registry; that registry is gone.
        // (Delta, hot-reload only: an SDK that type-dispatches to a
        // backend-CONCRETE handler — e.g. `codeblock`'s native Android
        // mount — renders its portable variant here, because the
        // registry it sees is the recorder's. Local device builds go
        // through `register_scene_extensions` and are unaffected.)
        backend_android::runtime_server::attach_with_url_with_register(
            &mut env,
            context,
            root,
            &url_str,
            |_backend| {{}},
        );
    }}));
}}

/// UI-thread drain. Pulls pending `DevToApp` messages off the
/// worker thread's channel and applies them through the
/// `RuntimeServerClient<AndroidBackend>`. Called from the Kotlin Handler tick
/// every ~16ms; cheap when idle.
///
/// JNI exception cleanup: a panic inside the drain (typically from
/// a JNI call returning `Err(JavaException)`) is caught and logged
/// by the panic hook in `backend_android::aas`, but the pending
/// Java exception remains set on `JNIEnv`. If we returned with that
/// exception unhandled, ART would crash the process the moment Java
/// resumed. We clear it here at the boundary — the cost is
/// swallowing the exception silently, but the panic hook already
/// logged the underlying cause to logcat.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_drainRuntimeServer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::runtime_server::drain();
    if env.exception_check().unwrap_or(false) {{
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }}
}}

/// Tear down the active runtime-server shell. Called from `onDestroy`.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_detach<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::runtime_server::detach();
}}

/// MainActivity.onConfigurationChanged trampoline. Triggers a
/// framework layout pass so rotation / multi-window / density
/// changes reflow against the host root's new dimensions.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_notifyConfigChanged<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::notify_config_changed();
}}

/// MainActivity's OnLayoutChangeListener trampoline. The Kotlin
/// side reports the root FrameLayout's pixel dimensions + the
/// current display density on every layout pass; the runtime-server shell
/// dedupes consecutive identical reports and emits an
/// `AppToDev::ViewportChanged` only on real changes.
///
/// Pre-fix the Android shell shipped `viewport: None` in its
/// Hello (since `View.getWidth()` is zero in `onCreate`) and
/// never recovered, leaving the sidecar's `RecordingViewOps::
/// frame()` reads stuck on the 393×800 mobile fallback.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_reportViewport<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    width_px: jni::sys::jint,
    height_px: jni::sys::jint,
    density: jni::sys::jfloat,
) {{
    backend_android::runtime_server::report_viewport(width_px, height_px, density);
}}
"#,
            jni = jni_package,
        ),
    };

    // `.cargo/config.toml` points the Android cross-compile at the
    // NDK's per-API clang wrapper. Without this — and without
    // `cargo-ndk` on the PATH — cargo can't link the cdylib and
    // fails with an obscure "ld: unknown option" or similar.
    let clang_wrapper = toolchain_bin.join(format!("{target_triple}{api_level}-clang"));
    let ar = toolchain_bin.join("llvm-ar");
    // Share `target/` with whatever the source resolved to (the
    // framework workspace's `target/` in-tree, the project's own
    // `target/` for external consumers). Common deps (the runtime,
    // dev-client, backend-android) don't recompile from scratch for
    // the wrapper that way. Cross-target artifacts live under
    // `<target>/aarch64-linux-android/...` so they coexist
    // peacefully with host-target artifacts in the same directory.
    let workspace_target = source.cargo_target_dir(project_dir);
    let cargo_config = format!(
        r#"# GENERATED. Points the Android cross-compile at the NDK's
# Clang wrapper so cargo can link the cdylib without `cargo-ndk`,
# and shares the workspace's `target/` so common dependencies
# aren't recompiled per-wrapper.

[build]
target-dir = "{target_dir}"

[target.{target_triple}]
linker = "{linker}"
ar = "{ar}"
"#,
        target_dir = workspace_target.display(),
        target_triple = target_triple,
        linker = clang_wrapper.display(),
        ar = ar.display(),
    );

    if !clang_wrapper.is_file() {
        anyhow::bail!(
            "expected NDK clang wrapper at {} but it doesn't exist. \
             Either the NDK install is incomplete or the api_level ({}) \
             isn't supported by this NDK revision.",
            clang_wrapper.display(),
            api_level,
        );
    }

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
    fs::write(wrapper_dir.join(".cargo/config.toml"), cargo_config)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo invocation
// ---------------------------------------------------------------------------

fn cargo_build(
    wrapper_dir: &Path,
    target: &str,
    release: bool,
    user_features: &[String],
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--target", target]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }

    eprintln!(
        "[build-android] cargo build --target {target}{}{} (in {})",
        if release { " --release" } else { "" },
        if user_features.is_empty() {
            String::new()
        } else {
            format!(" --features {}", user_features.join(","))
        },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("cargo build exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Regression coverage for the JNI bundle-id mangler.
    //!
    //! Pre-fix the function rejected `_` and `$` outright — usable
    //! reverse-DNS bundle ids like `com.example.nicho_portfolio`
    //! were unbuildable. Fix maps these per the JNI Native Method
    //! Naming spec so the emitted `Java_<pkg>_NativeBridge_…`
    //! symbols match what Kotlin/Java's runtime mangler produces.
    use super::bundle_id_to_jni_package;

    #[test]
    fn plain_bundle_id_dots_to_underscores() {
        assert_eq!(
            bundle_id_to_jni_package("com.example.app").unwrap(),
            "com_example_app",
        );
    }

    #[test]
    fn underscores_escape_as_underscore_one() {
        // Real-world case from the nicho-portfolio scaffolding —
        // the user has `_` in their bundle id and (pre-fix) the
        // Android build refused to proceed.
        assert_eq!(
            bundle_id_to_jni_package("com.example.nicho_portfolio").unwrap(),
            "com_example_nicho_1portfolio",
        );
    }

    #[test]
    fn dollar_signs_escape_as_underscore_zero_zero_zero_two_four() {
        assert_eq!(
            bundle_id_to_jni_package("com.example.foo$bar").unwrap(),
            "com_example_foo_00024bar",
        );
    }

    #[test]
    fn mixed_underscore_and_dollar_compose_independently() {
        // Underscore escape, dollar escape, AND dot separator all
        // in the same id — must not interfere with one another.
        assert_eq!(
            bundle_id_to_jni_package("com.x_y.a$b").unwrap(),
            "com_x_1y_a_00024b",
        );
    }

    #[test]
    fn empty_bundle_id_errors() {
        assert!(bundle_id_to_jni_package("").is_err());
    }
}

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regression for `build-android`.
    //!
    //! Both modes' wrappers must declare `runtime-core` +
    //! `runtime-shared` as direct deps and map them through a
    //! wrapper-local `dev` feature, so the launcher's dev `--features`
    //! invocation resolves (cargo only accepts `<dep>/<feat>` for a
    //! direct dependency). Additionally:
    //!  - Local mode must path-dep the user crate (the cdylib's
    //!    JNI bridge calls `<user>::app()` in-process).
    //!  - Runtime-server mode must NOT depend on the user crate —
    //!    that ownership belongs to the sidecar; if the wrapper
    //!    accidentally re-acquired the dep, the Android binary
    //!    would link two copies of the user code with diverging
    //!    reactive kernels and `Element` type mismatches
    //!    would surface at the cdylib's boundary.
    //!
    //! These tests run only the generation step (sub-ms) — no
    //! cargo build, no NDK toolchain access.

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
                // Added by the jobs SDK (b4531004); the fake manifest
                // has no worker sidecar.
                worker_bin: None,
                worker_manifest: None,
                web: Default::default(),
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }

    fn run_generator(mode: BuildMode) -> (std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let workspace_root = tmp.path().join("workspace");
        let toolchain_bin = tmp.path().join("ndk_bin");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&toolchain_bin).unwrap();
        // `generate_wrapper` probes for the NDK clang wrapper before
        // writing the .cargo/config.toml linker pin. Stub one so the
        // existence check passes — we never actually invoke it.
        std::fs::write(toolchain_bin.join("aarch64-linux-android21-clang"), b"").unwrap();
        let manifest = fake_manifest();
        let source = FrameworkSource::Workspace { root: workspace_root };
        generate_wrapper(
            &wrapper_dir,
            &project_dir,
            &source,
            &manifest,
            "ai_example_demo",
            &toolchain_bin,
            "aarch64-linux-android",
            21,
            mode,
        )
        .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    fn parse(toml_text: &str) -> toml::Value {
        toml::from_str(toml_text).expect("valid TOML")
    }

    /// Both wrappers must carry the dev-surface deps + the `dev`
    /// feature that maps onto them; without it the launcher's dev
    /// `--features` spec fails at cargo time and the MCP catalog is
    /// empty.
    fn assert_dev_surface(cargo: &str) {
        let parsed = parse(cargo);
        let deps = parsed.get("dependencies").expect("deps table");
        assert!(
            deps.get("runtime-core").is_some() && deps.get("runtime-shared").is_some(),
            "Android wrapper missing the dev-surface direct deps. Got:\n{cargo}",
        );
        let dev: Vec<&str> = parsed
            .get("features")
            .and_then(|f| f.get("dev"))
            .and_then(|d| d.as_array())
            .expect("Android wrapper must declare a `dev` feature")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            dev.contains(&"runtime-core/dev") && dev.contains(&"runtime-shared/robot"),
            "`dev` must switch on the catalog anchor AND the bridge \
             transport; got {dev:?}",
        );
    }

    #[test]
    fn local_wrapper_has_dev_surface() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        assert_dev_surface(&std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap());
    }

    #[test]
    fn local_wrapper_path_deps_user_crate() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed = parse(&cargo);
        let user_dep = parsed
            .get("dependencies")
            .and_then(|d| d.get("demo"))
            .expect("local Android wrapper deps the user crate by name");
        assert!(
            user_dep.get("path").is_some(),
            "user-crate dep must be a path dep; got {:?}",
            user_dep,
        );
    }

    /// The Local wrapper boots the scene core: `backend-android-mobile`
    /// carries the boot feature, `attach` mounts `scene_app()` through
    /// `newcore::start` with the app's registry-generic
    /// `register_scene_extensions` seam, and `detach` calls
    /// `newcore::stop()`. The user-crate dep pins no core feature.
    #[test]
    fn local_wrapper_boots_scene_core_with_registration_seam() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::Local);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        assert!(
            !cargo.contains("old-core") && !cargo.contains("default-features = false"),
            "user-crate dep must be a plain path dep:\n{cargo}",
        );
        let lib_rs = std::fs::read_to_string(wrapper_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.contains("backend_android::newcore::start("),
            "attach must mount through newcore::start:\n{lib_rs}",
        );
        assert!(
            lib_rs.contains("demo::register_scene_extensions"),
            "attach must pass the app's scene-registration seam:\n{lib_rs}",
        );
        assert!(
            lib_rs.contains("demo::scene_app"),
            "attach must mount the app's scene_app():\n{lib_rs}",
        );
        assert!(
            lib_rs.contains("backend_android::newcore::stop()"),
            "detach must tear the mounted app down:\n{lib_rs}",
        );
        assert!(
            !lib_rs.contains("#[cfg(feature = \"new-core\")]")
                && !lib_rs.contains("runtime_core::mount("),
            "there is one boot path — no cfg'd core branches:\n{lib_rs}",
        );
    }

    #[test]
    fn runtime_server_wrapper_has_dev_surface() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::RuntimeServer);
        assert_dev_surface(&std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap());
    }

    #[test]
    fn runtime_server_wrapper_does_not_dep_user_crate() {
        let (wrapper_dir, _tmp) = run_generator(BuildMode::RuntimeServer);
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed = parse(&cargo);
        assert!(
            parsed
                .get("dependencies")
                .and_then(|d| d.get("demo"))
                .is_none(),
            "runtime-server Android wrapper must NOT depend on the user crate \
             (the sidecar owns it); leaks two reactive kernels. Got:\n{cargo}",
        );
    }
}
