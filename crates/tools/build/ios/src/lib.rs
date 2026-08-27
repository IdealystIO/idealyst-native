//! iOS build orchestration for `idealyst build ios`.
//!
//! The user's app crate is intentionally platform-agnostic — it just
//! exposes `pub fn app() -> Element`. iOS needs (a) a `staticlib`
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

pub mod capabilities;
pub mod source;
pub mod web_html;

pub use source::{
    remap_path_flags, FrameworkSource, GitDefaults, GitRef, require_workspace_root,
};
pub use web_html::{font_preload_tags, inject_into_head};

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
    /// as `--features <list>`. Used by `idealyst dev` to pass `dev`
    /// (→ `runtime-core/dev` + `runtime-shared/robot`) so the Robot
    /// bridge auto-starts; left empty for plain `idealyst build`.
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
    /// User-visible version string (→ `CFBundleShortVersionString`).
    /// Falls back to `"0.0.1"`.
    pub version: String,
    /// Build number (→ `CFBundleVersion`). App Store Connect requires
    /// this to be unique and monotonically increasing across uploads of
    /// the same `version`; the dev/sim paths don't care, so it defaults
    /// to `"1"`. Set in TOML as
    /// `[package.metadata.idealyst.app].build_number`, or override per
    /// upload via `idealyst publish ios --build-number`.
    pub build_number: String,
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
    /// Optional cargo bin name to run as the project's server.
    /// When set, `idealyst dev --web` builds the user's wasm bundle
    /// into `pkg/` and then `cargo run`s this bin with
    /// `--features server` instead of launching `dev-http`'s static
    /// server — the user's bin is expected to serve both `/_srv/*`
    /// and the static assets at `/` itself (the `server` SDK's
    /// `router()` composed with a `ServeDir`). Set in TOML as:
    /// ```toml
    /// [package.metadata.idealyst.app]
    /// server_bin = "server"
    /// ```
    /// Leave unset for client-only projects.
    ///
    /// For non-trivial full-stack apps the server is usually a
    /// *standalone* workspace (so enabling its `--features server`
    /// deps can't leak into the client build via Cargo
    /// feature-unification) — declare [`server_manifest`] instead, and
    /// optionally keep `server_bin` to name which bin in that manifest
    /// to run.
    ///
    /// [`server_manifest`]: AppMetadata::server_manifest
    pub server_bin: Option<String>,
    /// Optional path to a *standalone* server workspace's `Cargo.toml`,
    /// relative to this app crate's directory. When set, `idealyst run
    /// server` builds the web bundle into `dist/web` and then
    /// `cargo run --manifest-path <server_manifest>` (adding `--bin
    /// <server_bin>` when that's also set) — instead of the in-crate
    /// `cargo run -p <pkg> --features server` path that [`server_bin`]
    /// alone selects.
    ///
    /// This is the recommended shape for real full-stack apps: the
    /// server lives in its own `[workspace]` so its server-only deps
    /// (axum/sqlx/the `server` feature) never feature-unify into the
    /// client/wasm build. Set in TOML as:
    /// ```toml
    /// [package.metadata.idealyst.app]
    /// server_manifest = "../../server/Cargo.toml"
    /// server_bin       = "server"   # optional: names the bin to run
    /// ```
    ///
    /// [`server_bin`]: AppMetadata::server_bin
    pub server_manifest: Option<String>,
    /// Port the project's server binds in dev / `run server`, unless the
    /// CLI is given an explicit override (`idealyst dev --port`,
    /// `idealyst run server --port`), which wins. The CLI
    /// passes the resolved value through as the `PORT` env var when it
    /// spawns the server,
    /// and advertises `http://<host>:<port>` to every client it launches
    /// (web via a `window.IDEALYST_SERVER_URL` global, native via the
    /// `IDEALYST_SERVER_URL` env var) so the app's `server::configure`
    /// can point at the dev backend without hardcoding the port. Defaults
    /// to `3000`. Set in TOML as:
    /// ```toml
    /// [package.metadata.idealyst.app]
    /// server_port = 3000
    /// ```
    pub server_port: u16,
    /// In-crate worker binary — the `jobs` SDK's queue drainer, analogous to
    /// [`server_bin`]. `worker_bin = "worker"` runs `cargo run -p <pkg> --bin
    /// worker --features server`. `idealyst worker` runs it standalone, and
    /// `idealyst dev` auto-spawns it alongside the server when a shared queue
    /// backend is configured (see the `[jobs]` block in `dev.toml`).
    ///
    /// [`server_bin`]: AppMetadata::server_bin
    pub worker_bin: Option<String>,
    /// Standalone-workspace worker manifest, analogous to [`server_manifest`].
    /// When set, the worker runs via `cargo run --manifest-path <worker_manifest>`
    /// (adding `--bin <worker_bin>` when that's also set).
    ///
    /// [`server_manifest`]: AppMetadata::server_manifest
    pub worker_manifest: Option<String>,
    /// Web-target-specific knobs. Always present — empty defaults if
    /// the user didn't declare a `[package.metadata.idealyst.app.web]`
    /// block.
    pub web: WebMetadata,
    /// macOS-target-specific knobs. Always present — defaults if the
    /// user didn't declare a `[package.metadata.idealyst.app.macos]`
    /// block. Drives `idealyst publish macos` (App Store category,
    /// minimum-OS, copyright).
    pub macos: MacosMetadata,
    /// User-facing reason strings for capabilities, keyed by capability
    /// name, from `[package.metadata.idealyst.app.permissions]`:
    /// ```toml
    /// [package.metadata.idealyst.app.permissions]
    /// microphone = "Record voice notes"
    /// ```
    /// The *requirement* (which permission) comes from an SDK's
    /// `capabilities` declaration; this map supplies the *justification*
    /// the OS prompt shows. A capability with no entry here gets a
    /// generic default and a build-time warning. See
    /// [`capabilities`](crate::capabilities).
    pub permissions: std::collections::BTreeMap<String, String>,
}

