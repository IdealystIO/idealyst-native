//! Web build orchestration for `idealyst build web` and the dev
//! server.
//!
//! Mirror of `crates/build/ios/` and `crates/build/android/`: the
//! user's app crate is intentionally platform-agnostic — it exposes
//! `pub fn app() -> Element` and nothing else. The web target has
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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, FrameworkSource, Manifest};
use flate2::write::GzEncoder;
use flate2::Compression;

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
    /// `["dev-hot-reload"]` for runtime-server-mode hot reload). The wrapper's
    /// Cargo.toml grows a parallel `[features]` block that forwards
    /// each named feature to the user-crate dep, and wasm-pack runs
    /// with `-- --features <list>` so those features are active.
    /// Empty means "default features" — the common case.
    pub user_features: Vec<String>,
    /// When `Some`, after the normal in-project `pkg/` sync also stage
    /// a self-contained static-site bundle at this path. The bundle
    /// contains `index.html`, the fresh `pkg/`, and every top-level
    /// asset directory the user keeps in their project root (anything
    /// that isn't `src/`, `target/`, `tests/`, Cargo metadata, or a
    /// dotfile). When `None`, the bundle step is skipped — the build
    /// behaves exactly as before.
    pub bundle_out_dir: Option<PathBuf>,
    /// Pre-gzip every text-ish file in the staged bundle, writing
    /// gzipped bytes under the original filename. Only meaningful
    /// when `bundle_out_dir` is `Some`; ignored otherwise. The static
    /// host must send `Content-Encoding: gzip` on these responses for
    /// the browser to inflate them transparently.
    pub gzip: bool,
    /// How `lazy! { … }` bodies are handled for this build. See [`SplitMode`].
    pub split: SplitMode,
}

/// Code-splitting policy for `lazy! { … }` bodies on web.
///
/// Wasm **dynamic linking** is the sole web splitter (the dioxus reloc-based
/// `wasm-split` path is retired): when splitting, each `lazy!` body is
/// compiled into its own PIC `--shared` side module (carrying its own data,
/// so heavy data leaves the initial download) and dynamically linked on
/// demand against the PIC main.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    /// Split **iff** the project uses `lazy!`. This is `idealyst build`'s
    /// default — a project with no `lazy!` builds plainly (stable toolchain,
    /// no `build-std`); one that uses `lazy!` gets dynamic-link splitting
    /// (which needs a nightly toolchain with `rust-src`).
    Auto,
    /// Never split — every `lazy!` body compiles inline into the main bundle
    /// (one binary, stable toolchain, no loader). Used by the dev loop
    /// (`idealyst dev`) for fast iteration and by `idealyst build --no-split`.
    Off,
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
    /// Path to the staged static-site bundle, when `bundle_out_dir`
    /// was set on the build options. `None` otherwise.
    pub bundle_dir: Option<PathBuf>,
}

/// Build the user's project at `project_dir` for the web target.
/// Generates the wrapper, runs `wasm-pack build --target web`, and
/// copies the resulting `pkg/` over to `project_dir/pkg/`.
pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;

    // Decide whether to code-split. Wasm dynamic linking is the sole web
    // splitter; we split only in `SplitMode::Auto` AND only when the project
    // actually uses `lazy!` (otherwise there's nothing to split and we avoid
    // imposing the nightly/`build-std` cost). `SplitMode::Off` (dev loop /
    // `--no-split`) always inlines.
    let lazy_bodies = harvest_lazy_bodies(&project_dir)?;
    let do_split = opts.split == SplitMode::Auto && !lazy_bodies.is_empty();

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
        do_split,
    )?;

    // Splitting: PIC `build-std` main + per-`lazy!` PIC `--shared` side
    // modules + a JS dynamic linker. Otherwise fall through to the plain,
    // single-binary path below (every `lazy!` body inlined into main).
    if do_split {
        return run_dynamic_split(&project_dir, &wrapper_dir, &manifest, &opts, lazy_bodies);
    }

    // Plain single-binary path (no splitting). The `lazy!` macro inlines its
    // bodies into the main module, so there's nothing to extract — just
    // `cargo build` → `wasm-bindgen` → (`wasm-opt` on release). The retired
    // dioxus `wasm-split-cli` step is gone.
    let wrapper_pkg = wrapper_dir.join("pkg");
    let original_wasm = wrapper_dir
        .join("target/wasm32-unknown-unknown")
        .join(if opts.release { "release" } else { "debug" })
        .join(format!("{}.wasm", manifest.lib_name));
    cargo_build_wasm(&wrapper_dir, opts.release, &opts.user_features)?;
    wasm_bindgen_build(&original_wasm, &wrapper_pkg, &manifest.lib_name)
        .with_context(|| "wasm-bindgen")?;
    if opts.release {
        wasm_opt_pkg(&wrapper_pkg)
            .with_context(|| "wasm-opt")?;
    }

    let (pkg_dir, bundle_dir) = finalize_pkg(&project_dir, &wrapper_pkg, &opts)?;

    Ok(BuildArtifact {
        pkg_dir,
        wrapper_dir,
        bundle_dir,
    })
}

/// Sync the freshly-built `wrapper_pkg/` into the user project (or stage a
/// self-contained bundle when `bundle_out_dir` is set). Shared tail of both
/// the default and `--dynamic-split` pipelines. Returns `(pkg_dir, bundle_dir)`.
fn finalize_pkg(
    project_dir: &Path,
    wrapper_pkg: &Path,
    opts: &BuildOptions,
) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(out) = opts.bundle_out_dir.as_ref() {
        let staged = stage_bundle(project_dir, out).with_context(|| {
            format!("stage static bundle at {}", out.display())
        })?;
        let staged_pkg = staged.join("pkg");
        sync_pkg_dir(wrapper_pkg, &staged_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), staged_pkg.display())
        })?;
        strip_wasm_pack_metadata(&staged_pkg);
        if opts.gzip {
            gzip_bundle(&staged)
                .with_context(|| format!("gzip bundle at {}", staged.display()))?;
        }
        Ok((staged_pkg, Some(staged)))
    } else {
        let project_pkg = project_dir.join("pkg");
        sync_pkg_dir(wrapper_pkg, &project_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), project_pkg.display())
        })?;
        Ok((project_pkg, None))
    }
}

/// Stage a deployable static-site bundle at `out_dir`. Copies
/// `index.html` (required) and every top-level entry in the project
/// that isn't Rust source, build metadata, a dotfile, or `pkg/`
/// itself. `pkg/` is populated separately by the caller, straight
/// from the wasm-pack output dir — that way the project root never
/// has to carry a `pkg/` for the bundle's sake. `out_dir` is fully
/// cleared first so stale files from a prior bundle (renamed wasm,
/// removed assets) never linger.
///
/// Returns the canonicalized bundle path. Errors when `index.html`
/// is missing — without it there's nothing to serve.
pub fn stage_bundle(project_dir: &Path, out_dir: &Path) -> Result<PathBuf> {
    let index = project_dir.join("index.html");
    if !index.is_file() {
        anyhow::bail!(
            "cannot stage web bundle: {} missing (a web bundle needs an index.html at the \
             project root that loads ./pkg/<lib>.js)",
            index.display(),
        );
    }
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)
            .with_context(|| format!("clear stale bundle {}", out_dir.display()))?;
    }
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create bundle dir {}", out_dir.display()))?;

    for entry in fs::read_dir(project_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if is_excluded_from_bundle(&name_str) {
            continue;
        }
        let from = entry.path();
        let to = out_dir.join(&name);
        if from.is_dir() {
            copy_dir(&from, &to)
                .with_context(|| format!("copy dir {} → {}", from.display(), to.display()))?;
        } else if from.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copy file {} → {}", from.display(), to.display()))?;
        }
    }

    fs::canonicalize(out_dir).with_context(|| format!("canonicalize {}", out_dir.display()))
}

