//! Web build orchestration for `idealyst build web` and the dev
//! server.
//!
//! Mirror of `crates/build/ios/` and `crates/build/android/`: the
//! user's app crate is intentionally platform-agnostic — it exposes
//! `pub fn app() -> Primitive` and nothing else. The web target has
//! historically required the user to also write a `web.rs` with a
//! `#[wasm_bindgen(start)]` function plus a `[lib] crate-type =
//! ["cdylib", "rlib"]` and a handful of wasm-only deps
//! (`wasm-bindgen`, `console_error_panic_hook`, `lol_alloc`). That's
//! the same per-platform plumbing the iOS / Android wrapper crates
//! exist to absorb, and now web absorbs it the same way.
//!
//! `build()` generates an ephemeral `cdylib` wrapper at:
//!
//! ```text
//! <workspace>/target/idealyst/<project>/web/wrapper/
//! ```
//!
//! whose `src/lib.rs` is the wasm-bindgen entry point boilerplate
//! identical for every project — only the `<project>::app()` call
//! site changes. wasm-pack runs against the wrapper, producing the
//! `pkg/` bundle. We then copy that `pkg/` over to the user
//! project's root so the user's `index.html` (which references
//! `./pkg/<lib>.js`) keeps working without changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Build in release mode (`wasm-pack build --release`). Default:
    /// debug (`--dev`), which skips wasm-opt and keeps debug info.
    pub release: bool,
    /// Where the wrapper Cargo.toml should source framework crates
    /// from. The CLI constructs this with `FrameworkSource::detect`
    /// before invoking `build()`.
    pub source: FrameworkSource,
    /// Cargo features to enable on the **user crate** (e.g.
    /// `["dev-hot-reload"]` for AAS-mode hot reload). The wrapper's
    /// Cargo.toml grows a parallel `[features]` block that forwards
    /// each named feature to the user-crate dep, and wasm-pack runs
    /// with `-- --features <list>` so those features are active.
    /// Empty means "default features" — the common case.
    pub user_features: Vec<String>,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the generated `pkg/` directory inside the user project
    /// (NOT inside the wrapper). The dev server / static serve points
    /// here.
    pub pkg_dir: PathBuf,
    /// Path to the generated wrapper crate. Useful for debugging and
    /// for a future `idealyst scaffold web` command.
    pub wrapper_dir: PathBuf,
}

/// Build the user's project at `project_dir` for the web target.
/// Generates the wrapper, runs `wasm-pack build --target web`, and
/// copies the resulting `pkg/` over to `project_dir/pkg/`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    let wrapper_dir = opts
        .source
        .wrapper_root(&project_dir)
        .join(&manifest.name)
        .join("web/wrapper");
    generate_wrapper(
        &wrapper_dir,
        &project_dir,
        &opts.source,
        &manifest,
        &opts.user_features,
    )?;

    wasm_pack_build(&wrapper_dir, opts.release, &opts.user_features)?;

    // wasm-pack writes its output under `<wrapper_dir>/pkg/`. Copy it
    // over to `<project_dir>/pkg/` so the user's existing
    // `index.html` (which loads `./pkg/<lib>.js`) keeps working
    // without knowing the wrapper exists.
    let wrapper_pkg = wrapper_dir.join("pkg");
    let project_pkg = project_dir.join("pkg");
    sync_pkg_dir(&wrapper_pkg, &project_pkg)
        .with_context(|| format!("sync {} → {}", wrapper_pkg.display(), project_pkg.display()))?;

    Ok(BuildArtifact {
        pkg_dir: project_pkg,
        wrapper_dir,
    })
}