/// macOS-target-specific config from `[package.metadata.idealyst.app.macos]`.
///
/// Used by `idealyst publish macos`. Distribution to the Mac App Store
/// requires a `category`; the other fields refine the bundle's Info.plist.
/// ```toml
/// [package.metadata.idealyst.app.macos]
/// category = "public.app-category.productivity"  # LSApplicationCategoryType
/// min_version = "12.0"                            # LSMinimumSystemVersion
/// copyright = "© 2026 Acme, Inc."                # NSHumanReadableCopyright
/// ```
#[derive(Debug, Clone)]
pub struct MacosMetadata {
    /// `LSApplicationCategoryType` (an `public.app-category.*` UTI). The
    /// Mac App Store **requires** it — `publish macos --app-store` errors
    /// when it's unset. Optional otherwise (dev/Developer-ID builds).
    pub category: Option<String>,
    /// `LSMinimumSystemVersion`. Defaults to `"11.0"` (Big Sur) — the
    /// floor `backend-macos` targets.
    pub min_version: String,
    /// `NSHumanReadableCopyright`, shown in the About panel. Optional.
    pub copyright: Option<String>,
}

impl Default for MacosMetadata {
    fn default() -> Self {
        Self {
            category: None,
            min_version: "11.0".to_string(),
            copyright: None,
        }
    }
}

/// Web-target-specific config from `[package.metadata.idealyst.app.web]`.
///
/// Lives under `app.web` (not at the top level) so this is the place
/// every future web-only knob lands — keeps the namespace tidy and the
/// non-web `AppMetadata` fields focused on cross-platform identity.
#[derive(Debug, Clone, Default)]
pub struct WebMetadata {
    /// Project-relative paths to font files that should ship as
    /// `<link rel="preload" as="font" crossorigin>` tags in the
    /// staged `index.html`. Declared in TOML as:
    /// ```toml
    /// [package.metadata.idealyst.app.web]
    /// preload_fonts = ["fonts/Inter-Regular.ttf", "fonts/Inter-Bold.ttf"]
    /// ```
    /// Why declarative rather than auto-discovered: the framework
    /// stays out of the "which fonts matter for first paint" question
    /// — only the project author knows which weights / styles are
    /// above-the-fold. Preloading every face the project ships costs
    /// bandwidth for files the page may never reference; preloading
    /// nothing leaves the runtime `@font-face` injection as the only
    /// signal to the browser and the font fetch only starts AFTER wasm
    /// boots. This list is the seam in between.
    ///
    /// Paths are resolved relative to the project root; the build /
    /// dev paths prefix them with `/` to form the URL. Leave empty to
    /// preload nothing (the default — keeps existing projects on
    /// today's behavior).
    pub preload_fonts: Vec<String>,