/// Drop wasm-pack housekeeping files from a staged `pkg/`. They're
/// build artifacts that have no place in a deployed bundle:
/// `package.json` makes some CDNs mis-guess directory MIME types,
/// and `.d.ts` files just bloat the wire for browsers that don't
/// touch them.
fn strip_wasm_pack_metadata(staged_pkg: &Path) {
    for stem in ["package.json", ".gitignore", "README.md"] {
        let _ = fs::remove_file(staged_pkg.join(stem));
    }
    if let Ok(read) = fs::read_dir(staged_pkg) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("ts") {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Top-level entries that never belong in a deployable bundle. Source
/// trees, build outputs, VCS metadata, IDE state, package-manager
/// caches, dotfiles in general. Anything not on this list ships —
/// keeps the rule "drop a folder in your project root and it
/// auto-deploys" working for `fonts/`, `assets/`, `public/`,
/// `images/`, etc., without an explicit allowlist.
fn is_excluded_from_bundle(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name,
        "src"
            | "target"
            | "tests"
            | "benches"
            | "examples"
            | "node_modules"
            | "dist"
            | "pkg"
            | "Cargo.toml"
            | "Cargo.lock"
    ) || name.ends_with(".rs")
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to)?;
        }
        // Symlinks intentionally ignored — bundles are meant to be
        // self-contained and portable to remote object storage.
    }
    Ok(())
}

/// Replace every compressible file in `bundle_dir` with its gzipped
/// bytes (keeps the original filename). Skips formats that are already
/// compressed — re-gzipping wastes bytes and CPU and would force the
/// host to advertise the wrong Content-Type.
fn gzip_bundle(bundle_dir: &Path) -> Result<()> {
    fn walk(dir: &Path, on_file: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&path, on_file)?;
            } else if ft.is_file() {
                on_file(&path)?;
            }
        }
        Ok(())
    }
    walk(bundle_dir, &mut |path| {
        if is_already_compressed(path) {
            return Ok(());
        }
        let bytes = fs::read(path)
            .with_context(|| format!("read {} for gzip", path.display()))?;
        let mut enc = GzEncoder::new(Vec::with_capacity(bytes.len()), Compression::best());
        enc.write_all(&bytes)
            .with_context(|| format!("gzip {}", path.display()))?;
        let gz = enc
            .finish()
            .with_context(|| format!("finalize gzip {}", path.display()))?;
        fs::write(path, gz)
            .with_context(|| format!("write gzipped {}", path.display()))?;
        Ok(())
    })
}

fn is_already_compressed(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "ico"
            | "woff"
            | "woff2"
            | "mp4"
            | "mov"
            | "webm"
            | "mp3"
            | "ogg"
            | "m4a"
            | "zip"
            | "gz"
            | "br"
    )
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
/// the path runtime-server-mode hot reload uses to enable `dev-hot-reload` on
/// the user crate without forcing every user crate to carry that
/// feature in its default set.
pub fn generate_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &FrameworkSource,
    manifest: &Manifest,
    user_features: &[String],
    dynamic_split: bool,
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
    let fcore_dep = source.dep("crates/runtime/core", &[]);
    // The wrapper always installs `backend_web::install_async_executor()`
    // so `runtime_core::driver::spawn_async` works inside any
    // wasm app — required by `resource()`, `mutation()`, and the
    // server-fn batch flusher. The export only exists when the
    // `async-driver` feature on `backend-web` is on, so we enable
    // it unconditionally here.
    let bweb_dep = source.dep("crates/backend/web", &["async-driver"]);
    // `dev-client` is only needed in runtime-server mode. Declared as an
    // optional dep so plain wasm builds don't drag the `WireBackend`
    // replay engine into their bundle. We strip the outer braces from
    // `source.dep` so we can splice in `optional = true` alongside the
    // git/path fields.
    let dev_client_raw = source.dep("crates/dev/client", &[]);
    let dev_client_inner = dev_client_raw
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();

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
runtime-core = {fcore_dep}
backend-web = {bweb_dep}
# runtime-server-mode dep. Optional + gated by the `aas` feature so plain wasm
# builds don't pull the `WireBackend` replay engine into their bundle.
dev-client = {{ {dev_client_inner}, optional = true }}
{user_name} = {{ path = "{user_path}" }}

wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
# runtime-server-mode `start()` calls `js_sys::Reflect::get` to read
# `window.IDEALYST_RUNTIME_SERVER_URL` and `web_sys::console` for log lines.
# Both are already in the dep graph via backend-web; declaring them
# here lets the wrapper template reference them directly without
# leaking a transitive-import requirement on backend-web.
js-sys = "0.3"
web-sys = {{ version = "0.3", features = ["Window", "Navigator"] }}
# Smaller WASM allocator — slightly higher per-alloc cost in exchange
# for a few KB shaved off the bundle.
lol_alloc = "0.4"

