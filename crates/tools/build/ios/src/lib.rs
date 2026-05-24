//! iOS build orchestration for `idealyst build ios`.
//!
//! The user's app crate is intentionally platform-agnostic — it just
//! exposes `pub fn app() -> Primitive`. iOS needs (a) a `staticlib`
//! crate-type producing a `.a`, (b) a C-callable `ios_main` entry
//! point, and (c) the chain of iOS deps (`backend-ios-mobile`, `objc2*`).
//! Putting all of that in the user's crate would defeat the
//! platform-agnostic principle, so instead the CLI **generates** a
//! tiny staticlib wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/ios/wrapper/
//! ```
//!
//! The wrapper depends on the user's crate (path dep) and on the
//! framework's iOS bits, and its `lib.rs` is the iOS entry-point
//! boilerplate — identical for every project, modulo the
//! `<project>::app()` call site.
//!
//! Regenerated on every build (the wrapper is just a build artifact;
//! `idealyst scaffold ios` will eventually materialize it into the
//! repo if you want to take ownership).
//!
//! ## Why the manifest + source helpers are public
//!
//! Sibling crates — `run-ios`, `build-android`, `build-runtime-server`,
//! `build-roku` — reuse [`parse_manifest`] and the
//! [`source::FrameworkSource`] resolver so they don't re-parse the
//! same Cargo.toml twice or reimplement workspace-vs-git discovery.
//! The shared pieces live here because this crate already owns the
//! wrapper-generation contract that depends on them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

pub mod source;

pub use source::{FrameworkSource, GitDefaults, GitRef, require_workspace_root};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build in release mode (`--release`). Default: debug.
    pub release: bool,
    /// Target a physical device (`aarch64-apple-ios`) instead of the
    /// host-arch simulator (default).
    pub device: bool,
    /// Where the wrapper Cargo.toml should source framework crates
    /// from. The CLI constructs this with `FrameworkSource::detect`
    /// before invoking `build()`.
    pub source: FrameworkSource,
    /// Cargo features to enable on the cargo invocation. Forwarded
    /// as `--features <list>`. Used by `idealyst dev` to pass
    /// `runtime-core/dev` so the Robot bridge auto-starts; left
    /// empty for plain `idealyst build`.
    pub user_features: Vec<String>,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced `lib<project>_ios_wrapper.a`.
    pub staticlib: PathBuf,
    /// The rustc target triple the staticlib was built for.
    pub target_triple: &'static str,
    /// Path to the generated wrapper crate. Useful for debugging and
    /// for the eventual `scaffold ios` command to copy from.
    pub wrapper_dir: PathBuf,
}

/// Parsed view of the user project's `Cargo.toml`, including the
/// `[package.metadata.idealyst]` block. All call sites in this crate
/// and in `run-ios` route through this struct so the schema lives in
/// one place.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Cargo package name (e.g. `docs`). Used to name the wrapper
    /// crate and as the path-dep key in the wrapper's Cargo.toml.
    pub name: String,
    /// Cargo lib name (defaults to package name with `-` → `_`).
    /// Used to compute the produced staticlib filename, and as the
    /// Rust identifier the wrapper imports `app()` from.
    pub lib_name: String,
    /// Idealyst app config from `[package.metadata.idealyst.app]`.
    pub app: AppMetadata,
}

#[derive(Debug, Clone)]
pub struct AppMetadata {
    /// Human-facing app name (e.g. `"Idealyst Docs"`). May contain
    /// spaces. Falls back to title-cased `package.name`.
    pub name: String,
    /// Reverse-DNS bundle identifier (e.g.
    /// `"ai.truday.idealyst.docs"`). Required by every platform
    /// except Roku (which has no equivalent), so we keep it as
    /// `Option<String>` and let each platform's build/run path
    /// validate at point of use via [`AppMetadata::require_bundle_id`].
    /// This way a Roku-only project with no `bundle_id` still
    /// flows through `idealyst build --roku` without a misleading
    /// "iOS error" surfacing at CLI parse time.
    pub bundle_id: Option<String>,
    /// User-visible version string. Falls back to `"0.0.1"`.
    pub version: String,
    /// Splash-screen settings. Always present — if the user didn't
    /// declare `[package.metadata.idealyst.app.splash]`, defaults are
    /// filled in so every project gets a working splash without
    /// boilerplate. Set `duration_ms = 0` in TOML to skip the splash.
    pub splash: SplashConfig,
    /// Platforms this project ships on. Drives the default behavior
    /// of `idealyst dev` and `idealyst build` when no platform flag
    /// is passed: every target listed here is launched / built.
    /// Empty when the user didn't declare any; the CLI errors out
    /// in that case unless an explicit platform flag was given.
    pub targets: Vec<Target>,
}