    /// Explicit allowlist of project-root entries (files or folders)
    /// that should be staged into the served web bundle. Declared as:
    /// ```toml
    /// [package.metadata.idealyst.app.web]
    /// assets = ["assets", "public", "fonts", "robots.txt"]
    /// ```
    /// When NON-EMPTY this is the *only* set that ships (plus the
    /// always-needed `index.html` and the build-emitted `pkg/` + icon
    /// files) — an explicit-is-safe model that guarantees internal docs
    /// (`FEEDBACK.md`, `design-files/`, `dev.toml`, …) can never leak
    /// into production no matter what lands in the project root.
    ///
    /// When EMPTY (the default) staging falls back to a tightened
    /// denylist (see `is_excluded_from_bundle`) that still auto-ships
    /// real web assets but excludes source, docs, configs, and VCS
    /// metadata. Leave empty to keep the auto-discover behavior; set it
    /// to lock the bundle down to a known surface.
    pub assets: Vec<String>,

    /// EXTERNAL directories (outside this app crate) to stage into the
    /// served bundle — each copied in under its own final path component.
    /// Declared as:
    /// ```toml
    /// [package.metadata.idealyst.app.web]
    /// font_dirs = ["../whiteboard/fonts"]
    /// ```
    /// The motivating case: a reusable component LIBRARY declares a
    /// typeface (its `face!` `include_bytes!`s the bytes for native), and
    /// a consuming app needs those same font FILES served on web (the web
    /// backend links `@font-face` to `/fonts/<name>`). The library owns
    /// the files (so it stays self-contained), and the app points here to
    /// stage them — no per-app copy, no symlink. Paths are resolved
    /// relative to the app crate and MAY contain `..` (unlike `assets`,
    /// which is the in-crate allowlist). Each dir is copied to
    /// `<bundle>/<dir-final-name>/` (so `../whiteboard/fonts` →
    /// `<bundle>/fonts/`, matching the `/fonts/...` `@font-face` URLs).
    pub font_dirs: Vec<String>,
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
    /// Native Linux app via `backend-linux` (GTK4) + `host-gtk`. Uses
    /// real GTK widgets in a `gtk::ApplicationWindow`.
    Linux,
    /// Native Windows app via `backend-windows` (Win32) + `host-win32`.
    /// Uses raw HWND child controls under a top-level window.
    Windows,
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
            "linux" => Ok(Target::Linux),
            "windows" => Ok(Target::Windows),
            other => anyhow::bail!(
                "unknown target {:?}; expected one of: web, ios, android, roku, macos, terminal, linux, windows",
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
            Target::Linux => "linux",
            Target::Windows => "windows",
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
    build_number: Option<String>,
    #[serde(default)]
    splash: Option<RawSplashConfig>,
    #[serde(default)]
    targets: Option<Vec<String>>,
    #[serde(default)]
    server_bin: Option<String>,
    #[serde(default)]
    server_manifest: Option<String>,
    #[serde(default)]
    server_port: Option<u16>,
    #[serde(default)]
    worker_bin: Option<String>,
    #[serde(default)]
    worker_manifest: Option<String>,
    #[serde(default)]
    web: Option<RawWebMetadata>,
    #[serde(default)]
    macos: Option<RawMacosMetadata>,
    #[serde(default)]
    permissions: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Default, Deserialize)]
struct RawMacosMetadata {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    min_version: Option<String>,
    #[serde(default)]
    copyright: Option<String>,
}

#[derive(Default, Deserialize)]
struct RawWebMetadata {
    #[serde(default)]
    preload_fonts: Option<Vec<String>>,
    #[serde(default)]
    assets: Option<Vec<String>>,
    #[serde(default)]
    font_dirs: Option<Vec<String>>,
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

    let idealyst_raw = parsed.package.metadata.idealyst.unwrap_or_default();
    let app_raw = idealyst_raw.app.unwrap_or_default();
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