[features]
# runtime-server-mode hot reload. Activated by `idealyst dev --aas --web`. When
# on, the generated `start()` reads `window.IDEALYST_RUNTIME_SERVER_URL` (the dev
# HTTP server injects it on every served page) and connects the
# `WebBackend` to the runtime-server host over WebSocket via
# `backend_web::connect_web`. When off (plain `idealyst build --web`
# or `idealyst dev --web` without `--aas`), `start()` mounts the
# user's `app()` locally in the browser as before.
# Two flips together: pull in the optional `dev-client` (WireBackend
# replay engine) AND turn on `backend-web/runtime-server`, which is what
# gates the `connect_web` + `WebClientHandle` exports we use below.
aas = ["dep:dev-client", "backend-web/runtime-server"]
{user_feature_forwards}
{patch_block}
# Wrapper-level release profile. wasm-pack is no longer in the
# pipeline (build-web invokes cargo + wasm-bindgen + wasm-split +
# wasm-opt directly), so the wrapper's [profile.release] is what
# governs release builds.
#
# Key choices, all in service of letting wasm-split-cli actually do
# its job:
#   * `lto = "off"` — `"fat"` LTO inlines `#[wasm_split]` annotated
#     functions back into their callers, which puts their body code
#     in main and shrinks the chunk to a stub. "off" preserves the
#     function boundary; wasm-opt's per-chunk pass recovers most of
#     the LTO size win anyway.
#   * `codegen-units = 1` — fewer cross-unit indirections in the
#     emitted wasm gives wasm-split's reachability walker more
#     precision (it's pessimistic across CU boundaries).
#   * `strip = "none"` — symbol names alive for wasm-split's
#     call-graph matching; wasm-opt strips them as a final step.
#   * `debug = "limited"` — line tables stay so wasm-split can match
#     calls to their reloc records; stripped by wasm-opt.
[profile.release]
opt-level = "z"
codegen-units = 1
lto = "off"
panic = "abort"
strip = "none"
debug = "limited"
"#,
        wrapper_name = wrapper_name,
        lib_name = manifest.lib_name,
        user_name = manifest.name,
        user_path = project_dir.display(),
        fcore_dep = fcore_dep,
        bweb_dep = bweb_dep,
        dev_client_inner = dev_client_inner,
        user_feature_forwards = user_feature_forwards(&manifest.name, user_features),
        patch_block = source.patch_block(),
    );

    let lib_rs = format!(
        r##"//! GENERATED by `idealyst build web`. Two start paths, picked by
//! the `aas` cargo feature:
//!
//! - **Default (no feature):** mounts `{lib}::app()` locally on the
//!   DOM element `#app`. The browser runs the framework runtime
//!   directly. This is what `idealyst build --web` produces and what
//!   `idealyst dev --web` (without `--aas`) serves.
//!
//! - **`aas` feature on:** reads `window.IDEALYST_RUNTIME_SERVER_URL` (the dev
//!   HTTP server injects it into every served page) and connects a
//!   `WireBackend<WebBackend>` to the runtime-server host over WebSocket via
//!   `backend_web::connect_web`. The browser becomes a thin replayer;
//!   the framework runtime lives in the runtime-server sidecar. This is what
//!   `idealyst dev --aas --web` produces. Without the feature the
//!   browser would render locally and never connect to runtime-server — the
//!   sidecar would sit idle reporting `0 session(s)` on every
//!   hot-patch.

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
    /// Local-mode: `mount` returns an `Owner` that must outlive the
    /// page. Stash it in a thread-local so it survives `start()`.
    static OWNER: RefCell<Option<runtime_core::Owner>> =
        const {{ RefCell::new(None) }};
    /// runtime-server-mode: the `WebClientHandle` owns the WebSocket + event
    /// closures + raf pump. Drop tears down the connection, so keep it
    /// alive for the page's lifetime.
    #[cfg(feature = "runtime-server")]
    static AAS_HANDLE: RefCell<Option<backend_web::WebClientHandle>> =
        const {{ RefCell::new(None) }};
    /// runtime-server-mode: the `WireBackend` lives behind an `Rc<RefCell<…>>`
    /// because both the `connect_web` raf pump and the on-disconnect
    /// reconnect closure want to retarget it.
    #[cfg(feature = "runtime-server")]
    static AAS_WIRE: RefCell<Option<Rc<RefCell<dev_client::WireBackend<WebBackend>>>>> =
        const {{ RefCell::new(None) }};
}}

// Named `main` (not `start`) because `wasm-split-cli` looks for a
// function called `main` as the entry point of the call graph it
// walks to decide what's reachable from the base bundle vs. only
// from a lazy chunk. The `#[wasm_bindgen(start)]` attribute is what
// actually marks it as the JS-init entry — the function name is
// arbitrary as far as wasm-bindgen is concerned.
#[wasm_bindgen(start)]
pub fn main() {{
    console_error_panic_hook::set_once();

    // Scheduler + time source + async executor + render loop -- every
    // code path needs them. The async executor is what makes
    // `runtime_core::driver::spawn_async` work on wasm; without it any
    // async work (resource fetchers, server-fn calls, mutation
    // triggers) panics at first poll with "no AsyncExecutor
    // installed". The render-loop driver is what makes
    // `runtime_core::driver::render_loop` tick frames; without it,
    // host-web's per-frame paint closure (the wgpu Simulator preview,
    // every future host-driven animation surface) gets a `NoopHandle`
    // and never paints -- the canvas mounts but stays blank.
    backend_web::install_scheduler();
    backend_web::install_time_source();
    backend_web::install_async_executor();
    backend_web::install_render_loop();
    // Push `window.innerWidth/innerHeight` into the framework's
    // reactive viewport signal on initial install + every `resize`
    // event. Author code reads via `runtime_core::viewport_size()`.
    backend_web::install_viewport_observer();
    {dynlink_install}

    #[cfg(feature = "runtime-server")]
    {{
        start_aas();
    }}
    #[cfg(not(feature = "runtime-server"))]
    {{
        start_local();
    }}
}}

/// Local mode: framework runtime lives in this browser. Same flow as
/// `idealyst build --web` (no `--aas`).
#[cfg(not(feature = "runtime-server"))]
fn start_local() {{
    let mut web = WebBackend::new("#app");
    // Hand the bare backend to the user crate so it can install
    // navigator-SDK / external-primitive handlers before mount. The
    // user crate must expose `pub fn register_extensions(&mut WebBackend)`;
    // an empty body is fine when the crate has no SDK deps.
    {lib}::register_extensions(&mut web);
    let backend = Rc::new(RefCell::new(web));
    backend_web::install_global_self(&backend);
    let owner = runtime_core::mount(backend, {lib}::app);
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}}

/// runtime-server mode: framework runtime lives in the runtime-server sidecar on the dev
/// host. The browser is a thin client that replays wire commands and
/// forwards events back.
#[cfg(feature = "runtime-server")]
fn start_aas() {{
    let url = match read_aas_url() {{
        Some(u) => u,
        None => {{
            web_sys::console::error_1(
                &"[dev-client] runtime-server mode enabled but window.IDEALYST_RUNTIME_SERVER_URL is missing — \
                  did the dev HTTP server fail to inject it? Falling back to local mount.".into(),
            );
            // Defensive fallback so the page doesn't go blank.
            let mut web = WebBackend::new("#app");
            {lib}::register_extensions(&mut web);
            let backend = Rc::new(RefCell::new(web));
            backend_web::install_global_self(&backend);
            let owner = runtime_core::mount(backend, {lib}::app);
            OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
            return;
        }}
    }};

    web_sys::console::log_1(
        &format!("[dev-client] runtime-server mode: connecting to {{}}", url).into(),
    );

    let backend = WebBackend::new("#app");
    let outbound = dev_client::OutboundSender::new();
    let wire = Rc::new(RefCell::new(dev_client::WireBackend::new(backend, outbound)));
    AAS_WIRE.with(|slot| *slot.borrow_mut() = Some(wire.clone()));

    let wire_for_reconnect = wire.clone();
    let url_for_reconnect = url.clone();
    let on_disconnect: Rc<dyn Fn()> = Rc::new(move || {{
        // The dev server is likely restarting the sidecar (hot-patch
        // fallback). Try to reconnect; if it fails we'll drop the
        // handle and the page will be inert until next reload.
        let wire = wire_for_reconnect.clone();
        let url = url_for_reconnect.clone();
        let nested_url = url.clone();
        let nested_wire = wire.clone();
        let on_disconnect_again: Rc<dyn Fn()> = Rc::new(move || {{
            web_sys::console::warn_1(&format!(
                "[dev-client] reconnect to {{}} failed; will retry on next disconnect",
                nested_url
            ).into());
            let _ = nested_wire;
        }});
        match backend_web::connect_web(&url, wire, on_disconnect_again) {{
            Ok(h) => {{
                AAS_HANDLE.with(|slot| *slot.borrow_mut() = Some(h));
            }}
            Err(e) => web_sys::console::error_2(
                &"[dev-client] reconnect failed:".into(),
                &e,
            ),
        }}
    }});

    match backend_web::connect_web(&url, wire, on_disconnect) {{
        Ok(h) => {{
            AAS_HANDLE.with(|slot| *slot.borrow_mut() = Some(h));
        }}
        Err(e) => web_sys::console::error_2(
            &"[dev-client] initial runtime-server connect failed:".into(),
            &e,
        ),
    }}
}}