impl AppMetadata {
    /// Borrow `bundle_id` or surface a helpful error pointing at
    /// the missing field. Called by every platform that needs the
    /// bundle id — iOS, Android, runtime-server, the dev-mode bonjour service
    /// name — so the diagnostic lands at the right time (when that
    /// platform was actually selected) instead of upfront in the
    /// shared CLI parser.
    pub fn require_bundle_id(&self) -> anyhow::Result<&str> {
        self.bundle_id
            .as_deref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing `[package.metadata.idealyst.app].bundle_id` — \
                     this platform needs a reverse-DNS bundle identifier \
                     (e.g. \"com.example.myapp\"). Roku builds don't need it; \
                     iOS / Android / runtime-server / dev do."
                )
            })
    }
}

/// Supported platform targets. Used both as the parsed form of the
/// `targets` field in `[package.metadata.idealyst.app]` and as the
/// switch the CLI's `dev` / `build` commands use to pick a
/// platform-specific code path. Variants are added here as backends
/// land — `Roku` is on the list because the framework already has
/// a `backend-roku` crate, even if the dev-loop story isn't wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Web,
    Ios,
    Android,
    Roku,
    /// Native macOS app via `backend-macos` + `host-appkit`. Builds a
    /// real `.app` bundle (eventually) — for now produces a binary
    /// the user can launch directly.
    Macos,
    /// TTY app via `backend-terminal` + `host-terminal`. Foreground
    /// crossterm grid in the current shell.
    Terminal,
}

impl Target {
    /// Parse one of `web | ios | android | roku | macos | terminal`
    /// (case-insensitive) from the `targets = [...]` array. Anything
    /// else is an error rather than a silent skip — typos in the
    /// manifest should be noisy.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "web" => Ok(Target::Web),
            "ios" => Ok(Target::Ios),
            "android" => Ok(Target::Android),
            "roku" => Ok(Target::Roku),
            "macos" => Ok(Target::Macos),
            "terminal" => Ok(Target::Terminal),
            other => anyhow::bail!(
                "unknown target {:?}; expected one of: web, ios, android, roku, macos, terminal",
                other
            ),
        }
    }

    /// Stable string form, used by the CLI when echoing what it's
    /// launching ("[dev] launching web…").
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Web => "web",
            Target::Ios => "ios",
            Target::Android => "android",
            Target::Roku => "roku",
            Target::Macos => "macos",
            Target::Terminal => "terminal",
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Splash-screen rendering config. Eventually this will be derived
/// from a Rust-authored `#[idealyst::splash]` AST (richer layout,
/// theme-token references, cross-platform). For now it's a tiny
/// TOML schema with reasonable defaults — enough to see the splash
/// pipeline working end-to-end.
#[derive(Debug, Clone)]
pub struct SplashConfig {
    /// Background color hex like `"#1a1a2e"`. Used to fill the
    /// initial screen before the framework mounts.
    pub background: String,
    /// Text shown centered on the splash. Defaults to `app.name`.
    pub title: String,
    /// Title text color hex. Defaults to `"#ffffff"`.
    pub title_color: String,
    /// How long the splash stays up after process launch, before the
    /// framework root mounts. `0` disables the splash entirely (mount
    /// happens immediately, no fade, no delay).
    pub duration_ms: u32,
}

impl SplashConfig {
    fn default_for(app_name: &str) -> Self {
        Self {
            background: "#1a1a2e".to_string(),
            title: app_name.to_string(),
            title_color: "#ffffff".to_string(),
            duration_ms: 1500,
        }
    }
}

/// Build the user's project at `project_dir` for iOS. Returns the
/// produced `.a` and metadata about how it was built.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("ios/wrapper");
    generate_wrapper(&wrapper_dir, &project_dir, &opts.source, &manifest)?;

    let target = pick_target(opts.device);
    cargo_build(&wrapper_dir, target, opts.release, &opts.user_features)?;

    let profile = if opts.release { "release" } else { "debug" };
    let staticlib_name = format!("lib{}_ios_wrapper.a", manifest.lib_name);
    let staticlib = wrapper_dir
        .join("target")
        .join(target)
        .join(profile)
        .join(staticlib_name);

    if !staticlib.is_file() {
        anyhow::bail!(
            "cargo build reported success but staticlib was not produced at {}",
            staticlib.display(),
        );
    }

    Ok(BuildArtifact {
        staticlib,
        target_triple: target,
        wrapper_dir,
    })
}