    let raw_web = app_raw.web.unwrap_or_default();
    let web = WebMetadata {
        preload_fonts: raw_web.preload_fonts.unwrap_or_default(),
        assets: raw_web.assets.unwrap_or_default(),
        font_dirs: raw_web.font_dirs.unwrap_or_default(),
    };

    let raw_macos = app_raw.macos.unwrap_or_default();
    let macos = MacosMetadata {
        category: raw_macos.category,
        min_version: raw_macos
            .min_version
            .unwrap_or_else(|| MacosMetadata::default().min_version),
        copyright: raw_macos.copyright,
    };

    let app = AppMetadata {
        name: app_name,
        bundle_id,
        version: app_raw.version.unwrap_or_else(|| "0.0.1".to_string()),
        build_number: app_raw.build_number.unwrap_or_else(|| "1".to_string()),
        splash,
        targets,
        server_bin: app_raw.server_bin,
        server_manifest: app_raw.server_manifest,
        server_port: app_raw.server_port.unwrap_or(3000),
        worker_bin: app_raw.worker_bin,
        worker_manifest: app_raw.worker_manifest,
        web,
        macos,
        permissions: app_raw.permissions.unwrap_or_default(),
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
    // `runtime-core` + `runtime-shared` are DIRECT deps so the dev
    // build's `--features` spec resolves: cargo only accepts
    // `<dep>/<feat>` for a direct dependency of the package being built.
    // The facade carries the catalog anchor + emission gate
    // (`runtime-core/dev` = robot registry + catalog); runtime-shared
    // carries the substrate names this wrapper spells directly
    // (`set_initial_path`, `robot::bridge`).
    let runtime_core_dep = source.dep("crates/runtime/core", &[]);
    let shared_dep = source.dep("crates/runtime/shared", &[]);
    // `async-driver` so the iOS backend installs the cooperative main-thread
    // async executor in `install_scheduler` (forwards ios-mobile → ios-core →
    // apple-core). Without it, `spawn_async` falls back to `pollster::block_on`
    // on the main thread and a long-running future (`use_sse` / `use_socket`
    // recv loop) freezes the UI.
    //
    // `new-core` additionally compiles the backend's newcore module
    // (Host + caps impls + `run_in_view` boot + dispatch-site flush
    // driver) that `ios_main` below calls into. The feature is vacuous
    // once backend-ios-mobile makes its contents unconditional; drop it
    // from this list at that point.
    let bios_dep = source.dep("crates/backend/ios/mobile", &["async-driver"]);
    // Plain path dep on the user crate: the app's own defaults select
    // its prim families / feature set, and there is only one core.
    let user_dep = format!("{{ path = \"{}\" }}", project_dir.display());
    // Redirect the USER crate's git-pinned framework deps onto the same
    // local checkout this wrapper uses. The wrapper is its own
    // `[workspace]`, so a `[patch]` in the user's own workspace root is
    // INERT here — cargo only honours the patch table of the workspace
    // being built. Without this block an out-of-tree project whose
    // framework source resolved to `Workspace` (a local checkout, e.g. via
    // `IDEALYST_FRAMEWORK_PATH`) gets the wrapper on path deps and the app
    // crate on git deps: two `runtime_scene`/`runtime_core` instances and
    // the inscrutable "expected `Element`, found `Element`" error at the
    // wrapper→app boundary. Empty string in `Git` mode (both sides already
    // share one rev). The web/SSR wrappers emit this for the same reason.
    let patch_block = source.patch_block();

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
runtime-core = {runtime_core_dep}
runtime-shared = {shared_dep}
{user_name} = {user_dep}

[features]
# `idealyst dev` builds with `--features dev`: the catalog + automation
# surface (`runtime-core/dev` = robot registry + catalog emission
# gate) plus the bridge TRANSPORT (`runtime-shared/robot`), which is
# what makes `robot::bridge::set_app_identity` below resolve.
dev = ["runtime-core/dev", "runtime-shared/robot"]

[target.'cfg(target_os = "ios")'.dependencies]
backend-ios-mobile = {bios_dep}
objc2 = "0.5"
objc2-foundation = {{ version = "0.2", features = ["NSString"] }}
objc2-ui-kit = {{ version = "0.2", features = ["UIResponder", "UIView"] }}
{patch_block}"#,
        runtime_core_dep = runtime_core_dep,
        shared_dep = shared_dep,
        bios_dep = bios_dep,
        user_name = manifest.name,
        user_dep = user_dep,
        patch_block = patch_block,
    );