/// Read `window.IDEALYST_RUNTIME_SERVER_URL`, the URL the dev HTTP layer injects
/// into the page via `<script>window.IDEALYST_RUNTIME_SERVER_URL = "..."</script>`.
#[cfg(feature = "runtime-server")]
fn read_aas_url() -> Option<String> {{
    let win = web_sys::window()?;
    js_sys::Reflect::get(&win, &"IDEALYST_RUNTIME_SERVER_URL".into())
        .ok()?
        .as_string()
}}
"##,
        lib = manifest.lib_name,
        // In `--dynamic-split` builds, wire `lazy!`'s `__dynlink_load`
        // seam to the JS dynamic linker. No-op string otherwise so the
        // default pipeline's wrapper is byte-for-byte unchanged.
        dynlink_install = if dynamic_split {
            "backend_web::install_dynlink_loader();"
        } else {
            ""
        },
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}


/// Run `cargo build --target wasm32-unknown-unknown` against the
/// wrapper crate. `RUSTFLAGS="-C link-args=--emit-relocs"` is set
/// (composing with any user-supplied RUSTFLAGS) so the rustc-emitted
/// wasm carries the relocation info wasm-split-cli needs to identify
/// indirect-call targets per chunk. Cost is a few KB of metadata
/// pre-bindgen; stripped from the final bundle by wasm-opt.
fn cargo_build_wasm(
    wrapper_dir: &Path,
    release: bool,
    user_features: &[String],
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(wrapper_dir)
        .arg("build")
        .args(["--target", "wasm32-unknown-unknown"]);
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    let existing_rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    let combined = if existing_rustflags.is_empty() {
        "-C link-args=--emit-relocs".to_string()
    } else {
        format!("{existing_rustflags} -C link-args=--emit-relocs")
    };
    cmd.env("RUSTFLAGS", combined);

    eprintln!(
        "[build-web] cargo build --target wasm32-unknown-unknown{} (in {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "exec cargo")?;
    if !status.success() {
        anyhow::bail!("cargo exited with {status}");
    }
    Ok(())
}

/// Run `wasm-bindgen --target web --keep-lld-exports` to turn the
/// rustc-emitted wasm into the JS-callable wasm-bindgen output.
///
/// `--keep-lld-exports` is the critical flag: without it,
/// wasm-bindgen strips the LLD-emitted exports that wasm-split-cli
/// uses to identify per-chunk reachable code. With them stripped,
/// wasm-split conservatively keeps everything in the main bundle —
/// which is exactly what was happening to the website's bundle in
/// the wasm-pack pipeline.
///
/// We also pass `--keep-debug` so wasm-split has the symbol info it
/// needs to match function references across the relocations. The
/// final wasm-opt pass strips debug info, so this doesn't bloat the
/// shipped bundle.
fn wasm_bindgen_build(
    original_wasm: &Path,
    out_dir: &Path,
    lib_name: &str,
) -> Result<()> {
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)
            .with_context(|| format!("clear {}", out_dir.display()))?;
    }
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create {}", out_dir.display()))?;
    eprintln!(
        "[build-web] wasm-bindgen --target web --keep-lld-exports --keep-debug → {}",
        out_dir.display(),
    );
    let status = Command::new("wasm-bindgen")
        .args(["--target", "web"])
        .arg("--keep-lld-exports")
        .arg("--keep-debug")
        // CRITICAL: --no-demangle. wasm-bindgen demangles Rust
        // symbol names by default. wasm-split-cli matches reloc
        // records (which carry MANGLED names from rustc) against
        // the bindgened wasm's symbol table — demangled names
        // there mean nothing matches, so wasm-split conservatively
        // keeps everything in main and emits empty chunks. Without
        // this flag the website's `lazy! { Simulator(…) }` chunk
        // measured 469 bytes; with it, the wgpu/welcome/sim stack
        // actually moves out of main.
        .arg("--no-demangle")
        .args(["--out-name", lib_name])
        .args(["--out-dir"])
        .arg(out_dir)
        .arg(original_wasm)
        .status()
        .with_context(|| {
            "exec wasm-bindgen — is it on PATH? (cargo install wasm-bindgen-cli --version <matching>)"
        })?;
    if !status.success() {
        anyhow::bail!("wasm-bindgen exited with {status}");
    }
    Ok(())
}

/// Run `wasm-opt -Oz` on every .wasm in `pkg_dir` (the base + each
/// chunk). Runs LAST in the pipeline — after wasm-split — so the
/// optimizer doesn't strip the symbols / reloc info wasm-split
/// needed. Per-chunk optimization keeps chunks lean independently.
fn wasm_opt_pkg(pkg_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(pkg_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let tmp = path.with_extension("wasm.opt");
        let status = Command::new("wasm-opt")
            .arg("-Oz")
            .arg("--strip-debug")
            .arg("--strip-producers")
            .arg("--enable-bulk-memory")
            .arg("--enable-nontrapping-float-to-int")
            .arg("-o")
            .arg(&tmp)
            .arg(&path)
            .status()
            .with_context(|| {
                "exec wasm-opt — is binaryen installed? (`brew install binaryen` / apt etc.)"
            })?;
        if !status.success() {
            anyhow::bail!("wasm-opt failed on {}: {status}", path.display());
        }
        fs::rename(&tmp, &path)?;
        eprintln!("[build-web] wasm-opt → {}", path.display());
    }
    Ok(())
}