/// Pick the rustc target triple for an iOS build. `device = true`
/// always targets physical devices; otherwise we pick the matching
/// simulator target for the host arch (arm64 sim on Apple Silicon,
/// x86_64 sim on Intel).
pub fn pick_target(device: bool) -> &'static str {
    if device {
        "aarch64-apple-ios"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-apple-ios-sim"
    } else {
        "x86_64-apple-ios"
    }
}

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default)]
    lib: Option<RawLib>,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    #[serde(default)]
    metadata: RawMetadata,
}

#[derive(Default, Deserialize)]
struct RawMetadata {
    #[serde(default)]
    idealyst: Option<RawIdealystMetadata>,
}

#[derive(Default, Deserialize)]
struct RawIdealystMetadata {
    #[serde(default)]
    app: Option<RawAppMetadata>,
}

#[derive(Default, Deserialize)]
struct RawAppMetadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    splash: Option<RawSplashConfig>,
    #[serde(default)]
    targets: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
struct RawSplashConfig {
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    title_color: Option<String>,
    #[serde(default)]
    duration_ms: Option<u32>,
}

#[derive(Deserialize)]
struct RawLib {
    name: Option<String>,
}

/// Read `<project_dir>/Cargo.toml` and pull out the bits we care
/// about. Public so sibling crates can reuse the same parse instead
/// of re-doing it.
pub fn parse_manifest(project_dir: &Path) -> Result<Manifest> {
    let path = project_dir.join("Cargo.toml");
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    // Distinguish "you pointed me at a workspace root" from a real
    // malformed manifest. Pre-fix the user saw a cryptic
    // `missing field \`package\`` from serde — the more common cause
    // (running `idealyst dev` from the repo root without naming a
    // project) deserves a hint.
    if let Ok(raw_value) = toml::from_str::<toml::Value>(&raw) {
        let has_workspace = raw_value.get("workspace").is_some();
        let has_package = raw_value.get("package").is_some();
        if has_workspace && !has_package {
            anyhow::bail!(
                "{} is a workspace root, not an idealyst project. Pass a project \
                 directory (e.g. `idealyst dev --terminal examples/welcome`), or \
                 `cd` into one before invoking the CLI",
                path.display(),
            );
        }
    }
    let parsed: RawManifest = toml::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    let name = parsed.package.name.clone();
    let lib_name = parsed
        .lib
        .as_ref()
        .and_then(|l| l.name.clone())
        .unwrap_or_else(|| name.replace('-', "_"));

    let app_raw = parsed
        .package
        .metadata
        .idealyst
        .and_then(|i| i.app)
        .unwrap_or_default();
    // bundle_id is read but not validated here — platforms that
    // need it (iOS, Android, runtime-server, dev) call
    // `manifest.app.require_bundle_id()` so the error is platform-
    // specific and only fires when that platform is selected. Roku
    // builds don't need it at all.
    let bundle_id = app_raw.bundle_id.clone();
    let app_name = app_raw.name.unwrap_or_else(|| title_case(&name));
    let splash = match app_raw.splash {
        Some(s) => SplashConfig {
            background: s.background.unwrap_or_else(|| "#1a1a2e".to_string()),
            title: s.title.unwrap_or_else(|| app_name.clone()),
            title_color: s.title_color.unwrap_or_else(|| "#ffffff".to_string()),
            duration_ms: s.duration_ms.unwrap_or(1500),
        },
        None => SplashConfig::default_for(&app_name),
    };
    // Parse target strings into the typed enum. Empty when the
    // user didn't declare any — the CLI flags the missing
    // declaration when the user runs `idealyst dev` / `build`
    // without an explicit platform.
    let targets = match app_raw.targets {
        Some(list) => list
            .iter()
            .map(|s| Target::from_str(s))
            .collect::<Result<Vec<_>>>()
            .with_context(|| {
                format!(
                    "{}: invalid value in `[package.metadata.idealyst.app].targets`",
                    path.display(),
                )
            })?,
        None => Vec::new(),
    };

    let app = AppMetadata {
        name: app_name,
        bundle_id,
        version: app_raw.version.unwrap_or_else(|| "0.0.1".to_string()),
        splash,
        targets,
    };

    Ok(Manifest {
        name,
        lib_name,
        app,
    })
}

// `find_workspace_root` was the legacy lax probe (`[workspace]` only).
// It's been superseded by `source::FrameworkSource::detect` (returns
// `Workspace` or falls back to `Git`) and `source::require_workspace_root`
// (the strict variant for runtime-server / dev-server, which genuinely need the
// in-tree checkout). Both live in [`source`].

