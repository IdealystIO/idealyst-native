//! Android build orchestration for `idealyst build android`.
//!
//! Same shape as [`build-ios`](../build_ios/index.html): the user's
//! app crate is platform-agnostic (just `pub fn app() -> Primitive`),
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
//! `Java_<bundle_id_with_dots_to_underscores>_NativeBridge_attach`
//! and `..._detach`. We bake the bundle-id-derived prefix into the
//! generated wrapper. (Underscores in the bundle id would need
//! `_1` escaping per the JNI spec, but reverse-DNS app ids almost
//! never have underscores; we error out if they do rather than
//! silently producing a mismatched symbol.)
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
    /// ([`BuildMode::Local`]) or acts as a thin AAS client
    /// ([`BuildMode::Aas`]). AAS-mode wrappers don't depend on the
    /// user crate at all — they delegate to
    /// `backend_android::aas::{attach, drain, detach}` and let the
    /// dev-host run the reactive tree.
    pub mode: BuildMode,
    /// Where the wrapper Cargo.toml sources framework crates from.
    /// AAS mode requires `Workspace` because the AAS-host build crate
    /// must come out of the framework workspace's `target/`.
    pub source: FrameworkSource,
    /// Cargo features to enable on the cargo invocation. Forwarded
    /// as `--features <list>`. Used by `idealyst dev` to pass
    /// `framework-core/dev` so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
}

/// Which kind of cdylib wrapper to generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    /// User's `app()` runs in-process. Wrapper depends on the user
    /// crate and exports `Java_<pkg>_NativeBridge_attach(ctx, root)`
    /// + `_detach()`.
    Local,
    /// Thin AAS client. Wrapper imports `backend-android` with
    /// `aas-shell` on, doesn't depend on the user crate, exports
    /// `Java_<pkg>_NativeBridge_attachAas(ctx, root, appId)` +
    /// `_drainAas()` + `_detach()`. UI runs on the dev-host;
    /// commands stream over a WebSocket discovered via Bonjour.
    Aas,
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

    // AAS mode reaches into the framework workspace's `target/` for
    // the host's hotpatch builder (`build-aas`); it can't be wired
    // through git deps. Fail loudly rather than producing a wrapper
    // that won't compile.
    if matches!(opts.mode, BuildMode::Aas) && !opts.source.is_workspace() {
        anyhow::bail!(
            "android AAS mode requires an in-tree idealyst framework checkout. \
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

    // AAS and Local wrappers live in sibling dirs so cargo's build
    // cache doesn't get confused by their (different) feature
    // resolutions of `backend-android`.
    let wrapper_subdir = match opts.mode {
        BuildMode::Local => "android/wrapper",
        BuildMode::Aas => "android-aas/wrapper",
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
        BuildMode::Aas => format!("lib{}_android_aas_wrapper.so", manifest.lib_name),
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

/// Translate a reverse-DNS bundle id (`ai.truday.idealyst.docs`)
/// into the JNI symbol prefix (`ai_truday_idealyst_docs`). The JNI
/// mangling spec also escapes embedded underscores as `_1` and `$`
/// as `_00024`; we reject bundle ids that would need that escaping
/// because we have no good way to communicate the mismatch back to
/// the Kotlin side, and reverse-DNS app ids practically never
/// contain those characters.
fn bundle_id_to_jni_package(bundle_id: &str) -> Result<String> {
    if bundle_id.contains('_') || bundle_id.contains('$') {
        anyhow::bail!(
            "bundle id {:?} contains characters JNI requires escaping ('_' or '$'); \
             rename in `[package.metadata.idealyst.app].bundle_id` or extend \
             this builder to emit the JNI-escaped form",
            bundle_id,
        );
    }
    if bundle_id.is_empty() {
        anyhow::bail!("bundle id is empty");
    }
    Ok(bundle_id.replace('.', "_"))
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
        BuildMode::Aas => format!("{}-android-aas-wrapper", manifest.name),
    };
    let fcore_dep = source.dep("crates/framework/core", &[]);
    let bandroid_local_dep = source.dep("crates/backend/android/mobile", &[]);
    let bandroid_aas_dep = source.dep("crates/backend/android/mobile", &["aas-shell"]);

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

[dependencies]
framework-core = {fcore_dep}
{user_name} = {{ path = "{user_path}" }}

[target.'cfg(target_os = "android")'.dependencies]
backend-android-mobile = {bandroid_dep}
jni = "0.21"
log = "0.4"
"#,
            fcore_dep = fcore_dep,
            bandroid_dep = bandroid_local_dep,
            user_name = manifest.name,
            user_path = project_dir.display(),
        ),
        BuildMode::Aas => format!(
            r#"# GENERATED by `idealyst build android` (AAS mode). Do not edit —
# rewritten every build. Unlike the local wrapper, this one doesn't
# depend on the user crate at all: the AAS client renders whatever
# the dev-host sends over the wire, not the in-process `app()`.

[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
framework-core = {fcore_dep}

[target.'cfg(target_os = "android")'.dependencies]
# `aas-shell` feature on backend-android compiles in the cross-platform
# `AasShell` from dev-client and exposes `backend_android::aas::{{attach,
# drain, detach}}` Rust helpers — the JNI bridge below is a thin
# trampoline over those.
backend-android-mobile = {bandroid_dep}
jni = "0.21"
log = "0.4"
"#,
            fcore_dep = fcore_dep,
            bandroid_dep = bandroid_aas_dep,
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
use jni::objects::{{JClass, JObject}};
use jni::JNIEnv;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {{
    /// `render` returns an `Owner` that must outlive the mounted UI.
    /// Stashed here so it survives `attach` returning.
    static OWNER: RefCell<Option<framework_core::Owner>> = const {{ RefCell::new(None) }};
}}

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
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        let context_global = env.new_global_ref(&context).expect("new_global_ref context");
        let root_global = env.new_global_ref(&root).expect("new_global_ref root");

        OWNER.with(|slot| slot.borrow_mut().take());

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
        // `mount` runs `app()` inside the root reactive scope so
        // top-level `effect!` / `signal!` / `Ref::new` calls in
        // `app()` adopt the scope. See `framework_core::mount` docs.
        let owner = framework_core::mount(backend, {lib}::app);
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));

        log::info!("idealyst: attach complete");
    }}));
}}

