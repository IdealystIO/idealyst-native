//! iOS build orchestration for `idealyst build ios`.
//!
//! The user's app crate is intentionally platform-agnostic — it just
//! exposes `pub fn app() -> Primitive`. iOS needs (a) a `staticlib`
//! crate-type producing a `.a`, (b) a C-callable `ios_main` entry
//! point, and (c) the chain of iOS deps (`backend-ios`, `objc2*`).
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
//! ## Why the manifest + workspace helpers are public
//!
//! Sibling crates — currently [`run-ios`](https://crates.io/crates/run-ios) —
//! reuse [`parse_manifest`] and [`find_workspace_root`] so they don't
//! re-parse the same Cargo.toml twice or reimplement workspace-root
//! discovery. The shared pieces live here because this crate already
//! owns the wrapper-generation contract that depends on them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build in release mode (`--release`). Default: debug.
    pub release: bool,
    /// Target a physical device (`aarch64-apple-ios`) instead of the
    /// host-arch simulator (default).
    pub device: bool,
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
    /// `"ai.truday.idealyst.docs"`). Required; we error out if it's
    /// missing because shipping anywhere needs one.
    pub bundle_id: String,
    /// User-visible version string. Falls back to `"0.0.1"`.
    pub version: String,
}

/// Build the user's project at `project_dir` for iOS. Returns the
/// produced `.a` and metadata about how it was built.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;
    let workspace_root = find_workspace_root(&project_dir)?;

    let wrapper_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("ios/wrapper");
    generate_wrapper(&wrapper_dir, &project_dir, &workspace_root, &manifest)?;

    let target = pick_target(opts.device);
    cargo_build(&wrapper_dir, target, opts.release)?;

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
    let bundle_id = app_raw
        .bundle_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!(
            "{}: missing `[package.metadata.idealyst.app].bundle_id` — \
             iOS builds need a reverse-DNS bundle identifier",
            path.display(),
        ))?;
    let app = AppMetadata {
        name: app_raw.name.unwrap_or_else(|| title_case(&name)),
        bundle_id,
        version: app_raw.version.unwrap_or_else(|| "0.0.1".to_string()),
    };

    Ok(Manifest {
        name,
        lib_name,
        app,
    })
}

/// Walk up from `start` looking for a Cargo.toml that contains
/// `[workspace]`. Public so sibling crates can locate the workspace
/// root the same way for relative-path math.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf> {
    for ancestor in start.ancestors() {
        let cargo = ancestor.join("Cargo.toml");
        if cargo.is_file() {
            let content = fs::read_to_string(&cargo).unwrap_or_default();
            if content.contains("[workspace]") {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    anyhow::bail!(
        "could not find workspace root walking up from {} — \
         the wrapper crate references framework crates by workspace-relative path",
        start.display(),
    )
}

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
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-ios-wrapper", manifest.name);
    let fcore = workspace_root.join("crates/framework/core");
    let bios = workspace_root.join("crates/backend/ios");

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
framework-core = {{ path = "{fcore}" }}
{user_name} = {{ path = "{user_path}" }}

[target.'cfg(target_os = "ios")'.dependencies]
backend-ios = {{ path = "{bios}" }}
objc2 = "0.5"
objc2-foundation = {{ version = "0.2", features = ["NSString"] }}
objc2-ui-kit = {{ version = "0.2", features = ["UIResponder", "UIView"] }}
"#,
        fcore = fcore.display(),
        bios = bios.display(),
        user_name = manifest.name,
        user_path = project_dir.display(),
    );

    let lib_rs = format!(
        r#"//! GENERATED by `idealyst build ios`. Mounts `{lib}::app()` under a
//! UIView provided by the Swift host. Boilerplate is identical for
//! every project — only the `app()` call site changes.

#![cfg(target_os = "ios")]

use backend_ios::IosBackend;
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {{
    /// `render` returns an `Owner` that must outlive the mounted UI.
    /// Stashed here so it survives `ios_main` returning.
    static OWNER: RefCell<Option<framework_core::Owner>> = const {{ RefCell::new(None) }};
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

    let mut backend = IosBackend::new(mtm);
    backend.set_host_root(view);
    let backend = Rc::new(RefCell::new(backend));
    // Lets navigator dispatch closures re-run layout after pushes/replaces.
    backend_ios::install_global_self(Rc::downgrade(&backend));

    let owner = framework_core::render(backend, {lib}::app());
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
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo invocation
// ---------------------------------------------------------------------------

fn cargo_build(wrapper_dir: &Path, target: &str, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--target", target]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }

    eprintln!(
        "[build-ios] cargo build --target {target}{} (in {})",
        if release { " --release" } else { "" },
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