    let lib_rs = format!(
        r#"//! GENERATED by `idealyst build ios`. Mounts
//! `{lib}::app()` under a UIView provided by the Swift host through
//! `backend_ios::newcore::run_in_view` — per-app `World`, scene
//! registry (`register_builtins` + the app's
//! `register_scene_extensions` seam), dispatch-site flush driver.
//! Boilerplate is identical for every project — only the `app()` call
//! site changes.

#![cfg(target_os = "ios")]

use std::cell::RefCell;

thread_local! {{
    /// `run_in_view` returns a `NewCoreApp` (world + realized tree +
    /// flush-driver hooks) that must outlive the mounted UI. Stashed
    /// here so it survives `ios_main` returning; `ios_teardown` (and a
    /// re-entrant `ios_main`) `stop()` it.
    static APP: RefCell<Option<backend_ios::newcore::NewCoreApp>> = const {{ RefCell::new(None) }};
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

    // Idempotent re-entry: tear down any previous mount first.
    APP.with(|slot| {{
        if let Some(app) = slot.borrow_mut().take() {{
            app.stop();
        }}
    }});

    // Register the project's identity for the Robot bridge mDNS
    // advertisement (shared substrate — core-agnostic). No-op when the
    // `dev` feature is off (bridge isn't built).
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

    // `run_in_view` performs the idempotent installs (scheduler,
    // logger, global self-handle), opens
    // the mount-buffering window, runs `register_builtins` + the app's
    // `register_scene_extensions` seam on the scene registry, and
    // realizes `app()` inside `World::enter`.
    let app = unsafe {{
        backend_ios::newcore::run_in_view(
            root_view,
            {lib}::register_scene_extensions,
            || {lib}::app(),
        )
    }};
    APP.with(|slot| *slot.borrow_mut() = Some(app));
}}

/// Tear down the active mount. Safe to call from anywhere on the main
/// thread; idempotent — a no-op if nothing is mounted.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {{
    APP.with(|slot| {{
        if let Some(app) = slot.borrow_mut().take() {{
            app.stop();
        }}
    }});
}}