/// Detach the active mount. Drops every signal/effect and releases
/// the per-element click callbacks.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_detach<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    OWNER.with(|slot| slot.borrow_mut().take());
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
"#,
            lib = manifest.lib_name,
            jni = jni_package,
            kotlin_pkg = manifest.app.require_bundle_id()?,
        ),
        BuildMode::Aas => format!(
            r#"//! GENERATED by `idealyst build android` (AAS mode). JNI trampolines
//! over `backend_android::aas::{{attach, attach_with_url, drain, detach}}`.
//! The dev-host runs the user's reactive tree; this side just feeds
//! incoming wire commands into an `AasClient<AndroidBackend>` on the
//! UI thread.

#![cfg(target_os = "android")]

use jni::objects::{{JClass, JObject, JString}};
use jni::JNIEnv;

/// Discover a Bonjour-advertised dev-host with the given app id and
/// stand up the AAS client. Used when the platform's network can
/// see the host's mDNS broadcasts — typical for physical devices on
/// the same Wi-Fi as the dev Mac.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_attachAas<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
    root: JObject<'local>,
    app_id: JString<'local>,
) {{
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        let app_id_str: String = match env.get_string(&app_id) {{
            Ok(s) => s.into(),
            Err(e) => {{
                log::error!("attachAas: bad app_id JString: {{}}", e);
                return;
            }}
        }};
        backend_android::aas::attach(&mut env, context, root, &app_id_str);
    }}));
}}

/// Direct-URL variant for the emulator path. The CLI sets up
/// `adb reverse tcp:<port> tcp:<port>` so the host port is reachable
/// at `127.0.0.1:<port>` from inside the emulator, then bakes the
/// resulting URL into manifest meta-data (`IdealystAasUrl`). When
/// MainActivity sees that meta-data, it calls this instead of
/// attachAas — skipping Bonjour entirely.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_attachAasUrl<'local>(
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
                log::error!("attachAasUrl: bad url JString: {{}}", e);
                return;
            }}
        }};
        backend_android::aas::attach_with_url(&mut env, context, root, &url_str);
    }}));
}}

/// UI-thread drain. Pulls pending `DevToApp` messages off the
/// worker thread's channel and applies them through the
/// `AasClient<AndroidBackend>`. Called from the Kotlin Handler tick
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
pub extern "system" fn Java_{jni}_NativeBridge_drainAas<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::aas::drain();
    if env.exception_check().unwrap_or(false) {{
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }}
}}

/// Tear down the active AAS shell. Called from `onDestroy`.
#[no_mangle]
pub extern "system" fn Java_{jni}_NativeBridge_detach<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {{
    backend_android::aas::detach();
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
    // `target/` for external consumers). Common deps (framework-core,
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