/// Materialize the wrapper crate at `wrapper_dir`. Idempotent —
/// overwrites whatever was there. Public so a future
/// `idealyst scaffold web` command can drive the same generator.
///
/// `user_features` names cargo features that should be forwarded to
/// the user-crate dep — the wrapper grows a `[features]` block that
/// re-exports each one (`<feat> = ["<user>/<feat>"]`) so a
/// `wasm-pack build -- --features <feat>` invocation against the
/// wrapper turns on the matching feature on the user crate. This is
/// the path AAS-mode hot reload uses to enable `dev-hot-reload` on
/// the user crate without forcing every user crate to carry that
/// feature in its default set.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    user_features: &[String],
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-web-wrapper", manifest.name);
    // `wasm-pack` uses `package.name` (`-` not preserved) to derive
    // the emitted JS filename: e.g. `<name>.js`, `<name>_bg.wasm`.
    // The user's `index.html` references `./pkg/<lib>.js`, where
    // `<lib>` is `manifest.lib_name` (= package name with `-` → `_`).
    // Our wrapper's package name is `<lib>-web-wrapper`; wasm-pack
    // would produce `<lib>_web_wrapper.js`. We force the wasm-pack
    // output to use the original lib name by setting `[lib].name`
    // on the wrapper to `manifest.lib_name`, which wasm-pack
    // prefers over the package name when present.
    let fcore_dep = source.dep("crates/framework/core", &[]);
    let bweb_dep = source.dep("crates/backend/web", &[]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build web`. Do not edit — rewritten
# every build. Run `idealyst scaffold web` to materialize an editable
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

# Forcing `[lib].name = "{lib_name}"` so wasm-pack emits
# `pkg/{lib_name}.js` / `pkg/{lib_name}_bg.wasm` regardless of the
# wrapper's package name — matches what the user's `index.html`
# expects (`import init from "./pkg/{lib_name}.js"`).
[lib]
name = "{lib_name}"
crate-type = ["cdylib"]

[dependencies]
framework-core = {fcore_dep}
backend-web = {bweb_dep}
{user_name} = {{ path = "{user_path}" }}

wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
# Smaller WASM allocator — slightly higher per-alloc cost in exchange
# for a few KB shaved off the bundle.
lol_alloc = "0.4"
{features_section}
# wasm-opt's bundled binaryen rejects bulk-memory ops emitted by recent
# rustc; pass the enable flags explicitly. `-Oz` prioritizes size like
# `opt-level = "z"` does for rustc.
[package.metadata.wasm-pack.profile.release]
wasm-opt = ["-Oz", "--strip-debug", "--strip-producers", "--enable-bulk-memory", "--enable-nontrapping-float-to-int"]
"#,
        wrapper_name = wrapper_name,
        lib_name = manifest.lib_name,
        user_name = manifest.name,
        user_path = project_dir.display(),
        fcore_dep = fcore_dep,
        bweb_dep = bweb_dep,
        features_section = features_section(&manifest.name, user_features),
    );

    let lib_rs = format!(
        r##"//! GENERATED by `idealyst build web`. Mounts `{lib}::app()` on the
//! DOM element selected by `#app`. Boilerplate is identical for
//! every project — only the `app()` call site changes.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — trades a few cycles per allocation for a
// few KB shaved off the bundle.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe {{ lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) }};

thread_local! {{
    /// `mount` returns an `Owner` that must outlive the page. Stash
    /// it in a thread-local so it survives `start()` returning.
    static OWNER: RefCell<Option<framework_core::Owner>> =
        const {{ RefCell::new(None) }};
}}

#[wasm_bindgen(start)]
pub fn start() {{
    console_error_panic_hook::set_once();

    // Scheduler — `after_ms` and the animation clock (raf-driven
    // per-frame ticks for `AnimatedValue` subscribers) both dispatch
    // through `framework_core::scheduling`. Without this neither
    // timer-driven features nor any spring ever ticks.
    backend_web::install_scheduler();
    // Time source — supplies wall-clock readings for the per-frame
    // animation clock's `dt` calculation.
    backend_web::install_time_source();

    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    // Install a Weak self-handle so the framework's
    // `AnimatedValue::bind` family (and any other writer that fires
    // outside the build path) can route into the backend without a
    // `&mut backend` in scope. Mirrors `backend_ios::install_global_self`
    // and `backend_android::install_global_self`.
    backend_web::install_global_self(&backend);
    let owner = framework_core::mount(backend, {lib}::app);
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}}
"##,
        lib = manifest.lib_name,
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}

fn wasm_pack_build(
    wrapper_dir: &Path,
    release: bool,
    user_features: &[String],
) -> Result<()> {
    let mut cmd = Command::new("wasm-pack");
    cmd.current_dir(wrapper_dir)
        .arg("build")
        .args(["--target", "web"]);
    if release {
        cmd.arg("--release");
    } else {
        cmd.arg("--dev");
    }
    // `--` separates wasm-pack flags from cargo flags it forwards.
    // Features go on the cargo side — the wrapper's `[features]`
    // block re-exports each one to the user crate so this turns the
    // feature on for the actual compile.
    if !user_features.is_empty() {
        cmd.arg("--")
            .arg("--features")
            .arg(user_features.join(","));
    }

    eprintln!(
        "[build-web] wasm-pack build --target web{} (in {})",
        if release { " --release" } else { " --dev" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "exec wasm-pack — is it on PATH? (cargo install wasm-pack)")?;
    if !status.success() {
        anyhow::bail!("wasm-pack exited with {status}");
    }
    Ok(())
}

/// Render the wrapper's `[features]` block. Each requested
/// `user_features` entry becomes `<feat> = ["<user>/<feat>"]` so a
/// `wasm-pack build -- --features <feat>` against the wrapper turns
/// that feature on in the user crate. Returns the empty string when
/// no features are requested so the resulting Cargo.toml doesn't
/// gain an empty `[features]` block.
fn features_section(user_name: &str, user_features: &[String]) -> String {
    if user_features.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n[features]\n");
    for feat in user_features {
        // Skip empties — defensive; callers shouldn't pass them but
        // a stray comma in a user-supplied list shouldn't blow up
        // the wrapper Cargo.toml parser.
        if feat.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{feat} = [\"{user_name}/{feat}\"]\n",
            feat = feat,
            user_name = user_name,
        ));
    }
    out
}

/// Mirror `wrapper_pkg/` → `project_pkg/`. We don't trust an OS-level
/// symlink for this — the dev server's static-file logic uses
/// `is_file` checks that would follow the link but cache filenames,
/// and on Windows symlinks need admin. Plain copy is robust and
/// `pkg/` is small (a few hundred KB).
fn sync_pkg_dir(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        anyhow::bail!(
            "wasm-pack reported success but {} doesn't exist",
            src.display()
        );
    }
    // Clean slate — wasm-pack sometimes leaves stale files behind
    // (e.g. renaming the lib renames the .js but leaves the old one).
    if dst.exists() {
        fs::remove_dir_all(dst)
            .with_context(|| format!("remove stale {}", dst.display()))?;
    }
    fs::create_dir_all(dst)
        .with_context(|| format!("create {}", dst.display()))?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copy {} → {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}