/// Cold-start deep-link hook. The Swift host calls this from
/// `application(_:didFinishLaunchingWithOptions:)` (custom-scheme /
/// universal-link launch) BEFORE `ios_main`, passing the URL's PATH
/// component (e.g. `/encounters/abc`). It seeds the shared substrate's
/// initial-path slot, which the vocabulary navigator handlers peek at
/// mount so the deep-linked screen resolves and the back stack is
/// reconstructed. When no launch URL is present the host never calls
/// this and behavior is unchanged.
///
/// # Safety
/// - Must be invoked on the main thread, before `ios_main`.
/// - `path` must be a non-null, valid, NUL-terminated C string, or null
///   (treated as "no deep link").
#[no_mangle]
pub unsafe extern "C" fn ios_set_launch_path(path: *const std::os::raw::c_char) {{
    if path.is_null() {{
        return;
    }}
    match unsafe {{ std::ffi::CStr::from_ptr(path) }}.to_str() {{
        Ok(s) if !s.is_empty() => runtime_shared::set_initial_path(Some(s.to_string())),
        _ => {{}}
    }}
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

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regression for `build-ios`.
    //!
    //! `idealyst dev` builds with a `--features` spec naming the
    //! framework's dev surface, and cargo only resolves `<dep>/<feat>`
    //! for a DIRECT dependency of the package being built. So the
    //! wrapper must declare `runtime-core` + `runtime-shared`
    //! directly and map them through its own `dev` feature. Otherwise
    //! cargo errors "unknown feature for unknown package" the moment
    //! the launcher fires its build, and the MCP catalog never sees the
    //! components linked into the resulting staticlib.

    use super::*;
    use crate::source::FrameworkSource;

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
                worker_bin: None,
                worker_manifest: None,
                web: WebMetadata::default(),
                macos: Default::default(),
                permissions: Default::default(),
            },
        }
    }

    fn run_generator() -> (std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let manifest = fake_manifest();
        let source = FrameworkSource::Workspace { root: workspace_root };
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    /// `ios_main` boots through `backend_ios::newcore::run_in_view`,
    /// enables `backend-ios-mobile/new-core`, and takes a plain path dep
    /// on the user crate (no core pin — there is one core).
    #[test]
    fn wrapper_boots_run_in_view_with_plain_user_dep() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let lib_rs = std::fs::read_to_string(wrapper_dir.join("src/lib.rs")).unwrap();
        assert!(
            !cargo.contains("old-core"),
            "wrapper must not pin any core feature on the user crate:\n{cargo}",
        );
        assert!(
            !cargo.contains("default-features = false"),
            "user-crate dep must be a plain path dep so the app's own \
             defaults apply:\n{cargo}",
        );
        assert!(
            cargo.contains("backend-ios-mobile"),
            "wrapper must depend on backend-ios-mobile:\n{cargo}",
        );
        assert!(
            lib_rs.contains("backend_ios::newcore::run_in_view"),
            "ios_main must boot through newcore::run_in_view:\n{lib_rs}",
        );
        assert!(
            lib_rs.contains("register_scene_extensions"),
            "ios_main must register through the scene seam:\n{lib_rs}",
        );
    }

    #[test]
    fn wrapper_has_dev_surface_deps_so_launcher_can_pass_dev_feature() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        assert!(
            parsed
                .get("dependencies")
                .and_then(|d| d.get("runtime-core"))
                .is_some()
                && parsed
                    .get("dependencies")
                    .and_then(|d| d.get("runtime-shared"))
                    .is_some(),
            "iOS wrapper missing the `runtime-core` / `runtime-shared` \
             direct deps — the launcher's dev `--features` spec will fail \
             at cargo time and the MCP catalog will be empty. Got:\n{cargo}",
        );
        let dev: Vec<&str> = parsed
            .get("features")
            .and_then(|f| f.get("dev"))
            .and_then(|d| d.as_array())
            .expect("iOS wrapper must declare a `dev` feature")
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
    fn wrapper_path_deps_user_crate() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        let user_dep = parsed
            .get("dependencies")
            .and_then(|d| d.get("demo"))
            .expect("wrapper depends on user crate `demo`");
        assert!(
            user_dep.get("path").is_some(),
            "iOS wrapper's user-crate dep should be a path dep so the local \
             code is what links into the staticlib; got {:?}",
            user_dep,
        );
    }

    /// Regression: an out-of-tree project whose framework source resolved to
    /// a local `Workspace` checkout failed to build for iOS with "expected
    /// `Element`, found `Element`". The wrapper is its own `[workspace]`, so
    /// the user's own `[patch]` table is inert inside it — the wrapper linked
    /// framework crates by PATH while the app crate kept resolving its
    /// git-pinned deps, producing two `runtime_scene` / `runtime_core`
    /// instances. The iOS wrapper generator simply never emitted the
    /// `[patch."<git-url>"]` block that `FrameworkSource::patch_block`
    /// already knew how to render (the catalog + export wrappers did).
    #[test]
    fn regression_ios_wrapper_patches_git_deps_onto_workspace_checkout() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml"))
            .expect("read Cargo.toml");
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        let patch = parsed
            .get("patch")
            .and_then(|p| p.get("https://github.com/IdealystIO/idealyst-native"))
            .unwrap_or_else(|| {
                panic!(
                    "iOS wrapper must emit a `[patch.\"<git-url>\"]` block in \
                     Workspace mode, or the user crate's git-pinned framework \
                     deps stay on git while the wrapper uses path deps — two \
                     instances of every runtime crate. Got:\n{cargo}"
                )
            });
        // The redirect has to cover the crates that actually cross the
        // wrapper→app boundary; `runtime-scene` is the one that surfaced the
        // duplicate as "expected `Element`, found `Element`".
        for krate in ["runtime-core", "runtime-scene", "runtime-shared"] {
            let entry = patch
                .get(krate)
                .unwrap_or_else(|| panic!("patch block missing `{krate}`:\n{cargo}"));
            assert!(
                entry.get("path").is_some(),
                "`{krate}` must be redirected to a path dep; got {entry:?}",
            );
        }
    }

    /// The mirror of the above: in `Git` mode the wrapper and the app crate
    /// already resolve the same rev, so emitting a patch block would point
    /// framework crates at paths that don't exist on this machine.
    #[test]
    fn git_mode_wrapper_emits_no_patch_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        std::fs::create_dir_all(&project_dir).unwrap();
        let source = FrameworkSource::Git {
            url: "https://github.com/IdealystIO/idealyst-native".into(),
            refspec: crate::source::GitRef::Tag("1.3.6".into()),
        };
        generate_wrapper(&wrapper_dir, &project_dir, &source, &fake_manifest())
            .expect("generate wrapper");
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        assert!(
            parsed.get("patch").is_none(),
            "git-mode wrapper must not emit a patch block:\n{cargo}",
        );
    }

    /// `build_number` (→ `CFBundleVersion`) parses from the manifest when
    /// present and defaults to `"1"` otherwise. App Store Connect rejects a
    /// re-used build number, so the field has to round-trip from TOML.
    #[test]
    fn build_number_parses_and_defaults() {
        fn parse_with(extra: &str) -> Manifest {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cargo = format!(
                "[package]\nname = \"demo\"\nversion = \"0.0.1\"\n\
                 [package.metadata.idealyst.app]\nbundle_id = \"ai.example.demo\"\n{extra}",
            );
            std::fs::write(tmp.path().join("Cargo.toml"), cargo).unwrap();
            parse_manifest(tmp.path()).expect("parse manifest")
        }

        assert_eq!(
            parse_with("").app.build_number,
            "1",
            "build_number should default to \"1\" when unset",
        );
        assert_eq!(
            parse_with("build_number = \"42\"\n").app.build_number,
            "42",
            "build_number should round-trip from the manifest",
        );
    }

    /// `[package.metadata.idealyst.app.macos]` parses into `MacosMetadata`
    /// (drives `idealyst publish macos`), with `min_version` defaulting to
    /// `"11.0"` and `category`/`copyright` optional.
    #[test]
    fn macos_metadata_parses_and_defaults() {
        fn parse_with(extra: &str) -> Manifest {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cargo = format!(
                "[package]\nname = \"demo\"\nversion = \"0.0.1\"\n\
                 [package.metadata.idealyst.app]\nbundle_id = \"ai.example.demo\"\n{extra}",
            );
            std::fs::write(tmp.path().join("Cargo.toml"), cargo).unwrap();
            parse_manifest(tmp.path()).expect("parse manifest")
        }

        let bare = parse_with("");
        assert_eq!(bare.app.macos.min_version, "11.0", "min_version defaults to 11.0");
        assert!(bare.app.macos.category.is_none());
        assert!(bare.app.macos.copyright.is_none());

        let full = parse_with(
            "[package.metadata.idealyst.app.macos]\n\
             category = \"public.app-category.productivity\"\n\
             min_version = \"13.0\"\n\
             copyright = \"© 2026 Acme\"\n",
        );
        assert_eq!(
            full.app.macos.category.as_deref(),
            Some("public.app-category.productivity"),
        );
        assert_eq!(full.app.macos.min_version, "13.0");
        assert_eq!(full.app.macos.copyright.as_deref(), Some("© 2026 Acme"));
    }

    /// The `jobs` SDK's worker declaration parses into `worker_bin` /
    /// `worker_manifest` (drives `idealyst worker` + `dev`'s auto-spawn), and
    /// defaults to `None` when absent.
    #[test]
    fn worker_fields_parse_and_default() {
        fn parse_with(extra: &str) -> Manifest {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cargo = format!(
                "[package]\nname = \"demo\"\nversion = \"0.0.1\"\n\
                 [package.metadata.idealyst.app]\nbundle_id = \"ai.example.demo\"\n{extra}",
            );
            std::fs::write(tmp.path().join("Cargo.toml"), cargo).unwrap();
            parse_manifest(tmp.path()).expect("parse manifest")
        }

        let bare = parse_with("");
        assert!(bare.app.worker_bin.is_none());
        assert!(bare.app.worker_manifest.is_none());

        let with_worker = parse_with(
            "worker_bin = \"worker\"\nworker_manifest = \"../../worker/Cargo.toml\"\n",
        );
        assert_eq!(with_worker.app.worker_bin.as_deref(), Some("worker"));
        assert_eq!(
            with_worker.app.worker_manifest.as_deref(),
            Some("../../worker/Cargo.toml"),
        );
    }
}