/// Render the wrapper's `[features]` block. Each *wrapper-local*
/// feature entry becomes `<feat> = ["<user>/<feat>"]` so a
/// `wasm-pack build -- --features <feat>` against the wrapper turns
/// that feature on in the user crate.
///
/// Cross-crate feature activations of the form `<dep>/<feat>` (e.g.
/// `runtime-core/dev`) are **skipped** here — those are valid
/// cargo command-line arguments to `--features`, but they aren't
/// valid feature *names*, and trying to emit them as keys produces
/// invalid TOML. The build command passes them through to cargo as
/// `--features <dep>/<feat>` directly and cargo activates the
/// underlying feature on the named dep.
///
/// Returns the empty string when no wrapper-local features remain
/// so the resulting Cargo.toml doesn't gain an empty `[features]`
/// block.
/// Render user-feature pass-throughs that sit inside the wrapper's
/// single `[features]` block (the wrapper already declares `aas =
/// ["dep:dev-client"]`; we append the forwards to it). Each
/// wrapper-local feature `<f>` becomes `<f> = ["<user>/<f>"]` so a
/// `wasm-pack build -- --features <f>` invocation against the wrapper
/// flips the matching feature on the user crate.
///
/// Two filters:
/// - **Cross-crate (`<dep>/<feat>`) features are skipped.** Those are
///   already valid cargo `--features` values; no aliasing needed.
/// - **`aas` is skipped** because the wrapper defines it itself
///   (gates `dev-client` + the WireBackend `start()` branch). Without
///   this skip, the forward would emit `aas = ["<user>/aas"]` which
///   collides with the wrapper-local definition and fails cargo
///   resolution.
fn user_feature_forwards(user_name: &str, user_features: &[String]) -> String {
    let local: Vec<&String> = user_features
        .iter()
        .filter(|f| !f.is_empty() && !f.contains('/') && f.as_str() != "aas")
        .collect();
    if local.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for feat in local {
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
    // Recurse so wasm-pack subdirs (notably `snippets/<crate>-<hash>/`
    // for `#[wasm_bindgen(inline_js = ...)]` blocks) come along.
    // Missing snippets/ shows up at runtime as a 404 for
    // `pkg/snippets/.../inline*.js` which the main shim's `import`
    // tries to resolve. `pkg/` is small (a few hundred KB) so
    // straight copy stays cheap.
    copy_dir(src, dst)?;
    Ok(())
}

// ===========================================================================
// Dynamic-linking code-splitting pipeline (`--dynamic-split`)
// ===========================================================================
//
// A wholly separate pipeline from the dioxus reloc-based `wasm-split` above.
// Instead of post-link rewriting a single bundle, it compiles a PIC main
// module + one PIC `--shared` side module per `lazy!` body, all sharing ONE
// `build-std` std artifact so their symbol hashes line up, and ships a JS
// dynamic linker that resolves each side's GOT against the live main instance
// on demand. Proven end-to-end in `crates/tools/dynlink/` (see the
// project_web_dynamic_linking notes). The main upside over the dioxus
// splitter: a side module carries its OWN data segments, so heavy data
// genuinely leaves the initial download.

/// Nightly toolchain used for `-Z build-std` (precompiled std isn't PIC).
/// Overridable via `IDEALYST_DYNLINK_TOOLCHAIN`. Must have `rust-src`.
const DYNLINK_TOOLCHAIN_DEFAULT: &str = "nightly-2025-09-01";

/// The JS dynamic linker, vendored verbatim from the proven spike
/// (`crates/tools/dynlink/loader.mjs`). Resolves a PIC `--shared` side
/// module's imports against an already-instantiated main: env.memory /
/// table → main's; GOT.mem.<sym> → main's exported address-global;
/// GOT.func.<sym> → a fresh table slot holding main's function; the side's
/// own functions land at `__table_base` after the GOT.func slots.
const LOADER_MJS: &str = r##"// Minimal wasm dynamic linker (Emscripten dylink.0 subset). Vendored by
// build-web from crates/tools/dynlink/loader.mjs (proven end-to-end).
export async function loadSide(main, sideMod, { regionBytes = 8 * 1024 * 1024, sideTableReserve = 2048 } = {}) {
  const ex = main.exports;
  const mem = ex.memory;
  const table = ex.__indirect_function_table;
  const g = (v, mut) => new WebAssembly.Global({ value: "i32", mutable: mut }, v);

  const memoryBase = ex.host_reserve(regionBytes);
  const stackTop = memoryBase + regionBytes - 16;

  const imports = {};
  const unresolved = [];
  for (const imp of WebAssembly.Module.imports(sideMod)) {
    const ns = (imports[imp.module] ??= {});
    const { name } = imp;
    if (imp.module === "env" && name === "memory") { ns.memory = mem; continue; }
    if (name === "__indirect_function_table") { ns[name] = table; continue; }
    if (name === "__memory_base") { ns[name] = g(memoryBase, false); continue; }
    if (name === "__table_base")  { continue; }
    if (name === "__stack_pointer") {
      ns[name] = ex.__stack_pointer instanceof WebAssembly.Global ? ex.__stack_pointer : g(stackTop, true);
      continue;
    }
    if (imp.module === "GOT.mem") {
      const a = ex[name];
      if (a instanceof WebAssembly.Global) ns[name] = g(a.value, true);
      else { ns[name] = g(0, true); unresolved.push("GOT.mem." + name); }
      continue;
    }
    if (imp.module === "GOT.func") {
      const fn = ex[name];
      if (typeof fn === "function") {
        const idx = table.length; table.grow(1); table.set(idx, fn);
        ns[name] = g(idx, true);
      } else { ns[name] = g(0, true); unresolved.push("GOT.func." + name); }
      continue;
    }
    if (imp.kind === "function") {
      const fn = ex[name];
      ns[name] = typeof fn === "function" ? fn : () => { throw new Error("unresolved fn " + name); };
      if (typeof fn !== "function") unresolved.push("env." + name);
      continue;
    }
    ns[name] = g(0, true);
  }

  const tableBase = table.length;
  table.grow(sideTableReserve);
  (imports.env ??= {}).__table_base = g(tableBase, false);

  const inst = await WebAssembly.instantiate(sideMod, imports);
  inst.exports.__wasm_apply_data_relocs?.();
  inst.exports.__wasm_call_ctors?.();
  return { side: inst, memoryBase, tableBase, unresolved };
}
"##;

/// Run the dynamic-linking code-splitting build. Produces a PIC main +
/// per-`lazy!` PIC `--shared` side modules + the JS loader/glue, then syncs
/// the result the same way the default path does.
fn run_dynamic_split(
    project_dir: &Path,
    wrapper_dir: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
    bodies: Vec<(String, String)>,
) -> Result<BuildArtifact> {
    let toolchain = std::env::var("IDEALYST_DYNLINK_TOOLCHAIN")
        .unwrap_or_else(|_| DYNLINK_TOOLCHAIN_DEFAULT.to_string());
    let profile_dir = if opts.release { "release" } else { "debug" };
    let target_dir = wrapper_dir.join("target");
    let wrapper_pkg = wrapper_dir.join("pkg");

    // 1. `lazy! { … }` bodies harvested by the caller from the user's source.
    eprintln!(
        "[build-web] dynamic-split: {} lazy! body(ies) harvested",
        bodies.len()
    );

    // 2. Build the PIC main. `IDEALYST_DYNAMIC_SPLIT=1` makes the `lazy!`
    //    macro emit stubs, so the bodies stay OUT of the main module.
    cargo_build_pic(
        wrapper_dir,
        &toolchain,
        opts.release,
        &opts.user_features,
        &["--export-all", "--growable-table", "--export-table"],
        &target_dir,
    )
    .with_context(|| "dynamic-split: build PIC main")?;
    let main_wasm = target_dir
        .join("wasm32-unknown-unknown")
        .join(profile_dir)
        .join(format!("{}.wasm", manifest.lib_name));

    // 3. wasm-bindgen the main (reuses the default path's invocation).
    wasm_bindgen_build(&main_wasm, &wrapper_pkg, &manifest.lib_name)
        .with_context(|| "dynamic-split: wasm-bindgen main")?;

    // 4. Patch the main glue: no-op describe stubs (so `init()` can
    //    instantiate a `--export-all` module), expose main's exports for the
    //    linker, and auto-load the dynlink glue.
    patch_main_glue(&wrapper_pkg, &manifest.lib_name)
        .with_context(|| "dynamic-split: patch main glue")?;

    // 5. Generate + build one PIC `--shared` side module per body, sharing
    //    the main's target dir so std + the user crate are reused and GOT
    //    symbol hashes line up.
    let lazy_root = wrapper_dir.join("lazy");
    let _ = fs::remove_dir_all(&lazy_root);
    let mut hashes = Vec::new();
    for (hash, body) in &bodies {
        let side_dir = lazy_root.join(hash);
        generate_side_crate(&side_dir, hash, body, project_dir, manifest, &opts.source)
            .with_context(|| format!("dynamic-split: generate side crate {hash}"))?;
        cargo_build_pic(&side_dir, &toolchain, opts.release, &[], &["--shared"], &target_dir)
            .with_context(|| format!("dynamic-split: build side module {hash}"))?;
        let side_wasm = target_dir
            .join("wasm32-unknown-unknown")
            .join(profile_dir)
            .join(format!("idealyst_lazy_{hash}.wasm"));
        fs::copy(&side_wasm, wrapper_pkg.join(format!("module_{hash}.wasm")))
            .with_context(|| format!("copy side module {hash}"))?;
        hashes.push(hash.clone());
    }

    // 6. Ship the loader + dynlink glue.
    fs::write(wrapper_pkg.join("loader.mjs"), LOADER_MJS)?;
    write_dynlink_glue(&wrapper_pkg, &manifest.lib_name)?;

    // 7. Release: wasm-opt the main + every side module. Verified safe on
    //    both the `--export-all` PIC main AND the `--shared`/`dylink.0` side
    //    (GOT resolution survives) — a huge win: a debug-std demo main of
    //    35 MB → 10.6 MB (release) → 2.2 MB (wasm-opt); side 20 → 1.6 → 1.0.
    if opts.release {
        wasm_opt_dynsplit(&wrapper_pkg).with_context(|| "dynamic-split: wasm-opt")?;
    }

    eprintln!(
        "[build-web] dynamic-split: main + {} side module(s) emitted",
        hashes.len()
    );

    let (pkg_dir, bundle_dir) = finalize_pkg(project_dir, &wrapper_pkg, opts)?;
    Ok(BuildArtifact {
        pkg_dir,
        wrapper_dir: wrapper_dir.to_path_buf(),
        bundle_dir,
    })
}

/// `cargo +<nightly> rustc -Z build-std=std,panic_abort --target
/// wasm32-unknown-unknown` with PIC RUSTFLAGS, `IDEALYST_DYNAMIC_SPLIT=1`,
/// and a shared `CARGO_TARGET_DIR` (so main + every side reuse one std and
/// the user crate). `final_link_args` are passed to the FINAL crate only via
/// `cargo rustc -- …` so std/deps build once.
fn cargo_build_pic(
    crate_dir: &Path,
    toolchain: &str,
    release: bool,
    user_features: &[String],
    final_link_args: &[&str],
    target_dir: &Path,
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(crate_dir)
        .arg(format!("+{toolchain}"))
        .arg("rustc")
        .arg("-Z")
        .arg("build-std=std,panic_abort")
        .args(["--target", "wasm32-unknown-unknown"]);
    if release {
        cmd.arg("--release");
    }
    if !user_features.is_empty() {
        cmd.arg("--features").arg(user_features.join(","));
    }
    cmd.arg("--");
    for a in final_link_args {
        cmd.arg("-C").arg(format!("link-arg={a}"));
    }

    let existing = std::env::var("RUSTFLAGS").unwrap_or_default();
    let pic = "-C relocation-model=pic -C link-arg=--experimental-pic";
    let combined = if existing.is_empty() {
        pic.to_string()
    } else {
        format!("{existing} {pic}")
    };
    cmd.env("RUSTFLAGS", combined)
        .env("IDEALYST_DYNAMIC_SPLIT", "1")
        .env("CARGO_TARGET_DIR", target_dir);

    eprintln!(
        "[build-web] dynamic-split: cargo +{toolchain} rustc -Z build-std … -- {} (in {})",
        final_link_args.join(" "),
        crate_dir.display(),
    );
    let status = cmd.status().with_context(|| {
        format!(
            "exec cargo +{toolchain} — is the nightly toolchain installed with rust-src? \
             (rustup toolchain install {toolchain} && rustup component add rust-src --toolchain {toolchain})"
        )
    })?;
    if !status.success() {
        anyhow::bail!("cargo (dynamic-split) exited with {status}");
    }
    Ok(())
}

/// Walk every `.rs` under `<project>/src`, find each `lazy! { … }` (anywhere
/// in the token tree — including nested inside `ui!`/`jsx!`, which an AST walk
/// can't see), and return `(hash, body_source)` per unique body. The hash
/// matches the `lazy!` macro's (sha256 of the whitespace-stripped body).
fn harvest_lazy_bodies(project_dir: &Path) -> Result<Vec<(String, String)>> {
    let src = project_dir.join("src");
    let mut bodies = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if src.is_dir() {
        harvest_dir(&src, &mut bodies, &mut seen)?;
    }
    Ok(bodies)
}

fn harvest_dir(
    dir: &Path,
    bodies: &mut Vec<(String, String)>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            harvest_dir(&path, bodies, seen)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            if let Ok(ts) = std::str::FromStr::from_str(&text) {
                find_lazy_in_tokens(ts, bodies, seen);
            }
        }
    }
    Ok(())
}

