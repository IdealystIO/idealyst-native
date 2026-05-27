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

    // wasm-pack writes its output under `<wrapper_dir>/pkg/`. Two
    // destinations from here:
    //   * No bundle (dev loop, legacy "open index.html locally" flow):
    //     sync into `<project_dir>/pkg/` so the user's `index.html`
    //     (which loads `./pkg/<lib>.js`) and the dev HTTP server can
    //     find the freshly built JS / wasm at the path they expect.
    //   * Bundle requested: copy straight into `<bundle>/pkg/`. We
    //     deliberately do NOT also sync to `<project_dir>/pkg/` —
    //     bundling is for deployment, and littering the user's
    //     project root with build artifacts is a footgun (and was
    //     a complaint).
    let wrapper_pkg = wrapper_dir.join("pkg");
    let (pkg_dir, bundle_dir) = if let Some(out) = opts.bundle_out_dir.as_ref() {
        let staged = stage_bundle(&project_dir, out).with_context(|| {
            format!("stage static bundle at {}", out.display())
        })?;
        let staged_pkg = staged.join("pkg");
        sync_pkg_dir(&wrapper_pkg, &staged_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), staged_pkg.display())
        })?;
        strip_wasm_pack_metadata(&staged_pkg);
        if opts.gzip {
            gzip_bundle(&staged)
                .with_context(|| format!("gzip bundle at {}", staged.display()))?;
        }
        (staged_pkg, Some(staged))
    } else {
        let project_pkg = project_dir.join("pkg");
        sync_pkg_dir(&wrapper_pkg, &project_pkg).with_context(|| {
            format!("sync {} → {}", wrapper_pkg.display(), project_pkg.display())
        })?;
        (project_pkg, None)
    };

    Ok(BuildArtifact {
        pkg_dir,
        wrapper_dir,
        bundle_dir,
    })
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

#[wasm_bindgen(start)]
pub fn start() {{
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
        generate_wrapper(&wrapper_dir, &project_dir, &source, &manifest, &[])
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