fn title_case(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Wrapper generation
// ---------------------------------------------------------------------------

/// Write the ephemeral wrapper crate to `wrapper_dir`. Idempotent —
/// overwrites whatever was there. Public so sibling crates can drive
/// the same wrapper without going through the full `build()`.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-ios-wrapper", manifest.name);
    let fcore_dep = source.dep("crates/framework/core", &[]);
    let bios_dep = source.dep("crates/backend/ios/mobile", &[]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build ios`. Do not edit — rewritten
# every build. Run `idealyst scaffold ios` to materialize an editable
# copy of this wrapper into your repo (once that command lands).

# Empty `[workspace]` declares this wrapper as a standalone project
# even though it physically lives under the main workspace's
# `target/idealyst/...`. Without it, cargo refuses to build because
# the parent Cargo.toml has `[workspace]` and would normally claim
# this directory as a member.
[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["staticlib"]

[dependencies]
runtime-core = {fcore_dep}
{user_name} = {{ path = "{user_path}" }}

[target.'cfg(target_os = "ios")'.dependencies]
backend-ios-mobile = {bios_dep}
objc2 = "0.5"
objc2-foundation = {{ version = "0.2", features = ["NSString"] }}
objc2-ui-kit = {{ version = "0.2", features = ["UIResponder", "UIView"] }}
"#,
        fcore_dep = fcore_dep,
        bios_dep = bios_dep,
        user_name = manifest.name,
        user_path = project_dir.display(),
    );

    let lib_rs = format!(
        r#"//! GENERATED by `idealyst build ios`. Mounts `{lib}::app()` under a
//! UIView provided by the Swift host. Boilerplate is identical for
//! every project — only the `app()` call site changes.

#![cfg(target_os = "ios")]

// Cargo package `backend-ios-mobile` ships under `[lib].name =
// "backend_ios"` to preserve the historical `libbackend_ios.a`
// filename Xcode's link step expects.
use backend_ios::IosBackend;
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {{
    /// `render` returns an `Owner` that must outlive the mounted UI.
    /// Stashed here so it survives `ios_main` returning.
    static OWNER: RefCell<Option<runtime_core::Owner>> = const {{ RefCell::new(None) }};
}}

/// C-exported entry point called by the Swift host from `viewDidLoad`.
///
/// # Safety
/// - Must be invoked on the main thread.
/// - `root_view` must be a non-null, valid `UIView *`.
#[no_mangle]
pub unsafe extern "C" fn ios_main(root_view: *mut std::ffi::c_void) {{
    std::panic::set_hook(Box::new(|info| {{
        eprintln!("RUST PANIC: {{}}", info);
    }}));

    let mtm = unsafe {{ MainThreadMarker::new_unchecked() }};
    let view: Retained<UIView> = unsafe {{
        Retained::retain(root_view as *mut UIView)
            .expect("ios_main: root_view must be non-null")
    }};

    OWNER.with(|slot| slot.borrow_mut().take());

    // Register the project's identity for the Robot bridge mDNS
    // advertisement. Done before `mount()` so the bridge thread
    // started inside the framework's walker sees the populated
    // identity. No-op when `dev` feature is off (bridge isn't built).
    #[cfg(feature = "dev")]
    {{
        ::runtime_core::robot::bridge::set_app_identity(
            ::runtime_core::robot::bridge::AppIdentity {{
                name: "{app_name}".to_string(),
                bundle_id: Some("{bundle_id}".to_string()),
                project_root: ::std::option::Option::None,
            }},
        );
    }}

    let mut backend = IosBackend::new(mtm);
    backend.set_host_root(view);
    let backend = Rc::new(RefCell::new(backend));
    // Lets navigator dispatch closures re-run layout after pushes/replaces.
    backend_ios::install_global_self(Rc::downgrade(&backend));
    // NSTimer-backed scheduler so `after_ms` / `schedule_microtask`
    // delay correctly. Without it `after_ms` fires its callback
    // synchronously at call time, which breaks long-press
    // recognizers and any other timer-driven feature.
    backend_ios::install_scheduler();

    // `mount` runs the user's `app()` inside the root reactive
    // scope, so reactive primitives declared at the top of `app()`
    // (signals, effects, refs) are adopted by the returned Owner.
    // `render(backend, {lib}::app())` would have constructed the
    // tree first (outside any scope) — fine for trees with no
    // top-level reactive declarations, but silently drops `effect!`
    // cleanups for ones that do.
    let owner = runtime_core::mount(backend, {lib}::app);
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}}

/// Tear down the active mount. Safe to call from anywhere on the main
/// thread; idempotent — a no-op if nothing is mounted.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {{
    OWNER.with(|slot| slot.borrow_mut().take());
}}
"#,
        lib = manifest.lib_name,
        app_name = manifest.name,
        bundle_id = manifest
            .app
            .bundle_id
            .clone()
            .unwrap_or_else(|| format!("com.example.{}", manifest.name)),
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
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
        "[build-ios] cargo build --target {target}{}{} (in {})",
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
        .with_context(|| "failed to spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("cargo build exited with {status}");
    }
    Ok(())
}