fn find_lazy_in_tokens(
    ts: proc_macro2::TokenStream,
    bodies: &mut Vec<(String, String)>,
    seen: &mut std::collections::HashSet<String>,
) {
    use proc_macro2::{Delimiter, TokenTree};
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut i = 0;
    while i < toks.len() {
        if let TokenTree::Ident(id) = &toks[i] {
            if id.to_string() == "lazy" {
                if let (Some(TokenTree::Punct(p)), Some(TokenTree::Group(grp))) =
                    (toks.get(i + 1), toks.get(i + 2))
                {
                    if p.as_char() == '!' && grp.delimiter() == Delimiter::Brace {
                        let body = grp.stream().to_string();
                        let hash = body_hash(&body);
                        if seen.insert(hash.clone()) {
                            bodies.push((hash, body));
                        }
                        // Recurse into the body too (a lazy! can nest a lazy!).
                        find_lazy_in_tokens(grp.stream(), bodies, seen);
                        i += 3;
                        continue;
                    }
                }
            }
        }
        if let TokenTree::Group(grp) = &toks[i] {
            find_lazy_in_tokens(grp.stream(), bodies, seen);
        }
        i += 1;
    }
}

/// Hash a `lazy!` body the SAME way the macro does: sha256 of the
/// whitespace-stripped token text, first 6 bytes as hex. Whitespace is
/// stripped because `proc_macro2`'s fallback `to_string` here spaces tokens
/// differently than `proc_macro`'s inside the macro; token *content* matches.
fn body_hash(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let normalized: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let mut h = Sha256::new();
    h.update(normalized.as_bytes());
    let bytes = h.finalize();
    bytes[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Generate a side-module crate that exports `__idealyst_lazy_body_<hash>`,
/// returning a heap `Box<Element>` built from the harvested body. Depends
/// on the user crate (for its components) + runtime-core; a glob `use` of
/// both brings the body's identifiers into scope.
fn generate_side_crate(
    side_dir: &Path,
    hash: &str,
    body: &str,
    project_dir: &Path,
    manifest: &Manifest,
    source: &FrameworkSource,
) -> Result<()> {
    fs::create_dir_all(side_dir.join("src"))
        .with_context(|| format!("create {}", side_dir.display()))?;
    let rc_dep = source.dep("crates/runtime/core", &[]);
    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build --web --dynamic-split`. One side
# module per `lazy!` body; rewritten every build.
[workspace]

[package]
name = "idealyst-lazy-{hash}"
version = "0.0.1"
edition = "2021"

[lib]
name = "idealyst_lazy_{hash}"
crate-type = ["cdylib"]

[dependencies]
runtime-core = {rc_dep}
{user_name} = {{ path = "{user_path}" }}
{patch_block}
[profile.release]
opt-level = "z"
panic = "abort"
"#,
        hash = hash,
        rc_dep = rc_dep,
        user_name = manifest.name,
        user_path = project_dir.display(),
        patch_block = source.patch_block(),
    );

    let lib_rs = format!(
        r##"//! GENERATED side module for a `lazy!` body — do not edit.
#![allow(unused_imports, clippy::all)]
use runtime_core::*;
use {user_lib}::*;

/// Build the body's `Element` and hand main a raw pointer to it (on the
/// SHARED heap; main reconstitutes + mounts via the walker).
#[no_mangle]
pub extern "C" fn __idealyst_lazy_body_{hash}() -> *mut runtime_core::Element {{
    use runtime_core::IntoElement as _;
    let __p: runtime_core::Element = {{ {body} }}.into_element();
    ::std::boxed::Box::into_raw(::std::boxed::Box::new(__p))
}}
"##,
        user_lib = manifest.lib_name,
        hash = hash,
        body = body,
    );

    fs::write(side_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(side_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}

/// Patch the wasm-bindgen main glue for dynamic linking:
///  1. inject no-op `__wbindgen_describe` stubs into `__wbg_get_imports` —
///     `--export-all` keeps the describe machinery alive, so the wasm still
///     imports them; they're never called at runtime.
///  2. export `__idealyst_main_exports()` so the dynlink glue can resolve
///     side modules against the live main instance.
///  3. auto-load the dynlink glue (sets `globalThis.__IDEALYST_DYNLINK`).
fn patch_main_glue(pkg: &Path, lib_name: &str) -> Result<()> {
    let glue_path = pkg.join(format!("{lib_name}.js"));
    let mut glue = fs::read_to_string(&glue_path)
        .with_context(|| format!("read main glue {}", glue_path.display()))?;

    // wasm-bindgen 0.2.x's `__wbg_get_imports` ends with
    //   return {
    //       __proto__: null,
    //       "./<lib>_bg.js": import0,
    //   };
    // Inject the `__wbindgen_placeholder__` namespace as the first key so the
    // dangling describe imports resolve to no-ops. Anchoring on the
    // `return {` + `__proto__: null,` pair is lib-name-agnostic and unique to
    // this function (the loader's own `return { instance, module }` has no
    // `__proto__` line).
    let anchor = "    return {\n        __proto__: null,";
    let injected = "    return {\n        __proto__: null,\n        \
        \"__wbindgen_placeholder__\": { __wbindgen_describe: () => {}, __wbindgen_describe_cast: () => {} },";
    if glue.contains(anchor) {
        glue = glue.replacen(anchor, injected, 1);
    } else {
        anyhow::bail!(
            "dynamic-split: could not find `__wbg_get_imports`'s return object in {} to inject \
             describe stubs (wasm-bindgen glue shape changed?)",
            glue_path.display()
        );
    }

    glue.push_str("\nexport function __idealyst_main_exports() { return wasm; }\n");
    let glue = format!("import \"./__idealyst_dynlink.js\";\n{glue}");
    fs::write(&glue_path, glue)?;
    Ok(())
}

/// Emit `__idealyst_dynlink.js`: installs `globalThis.__IDEALYST_DYNLINK.load`,
/// which `backend_web::dynlink` calls. It fetches `module_<hash>.wasm`, links
/// it against the live main instance via the vendored `loadSide`, and invokes
/// the side's `__idealyst_lazy_body_<hash>` export, returning the raw pointer.
fn write_dynlink_glue(pkg: &Path, lib_name: &str) -> Result<()> {
    let glue = format!(
        r#"// GENERATED by `idealyst build --web --dynamic-split`.
import {{ __idealyst_main_exports }} from "./{lib_name}.js";
import {{ loadSide }} from "./loader.mjs";

const cache = new Map();
globalThis.__IDEALYST_DYNLINK = {{
  load: async (hash) => {{
    const main = {{ exports: __idealyst_main_exports() }};
    let side = cache.get(hash);
    if (!side) {{
      const url = new URL("./module_" + hash + ".wasm", import.meta.url);
      const mod = await WebAssembly.compileStreaming(fetch(url));
      ({{ side }} = await loadSide(main, mod));
      cache.set(hash, side);
    }}
    const fn = side.exports["__idealyst_lazy_body_" + hash];
    if (typeof fn !== "function") {{
      console.error("[idealyst dynlink] missing body export for", hash);
      return 0;
    }}
    return fn();
  }},
}};
"#,
        lib_name = lib_name,
    );
    fs::write(pkg.join("__idealyst_dynlink.js"), glue)?;
    Ok(())
}

/// Run `wasm-opt -Oz` on the main + every side module in `pkg_dir`. Uses a
/// broader feature set than the dioxus path's [`wasm_opt_pkg`] because the PIC
/// dynamic-link modules need mutable-globals (the GOT address-globals),
/// reference-types (wasm-bindgen's externref table), bulk-memory, sign-ext,
/// and nontrapping-float-to-int. Verified end-to-end: the optimized main +
/// `--shared` side still dynamically link (GOT resolution survives) and render
/// in a browser with 0 console errors.
fn wasm_opt_dynsplit(pkg_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(pkg_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let tmp = path.with_extension("wasm.opt");
        let status = Command::new("wasm-opt")
            .arg("-Oz")
            .arg("--strip-debug")
            .arg("--strip-producers")
            .arg("--enable-reference-types")
            .arg("--enable-bulk-memory")
            .arg("--enable-mutable-globals")
            .arg("--enable-nontrapping-float-to-int")
            .arg("--enable-sign-ext")
            .arg("-o")
            .arg(&tmp)
            .arg(&path)
            .status()
            .with_context(|| {
                "exec wasm-opt — is binaryen installed? (`brew install binaryen` / apt etc.)"
            })?;
        if !status.success() {
            anyhow::bail!("wasm-opt failed on {}: {status}", path.display());
        }
        fs::rename(&tmp, &path)?;
        eprintln!("[build-web] dynamic-split: wasm-opt → {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod regression_tests {
    //! Wrapper-shape regression for `build-web`.
    //!
    //! The web wrapper has both a `runtime-core` direct dep (so the
    //! launcher's `--features runtime-core/dev` resolves) AND an
    //! `aas` feature that flips on `backend-web/runtime-server`
    //! (the WebSocket / WireBackend boot path). Dropping either
    //! breaks dev mode silently:
    //!  - no runtime-core dep → `--features runtime-core/dev` errors
    //!    at cargo time, MCP catalog ends up empty.
    //!  - no `aas` feature on the wrapper → `idealyst dev --web
    //!    --runtime-server` builds a wasm bundle that mounts
    //!    `app()` locally in the browser instead of connecting to
    //!    the dev-host, and saves visibly do nothing.

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
                splash: SplashConfig {
                    background: "#000000".to_string(),
                    title: "Demo".to_string(),
                    title_color: "#ffffff".to_string(),
                    duration_ms: 0,
                },
                targets: Vec::new(),
                server_bin: None,
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
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, &[], false)
            .expect("generate wrapper");
        (wrapper_dir, tmp)
    }

    #[test]
    fn wrapper_has_runtime_core_dep() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        assert!(
            parsed
                .get("dependencies")
                .and_then(|d| d.get("runtime-core"))
                .is_some(),
            "web wrapper missing runtime-core dep — launcher's \
             `--features runtime-core/dev` will fail. Got:\n{cargo}",
        );
    }

    #[test]
    fn wrapper_aas_feature_pulls_backend_web_runtime_server() {
        let (wrapper_dir, _tmp) = run_generator();
        let cargo = std::fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).expect("valid TOML");
        let aas = parsed
            .get("features")
            .and_then(|f| f.get("aas"))
            .and_then(|a| a.as_array())
            .expect("web wrapper declares the `aas` feature");
        let entries: Vec<&str> = aas.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            entries.iter().any(|e| *e == "backend-web/runtime-server"),
            "web wrapper `aas` feature must enable backend-web/runtime-server; \
             without it, `idealyst dev --web --runtime-server` produces a \
             local-mount bundle that won't connect to the dev-host. Got {:?}",
            entries,
        );
    }
}

#[cfg(test)]
mod bundle_tests {
    //! Coverage for `idealyst build --web --gzip --out-dir`. These
    //! tests don't run wasm-pack — they drive `stage_bundle` /
    //! `gzip_bundle` against a synthetic project layout, so they
    //! stay fast (<10ms) and don't need a wasm toolchain on CI.

    use super::*;
    use std::io::Read as _;

    fn fake_project(tmp: &Path) -> PathBuf {
        let project = tmp.join("proj");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(project.join("target/debug")).unwrap();
        // A stale project-root pkg/ left over from a previous build —
        // bundling must NOT pick it up (the freshly built pkg comes
        // straight from the wasm-pack output dir).
        fs::create_dir_all(project.join("pkg")).unwrap();
        fs::create_dir_all(project.join("fonts")).unwrap();
        fs::create_dir_all(project.join("assets/images")).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join("Cargo.toml"), b"[package]\nname = 'demo'\n").unwrap();
        fs::write(project.join("Cargo.lock"), b"").unwrap();
        fs::write(project.join("index.html"), b"<html><body>hi</body></html>").unwrap();
        fs::write(project.join("src/lib.rs"), b"pub fn app() {}").unwrap();
        fs::write(project.join("target/debug/junk"), b"big-binary").unwrap();
        fs::write(project.join("pkg/STALE_FROM_OLD_BUILD.wasm"), b"old-bytes").unwrap();
        fs::write(project.join("fonts/Inter.ttf"), b"font-bytes").unwrap();
        fs::write(project.join("assets/images/logo.png"), b"png-bytes").unwrap();
        project
    }

    fn read_gzipped(path: &Path) -> Vec<u8> {
        let raw = fs::read(path).expect("read gz");
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out).expect("decode gz");
        out
    }

    #[test]
    fn stage_bundle_keeps_assets_skips_sources_and_pkg() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        stage_bundle(&project, &out).expect("stage");

        assert!(out.join("index.html").is_file(), "index.html must be copied");
        assert!(
            out.join("fonts/Inter.ttf").is_file(),
            "top-level asset dir (fonts/) must auto-ship",
        );
        assert!(
            out.join("assets/images/logo.png").is_file(),
            "nested asset paths must auto-ship",
        );
        assert!(!out.join("src").exists(), "src/ must be skipped");
        assert!(!out.join("target").exists(), "target/ must be skipped");
        assert!(!out.join(".git").exists(), "dotdirs must be skipped");
        assert!(!out.join("Cargo.toml").exists(), "Cargo.toml must be skipped");
        // Bundling owns pkg/ — it gets populated from wasm-pack output
        // by the caller, NOT scraped out of the project root. A stale
        // project-root pkg/ from a previous build must not leak in,
        // or deployments would ship outdated wasm.
        assert!(
            !out.join("pkg").exists(),
            "stage_bundle must not copy project/pkg/ — the caller copies wrapper_pkg straight in",
        );
    }

    #[test]
    fn stage_bundle_errors_without_index_html() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let err = stage_bundle(&project, &tmp.path().join("dist")).unwrap_err();
        assert!(
            err.to_string().contains("index.html"),
            "missing-index error should mention index.html, got: {err}",
        );
    }

    #[test]
    fn stage_bundle_replaces_prior_output() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");

        // Pretend a previous build left a stale artifact behind.
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("ghost.wasm"), b"old").unwrap();

        stage_bundle(&project, &out).expect("stage");
        assert!(
            !out.join("ghost.wasm").exists(),
            "stale files from a prior bundle must be cleared so renamed/removed assets don't leak",
        );
    }

    #[test]
    fn strip_wasm_pack_metadata_drops_housekeeping_only() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("demo_bg.wasm"), b"wasm").unwrap();
        fs::write(pkg.join("demo.js"), b"js").unwrap();
        fs::write(pkg.join("demo.d.ts"), b"types").unwrap();
        fs::write(pkg.join("demo_bg.wasm.d.ts"), b"types").unwrap();
        fs::write(pkg.join("package.json"), b"{}").unwrap();
        fs::write(pkg.join("README.md"), b"# pkg").unwrap();

        strip_wasm_pack_metadata(&pkg);

        assert!(pkg.join("demo_bg.wasm").is_file(), ".wasm must stay");
        assert!(pkg.join("demo.js").is_file(), ".js must stay");
        assert!(!pkg.join("demo.d.ts").exists(), ".d.ts must be stripped");
        assert!(!pkg.join("demo_bg.wasm.d.ts").exists(), ".d.ts must be stripped");
        assert!(!pkg.join("package.json").exists(), "package.json must be stripped");
        assert!(!pkg.join("README.md").exists(), "README.md must be stripped");
    }

    #[test]
    fn gzip_bundle_compresses_text_skips_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let project = fake_project(tmp.path());
        let out = tmp.path().join("dist");
        stage_bundle(&project, &out).expect("stage");

        // Drop in a synthetic pkg/ the way `build()` would after
        // copying from `wrapper_pkg`. Wasm body is intentionally
        // long-and-repetitive so gzip noticeably shrinks it; without
        // that the test could flake on tiny inputs where the gzip
        // header outweighs the savings.
        let pkg = out.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        let wasm_raw = "abcdefgh".repeat(2000).into_bytes();
        fs::write(pkg.join("demo_bg.wasm"), &wasm_raw).unwrap();
        fs::write(pkg.join("demo.js"), b"export default function init() {}").unwrap();
        let png_raw = fs::read(out.join("assets/images/logo.png")).unwrap();

        gzip_bundle(&out).expect("gzip");

        // Compressible: wasm replaced by gzip bytes; filename unchanged
        // so the same `Content-Encoding: gzip` response can serve them.
        let wasm_after = fs::read(out.join("pkg/demo_bg.wasm")).unwrap();
        assert_ne!(
            wasm_raw, wasm_after,
            "wasm must be replaced by gzipped bytes (filename preserved)",
        );
        assert!(
            wasm_after.len() < wasm_raw.len(),
            "gzip must shrink the wasm (was {}, now {})",
            wasm_raw.len(),
            wasm_after.len(),
        );
        assert_eq!(
            read_gzipped(&out.join("pkg/demo_bg.wasm")),
            wasm_raw,
            "gzipped wasm must round-trip back to the original bytes",
        );

        // Pre-compressed formats must be left alone — re-gzipping
        // wastes bytes and would confuse the host's Content-Type
        // routing.
        assert_eq!(
            fs::read(out.join("assets/images/logo.png")).unwrap(),
            png_raw,
            ".png must not be re-compressed",
        );
    }

    #[test]
    fn is_already_compressed_covers_common_web_assets() {
        // Sanity: the skip-list keys off lowercase extension. A
        // capital-letter extension (.PNG from a careless author) must
        // still skip.
        assert!(is_already_compressed(Path::new("a.png")));
        assert!(is_already_compressed(Path::new("a.PNG")));
        assert!(is_already_compressed(Path::new("a.woff2")));
        assert!(is_already_compressed(Path::new("a.mp4")));
        assert!(!is_already_compressed(Path::new("a.wasm")));
        assert!(!is_already_compressed(Path::new("a.js")));
        assert!(!is_already_compressed(Path::new("a.html")));
        assert!(!is_already_compressed(Path::new("a.ttf")));
    }
}
