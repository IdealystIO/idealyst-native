//! Single-process AAS host with dlopen-driven hot reload.
//!
//! ## Architecture
//!
//! Two generated crates living in a shared sub-workspace under
//! `<workspace>/target/idealyst/<project>/aas/dylib-mode/`:
//!
//! - `host/`  — long-lived process. Statically links the user crate
//!              + `framework-core` (with the `hot-reload` feature on,
//!              so the `#[component]` macro emits `__*_hot_impl`
//!              inner fns and wraps every component in
//!              `framework_hot::call`). Runs `framework_core::render`,
//!              serves WebSocket clients, and watches the user's
//!              source for changes.
//!
//! - `patch/` — built on every source change. A Rust `dylib` whose
//!              source is just `pub use <user_crate>::*;`. That
//!              re-export pulls every `__*_hot_impl` function from
//!              the user crate's recompiled rlib into the dylib's
//!              symbol table. `framework_hot::diff::apply_from_dylib`
//!              then diffs the host's symbol table against the patch
//!              and installs a `subsecond` jump table — subsequent
//!              calls into any patched component dispatch into the
//!              dylib's body.
//!
//! ## Why this works on macOS
//!
//! The hard problem with cross-image hot reload on Darwin is the
//! `thread_local!` storage: TLV (thread-local variable) opcodes
//! don't resolve uniformly across separately-linked images. Two
//! options solve it:
//!
//! 1. Statically link `framework-core` into the host bin, expose its
//!    symbols with `-Wl,-export_dynamic`, leave them as undefined
//!    references in the patch (`-undefined dynamic_lookup`). At
//!    dlopen, dyld walks the loaded images and resolves the patch's
//!    framework refs back to the host bin's exports. TLV access
//!    happens inside the host bin's code, which is consistent — both
//!    host code paths and patch-originated calls land in the same
//!    image's TLV opcodes.
//!
//! 2. Compile `framework-core` itself as its own `dylib`. The host
//!    bin's `LC_LOAD_DYLIB` points to `libframework_core.dylib`; the
//!    patch's `LC_LOAD_DYLIB` points to the same dylib. dyld notices
//!    the shared dependency and unifies the load — TLV access
//!    happens inside `libframework_core.dylib`, again consistent.
//!
//! We use approach 2. It's the simpler build pipeline: cargo emits
//! the framework dylib automatically once `crate-type = ["rlib",
//! "dylib"]` is set, and `-C prefer-dynamic` is enough to tell rustc
//! to pick the dynamic variant when linking the host and patch.
//! No bespoke `rustc --emit=obj` + manual `ld` invocation required.
//!
//! ## Patch rebuild pipeline
//!
//! On file change, the host runs `cargo build -p patch` inside the
//! sub-workspace. Because `framework-core` (and every other
//! dependency) is already compiled and cached in the shared target
//! dir, only the user crate's incremental compilation + the patch's
//! relink runs. Empirical numbers in the comments around the bottom
//! of this file.
//!
//! After the build succeeds, the host calls
//! `framework_hot::diff::apply_from_dylib(<patch_dylib_path>)`. That
//! function:
//!
//! - Snapshots the host bin's symbol table on first call (lazy
//!   `OnceLock`).
//! - Parses the freshly-built patch dylib's symbol table.
//! - For every `__*_hot_impl` symbol present in both, records a
//!   `(host_offset, patch_offset)` pair.
//! - Constructs a `subsecond::JumpTable` and applies it.
//!
//! From that point on, `framework_hot::call(__Counter_hot_impl,
//! args)` dispatches into the patch dylib's `__Counter_hot_impl`
//! body. The walker rebuilds reactive scopes lazily — the next
//! signal change involving a patched component re-walks with the new
//! body, and the updated UI is broadcast to every connected client.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::Manifest;
use object::{Object, ObjectSymbol};

use crate::{
    host_binary_name, write_shared_target_config, BuildArtifact, BuildOptions, DEFAULT_BIND_ADDR,
};

pub(crate) fn build(
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<BuildArtifact> {
    let mode_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("aas/dylib-mode");
    let host_dir = mode_dir.join("host");
    let patch_dir = mode_dir.join("patch");

    generate_workspace_root(&mode_dir)?;
    generate_patch_crate(&patch_dir, project_dir, workspace_root, manifest)?;
    generate_host_crate(
        &host_dir,
        &patch_dir,
        project_dir,
        workspace_root,
        manifest,
    )?;
    write_shared_target_config(&mode_dir, workspace_root)?;

    // Build host + patch in a single cargo invocation. This is
    // critical: if we ran two separate `cargo build` calls, cargo
    // could pick different `framework-core` fingerprints for each
    // (the host's bin doesn't *need* the `dylib` output, while the
    // patch does, and that crate-type difference would split the
    // fingerprint). The two artifacts would then disagree on the
    // monomorphization hashes embedded in their mangled symbol
    // references, and `dyld` would refuse to load
    // `libframework_core.dylib` for the host at startup.
    //
    // A single `cargo build -p host -p patch` plans one
    // framework-core compilation that produces both rlib + dylib
    // and is consumed by both binaries — same hashes, no
    // mismatch.
    cargo_build_both(&mode_dir, opts.release)?;

    let profile = if opts.release { "release" } else { "debug" };
    let workspace_target = workspace_root.join("target");
    let host_binary = workspace_target
        .join(profile)
        .join(host_binary_name(&manifest.name));
    let patch_path = find_patch_dylib(&workspace_target, profile)?;

    if !host_binary.is_file() {
        anyhow::bail!(
            "cargo build reported success but host binary not at {}",
            host_binary.display(),
        );
    }

    // Capture the host's symbol table so subsequent patch links can
    // resolve undefined references against the exact link-time
    // addresses the running host bin will end up with. (See
    // `cli::cmd::link_patch` for how the patch consumes this.)
    capture_host_symbols(&host_binary, &mode_dir.join("host-symbols.json"))?;

    Ok(BuildArtifact {
        host_binary,
        // Naming kept for backwards compat with the CLI's artifact
        // display logic. The CLI prints both paths.
        sidecar_binary: patch_path,
        wrapper_dir: host_dir,
        sidecar_dir: patch_dir,
    })
}

// ---------------------------------------------------------------------------
// Sub-workspace root
// ---------------------------------------------------------------------------

fn generate_workspace_root(mode_dir: &Path) -> Result<()> {
    fs::create_dir_all(mode_dir)
        .with_context(|| format!("create {}", mode_dir.display()))?;
    let cargo_toml = r#"# GENERATED by `idealyst build aas` (dylib mode). Do not edit —
# rewritten every build.
#
# Sub-workspace that owns the host bin + patch dylib. Both members
# share dep resolution, so `framework-core`, `wire`, etc. compile
# once and the patch's symbol references match the host's by
# mangled hash.

[workspace]
resolver = "2"
members = ["host", "patch"]

# Keep cargo's default `[profile.dev]` (especially `debug-assertions
# = true`). Subsecond's hot-reload dispatcher is gated on
# `cfg!(debug_assertions)`: when it's off, `HotFn::call` short-
# circuits to a direct call and never consults the jump table, so
# any custom profile here must NOT disable assertions or the
# patches go nowhere.
"#;
    fs::write(mode_dir.join("Cargo.toml"), cargo_toml)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Patch crate
// ---------------------------------------------------------------------------

fn generate_patch_crate(
    patch_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(patch_dir.join("src"))
        .with_context(|| format!("create {}", patch_dir.display()))?;

    let fcore = workspace_root.join("crates/framework/core");
    let fhot = workspace_root.join("crates/framework/hot");
    let user_path = project_dir;
    let user_name = &manifest.name;

    // `dylib` (not `cdylib`) — preserves Rust ABI and, crucially,
    // honors `-C prefer-dynamic` for upstream rlib-or-dylib deps.
    // A `cdylib` would silently statically embed `framework-core`,
    // defeating the shared-image architecture.
    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas` (dylib mode). Do not edit.
#
# Patch dylib: re-exports the user crate so every `__*_hot_impl`
# inner function the `#[component]` macro emitted ends up in this
# dylib's symbol table. `framework_hot::diff` diffs the symbols
# against the running host bin and installs a subsecond jump table.

[package]
name = "patch"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["dylib"]

[dependencies]
# Match the host's framework dep so the hash, feature set, and code
# version are identical across the host link and the patch link.
framework-core = {{ path = "{fcore}", features = ["hot-reload"] }}
# CRITICAL: depend on `framework-hot` with the exact features the
# host enables (`hot` + `diff`). Without this, the watcher's
# `cargo build -p patch` resolves a feature union of `["hot"]`
# only (because the host isn't in the build target set), which
# is a different fingerprint from the initial `-p host -p patch`
# build's union of `["hot", "diff"]`. The drift forces cargo to
# recompile framework-hot AND framework-core (cascade), overwriting
# `libframework_core.dylib` with new mangled symbol hashes the
# running host bin doesn't match — and the next patch's calls
# into framework_core fail to lazy-bind, taking the host down.
framework-hot = {{ path = "{fhot}", features = ["hot", "diff"] }}
# Pulled in for the `__*_hot_impl` re-exports — that's why we depend
# on the user crate directly instead of going through the host.
{user_name} = {{ path = "{user_path}" }}
"#,
        fcore = fcore.display(),
        fhot = fhot.display(),
        user_name = user_name,
        user_path = user_path.display(),
    );

    // Rust quirk: `pub use foo::*` only re-exports items NAMED `foo::*`,
    // not private items. `__*_hot_impl` functions are emitted with
    // `#[doc(hidden)]` but pub-by-default — they re-export cleanly.
    //
    // The `#[export_name = "main"]` stub is required by
    // `subsecond::apply_patch`: it hardcodes a lookup for a `main`
    // symbol in the patch dylib to use as the ASLR baseline (the
    // assumption upstream is that hot patches are bins rebuilt with
    // `--crate-type=bin`; our AAS architecture builds them as
    // `dylib`s, so we synthesize the symbol here). The function is
    // never called — subsecond only reads its address.
    let lib_rs = format!(
        r#"//! GENERATED patch dylib. Re-exports the user crate so its
//! `__*_hot_impl` component bodies land in this dylib's symbol
//! table. `framework_hot::diff::apply_from_dylib` reads those
//! symbols on each rebuild and hot-swaps the host's component impls.

#[allow(unused_imports)]
pub use {user_name}::*;

/// Stable ASLR reference symbol — subsecond looks for `main` in the
/// patch dylib to compute the dlopen'd image's runtime base. We
/// don't have a real `main` here (this is a `dylib`, not a bin), so
/// export an empty stub under that name. Never called.
#[no_mangle]
pub extern "C" fn main() {{}}
"#,
        user_name = user_name,
    );

    fs::write(patch_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(patch_dir.join("src/lib.rs"), lib_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Host crate
// ---------------------------------------------------------------------------

fn generate_host_crate(
    host_dir: &Path,
    patch_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(host_dir.join("src"))
        .with_context(|| format!("create {}", host_dir.display()))?;

    let host_name = host_binary_name(&manifest.name);
    let fcore = workspace_root.join("crates/framework/core");
    let fhot = workspace_root.join("crates/framework/hot");
    let dev_server = workspace_root.join("crates/dev/server");
    let wire = workspace_root.join("crates/framework/wire");
    let user_name = &manifest.name;
    let user_path = project_dir;

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas` (dylib mode). Do not edit.
#
# Host bin: long-lived process that owns the WebSocket server, the
# framework reactive runtime, and the file watcher. Statically links
# the user crate so the initial render path doesn't need the patch
# dylib loaded at all — the patch only kicks in once a source change
# triggers a rebuild + apply.
#
# Package name is just `host` so the build helper can address it
# with `-p host`. The shipped binary name is the user-friendly
# `<project>-aas-host` (set via `[[bin]] name = ...`) to match the
# sidecar-mode naming convention.

[package]
name = "host"
version = "0.0.1"
edition = "2021"

[[bin]]
name = "{host_name}"
path = "src/main.rs"

[dependencies]
framework-core = {{ path = "{fcore}", features = ["hot-reload"] }}
framework-hot = {{ path = "{fhot}", features = ["hot", "diff"] }}
dev-server = {{ path = "{dev_server}" }}
wire = {{ path = "{wire}" }}
{user_name} = {{ path = "{user_path}" }}
serde_json = "1"
# Needed for `dlsym(RTLD_DEFAULT, "main")` at host startup — see
# the host's `main.rs` for why we need the C-ABI `_main` runtime
# address rather than Rust's `fn main`.
libc = "0.2"
"#,
        host_name = host_name,
        fcore = fcore.display(),
        fhot = fhot.display(),
        dev_server = dev_server.display(),
        wire = wire.display(),
        user_name = user_name,
        user_path = user_path.display(),
    );

    let host_main = generate_host_main(
        manifest,
        host_dir,
        patch_dir,
        project_dir,
        workspace_root,
    )?;

    fs::write(host_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(host_dir.join("src/main.rs"), host_main)?;
    Ok(())
}

fn generate_host_main(
    manifest: &Manifest,
    _host_dir: &Path,
    patch_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
) -> Result<String> {
    let app_id = manifest.app.require_bundle_id()?;
    let user_src = project_dir.join("src");
    let workspace_target = workspace_root.join("target");
    let mode_dir = patch_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| workspace_target.clone());
    let mode_workspace_manifest = mode_dir.join("Cargo.toml");
    // Same RUSTFLAGS the initial build used. The host re-uses these
    // for every patch rebuild so framework-core's fingerprint stays
    // constant — divergence would invalidate the running host's
    // pointer-equality with `libframework_core.dylib` and break
    // every subsequent dlopen.
    let rustflags = rustflags_for_dylib_mode()?;
    // Absolute path to the running idealyst CLI binary — the host's
    // watcher invokes `<idealyst> rebuild-patch <mode_dir>` for the
    // fast rustc-replay path. Baked at host-build time so the host
    // doesn't depend on PATH resolution at runtime.
    let idealyst_bin = std::env::current_exe()
        .context("locate idealyst CLI path for the host's fast-rebuild command")?;
    Ok(format!(
        r#"//! GENERATED dylib-mode AAS host. Single process; statically
//! links the user crate and `framework-core` (with `hot-reload`
//! on) so the `#[component]` macro emits `__*_hot_impl` bodies.
//! On file change, builds the patch dylib and asks
//! `framework_hot::diff::apply_from_dylib` to hot-swap the
//! components.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{{Arc, Mutex}};

use dev_server::{{
    serve_with_tick_and_port, spawn_rebuild_loop, RebuildCommand, RebuildConfig,
    WireRecordingBackend,
}};
use framework_core::{{render, Owner}};
use {lib}::app;

const DEFAULT_ADDR: &str = "{default_addr}";
/// mDNS-published app identifier.
const APP_ID: &str = "{app_id}";

/// Path to the freshly-built patch dylib. Recomputed lazily on
/// every rebuild because cargo's fingerprint hash can change with
/// any source edit that ripples through dep graph metadata.
const PATCH_PROFILE_DIR: &str = "{profile_dir}";
const PATCH_WORKSPACE_TARGET: &str = "{workspace_target}";
/// RUSTFLAGS baked at build time. Every patch rebuild reuses this
/// so framework-core's compilation fingerprint stays equal to the
/// one statically linked into this host bin.
const PATCH_RUSTFLAGS: &str = "{rustflags}";
/// AAS dylib-mode sub-workspace dir. The thin-link patch builder
/// reads `host-symbols.json` from here and writes `host-base.txt`
/// (our runtime `_main` address) into it on startup so subsequent
/// patches can compute the ASLR slide.
const MODE_DIR: &str = "{mode_dir}";

fn main() -> std::io::Result<()> {{
    let addr = if let Some(a) = std::env::args().nth(1) {{
        a
    }} else if let Ok(p) = std::env::var("IDEALYST_AAS_BIND_PORT") {{
        format!("0.0.0.0:{{}}", p)
    }} else {{
        DEFAULT_ADDR.to_string()
    }};

    // Propagate to child cargo invocations.
    std::env::set_var("RUSTFLAGS", PATCH_RUSTFLAGS);

    // Publish our runtime C-ABI `_main` address so external linker
    // processes (the watcher's `idealyst link-patch` step) can
    // compute the ASLR slide. Critical subtlety: Rust's `main as
    // usize` returns the address of *the Rust `fn main` you
    // declared*, NOT the C-ABI `_main` entry that dyld jumps to.
    // The host-symbols.json file captures `_main`'s link-time
    // address (the C-ABI one), so we have to query the same one
    // at runtime — via `dlsym(RTLD_DEFAULT, "main")`.
    let main_addr: usize = unsafe {{
        let p = libc::dlsym(libc::RTLD_DEFAULT, b"main\0".as_ptr() as *const _);
        if p.is_null() {{ 0 }} else {{ p as usize }}
    }};
    let path = std::path::Path::new(MODE_DIR).join("host-base.txt");
    if let Err(e) = std::fs::write(&path, format!("0x{{:x}}\n", main_addr)) {{
        eprintln!(
            "[dylib-host] could not write host base to {{}}: {{}} — hot-reload patches will fail",
            path.display(),
            e
        );
    }} else {{
        eprintln!(
            "[dylib-host] runtime _main (C-ABI) = 0x{{:x}} → {{}}",
            main_addr,
            path.display()
        );
    }}

    // Spin up the framework reactive runtime against the recording
    // backend. The owner is kept in a `RefCell` so the per-tick
    // hot-reload handler can drop the old reactive tree and
    // re-render with the patched `app()` body, propagating
    // structural edits (added/removed nodes, changed initial
    // values, etc.) to every connected client. Without the
    // re-render, swapping the jump table only takes effect on the
    // next signal change — which is fine for reactive edits but
    // misses static structural changes.
    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let owner: Rc<RefCell<Option<Owner>>> =
        Rc::new(RefCell::new(Some(render(backend_rc.clone(), app()))));

    // Pending-patch slot: file-watch thread drops a patch dylib path
    // here on every successful rebuild; the serve loop's per-tick
    // callback picks it up and calls `apply_from_dylib`.
    //
    // `apply_from_dylib` ultimately calls `subsecond::apply_patch`,
    // which dlopens the patch dylib + rewrites the global jump
    // table. We deliberately do it on the serve-loop thread (same
    // one that owns the framework runtime) — that's the safest place
    // to mutate runtime state, and the call is fast enough (~ms)
    // not to stutter the broadcast loop.
    let pending_patch: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

    // Initial patch apply — the patch dylib was built alongside the
    // host so its `__*_hot_impl` symbols match what the host bin
    // statically linked. Pushing it through `apply_from_dylib`
    // populates `subsecond`'s base address map (the `aslr_reference`
    // baseline) so subsequent rebuild-applies have a valid diff to
    // compute against.
    let initial_patch = patch_path();
    if let Some(p) = &initial_patch {{
        eprintln!("[dylib-host] initial patch: {{}}", p.display());
        match unsafe {{ framework_hot::diff::apply_from_dylib(p) }} {{
            Ok(_) => eprintln!("[dylib-host] initial jump-table installed"),
            Err(e) => eprintln!("[dylib-host] initial apply failed: {{:?}} (ok to ignore; hot-reload still active)", e),
        }}
    }}

    // File watcher → cargo build patch → drop the new dylib path
    // into `pending_patch`. The serve-loop tick picks it up on the
    // next iteration.
    let pending_for_watch = pending_patch.clone();
    spawn_rebuild_loop(RebuildConfig {{
        command: RebuildCommand {{
            program: "{idealyst_bin}".into(),
            args: vec![
                "link-patch".into(),
                "{mode_dir}".into(),
            ],
            // Thin-link path: emit the user crate's `.rcgu.o`
            // files, synthesize a stub object whose entries jump
            // to the running host bin's addresses, and link a
            // minimal patch dylib with no rlib inputs and no
            // dynamic dep on framework crates. The resulting
            // patch is tens of kilobytes and dlopens in <30 ms.
            cwd: None,
        }},
        watch_paths: vec![PathBuf::from("{user_src}")],
        debounce: std::time::Duration::from_millis(100),
        before_exec: None,
        on_success: Some(Box::new(move || {{
            if let Some(p) = patch_path() {{
                eprintln!("[dylib-host] new patch ready → {{}}", p.display());
                if let Ok(mut g) = pending_for_watch.lock() {{
                    *g = Some(p);
                }}
            }} else {{
                eprintln!("[dylib-host] patch build succeeded but dylib not found");
            }}
        }})),
    }});

    let port_mirror: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));

    // Sentinel-file writer for the CLI parent (same protocol as the
    // sidecar host).
    if let Ok(path) = std::env::var("IDEALYST_AAS_PORT_FILE") {{
        let port_for_file = port_mirror.clone();
        std::thread::spawn(move || {{
            for _ in 0..200 {{
                if let Ok(g) = port_for_file.lock() {{
                    if let Some(p) = *g {{
                        let _ = std::fs::write(&path, p.to_string());
                        return;
                    }}
                }}
                std::thread::sleep(std::time::Duration::from_millis(50));
            }}
        }});
    }}

    eprintln!("[dylib-host] starting (advertising app_id={{}} via mDNS)", APP_ID);
    let pending_for_tick = pending_patch.clone();
    let owner_for_tick = owner.clone();
    let recorder_for_tick = recorder.clone();
    let backend_for_tick = backend_rc.clone();
    serve_with_tick_and_port(
        addr,
        recorder,
        APP_ID,
        move || {{
            // Drain the pending-patch slot. Re-take under the lock so
            // a watcher firing again mid-apply doesn't lose its
            // update — the lock is held briefly and the actual
            // dlopen/diff happens after release.
            let maybe_path = {{
                let mut g = pending_for_tick.lock().ok();
                g.as_mut().and_then(|g| g.take())
            }};
            if let Some(p) = maybe_path {{
                let t_tick = std::time::Instant::now();
                // dyld caches dlopen by absolute path on macOS: a
                // second dlopen of the SAME path returns the cached
                // image even if the file on disk was rewritten in
                // between. Copy the freshly-built patch to a unique
                // filename so subsecond's dlopen forces a fresh load
                // every time. Without this, every patch after the
                // first one would silently no-op (the jump table
                // entry stays mapped to the first patch's body).
                static APPLY_SEQ: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let seq = APPLY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let unique = p.with_file_name(format!(
                    "libpatch-apply-{{}}.dylib",
                    seq
                ));
                let t_copy_start = std::time::Instant::now();
                if let Err(e) = std::fs::copy(&p, &unique) {{
                    eprintln!(
                        "[dylib-host] failed to stage patch as {{}}: {{}}",
                        unique.display(),
                        e
                    );
                    return;
                }}
                let copy_ms = t_copy_start.elapsed().as_millis();
                let t_apply_start = std::time::Instant::now();
                match unsafe {{ framework_hot::diff::apply_from_dylib(&unique) }} {{
                    Ok(_) => {{}}
                    Err(e) => {{
                        eprintln!("[dylib-host] patch apply failed: {{:?}}", e);
                        return;
                    }}
                }}
                let apply_ms = t_apply_start.elapsed().as_millis();

                // Apply succeeded — the jump table now points every
                // `__*_hot_impl` call site at the patch dylib's
                // freshly-compiled bodies. Re-render so structural
                // changes (added/removed nodes, changed initial
                // values, anything outside an Effect's dep set)
                // propagate to connected clients. Sequence:
                //   1. drop the old `Owner` → tears down every scope
                //      + signal the previous walk created, releasing
                //      backend resources via the regular destructor
                //      path.
                //   2. reset the recorder's log + scene model →
                //      fresh `NodeId` counter and an empty scene
                //      snapshot so reconnecting clients start clean.
                //   3. `render(backend, app())` → walks the patched
                //      `app()` tree, emitting a fresh `Commands`
                //      stream.
                //   4. broadcast a wholesale snapshot to every
                //      already-connected client so their local
                //      backends rebuild the new tree.
                // The whole sequence runs on the serve thread (same
                // thread that owns the reactive runtime) — no
                // cross-thread footguns.
                let t_render_start = std::time::Instant::now();
                {{
                    let mut slot = owner_for_tick.borrow_mut();
                    *slot = None;
                }}
                recorder_for_tick.reset_log_and_scene();
                let new_owner = render(backend_for_tick.clone(), app());
                *owner_for_tick.borrow_mut() = Some(new_owner);
                let render_ms = t_render_start.elapsed().as_millis();
                let tick_ms = t_tick.elapsed().as_millis();
                eprintln!(
                    "[dylib-host] timing: copy {{}}ms apply {{}}ms render {{}}ms (tick total {{}}ms)",
                    copy_ms, apply_ms, render_ms, tick_ms
                );
                eprintln!(
                    "[dylib-host] re-rendered ({{}} commands)",
                    recorder_for_tick.command_count()
                );
            }}
        }},
        Some(port_mirror),
        None,
    )
}}

/// Locate the patch dylib in the workspace target dir. The fast
/// rebuild path runs rustc directly with `--out-dir target/debug/
/// deps/`, so the freshly-built dylib lands at `deps/libpatch.
/// dylib`. (Cargo's normal pipeline ALSO hardlinks it to `target/
/// debug/libpatch.dylib`, but the direct-rustc replay skips that
/// step.) Both paths are checked; the most recently-modified one
/// wins so we don't dispatch a stale build.
fn patch_path() -> Option<PathBuf> {{
    let target = PathBuf::from(PATCH_WORKSPACE_TARGET);
    let canonical = target.join(PATCH_PROFILE_DIR).join("libpatch.dylib");
    let deps_canonical = target
        .join(PATCH_PROFILE_DIR)
        .join("deps")
        .join("libpatch.dylib");
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for p in [&canonical, &deps_canonical] {{
        if let Ok(mt) = std::fs::metadata(p).and_then(|m| m.modified()) {{
            match &newest {{
                Some((_, t)) if *t >= mt => {{}}
                _ => newest = Some((p.clone(), mt)),
            }}
        }}
    }}
    if newest.is_some() {{
        return newest.map(|(p, _)| p);
    }}
    // Fallback: scan deps/ for hashed dylibs (older toolchains).
    let deps = target.join(PATCH_PROFILE_DIR).join("deps");
    if let Ok(read) = std::fs::read_dir(&deps) {{
        for entry in read.flatten() {{
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("libpatch-") && name.ends_with(".dylib") {{
                let mt = entry.metadata().and_then(|m| m.modified()).ok()?;
                match &newest {{
                    Some((_, t)) if *t >= mt => {{}}
                    _ => newest = Some((p, mt)),
                }}
            }}
        }}
        return newest.map(|(p, _)| p);
    }}
    None
}}
"#,
        lib = manifest.lib_name,
        default_addr = DEFAULT_BIND_ADDR,
        app_id = app_id,
        profile_dir = "debug",
        workspace_target = workspace_target.display(),
        mode_dir = mode_dir.display(),
        user_src = user_src.display(),
        rustflags = rustflags,
        idealyst_bin = idealyst_bin.display(),
    ))
}

// ---------------------------------------------------------------------------
// Build invocation
// ---------------------------------------------------------------------------

/// Run `cargo build -p <pkg>` inside the sub-workspace. RUSTFLAGS is
/// set so dependents that have both an rlib and a dylib variant
/// (notably `framework-core`) resolve to the dylib — that's how the
/// host bin and the patch end up sharing the same `libframework_core
/// .dylib` image at runtime, which is what makes TLV opcodes
/// (thread-local-variable access) consistent across both code paths.
fn cargo_build_both(mode_dir: &Path, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "-p", "host", "-p", "patch"])
        .current_dir(mode_dir);
    if release {
        cmd.arg("--release");
    }
    cmd.env("RUSTFLAGS", rustflags_for_dylib_mode()?);

    // Wire the rustc-capture wrapper so we save the exact argv cargo
    // uses for each crate. The watcher's fast-rebuild path replays
    // these directly (bypassing cargo entirely) for ~3x faster
    // hot-reload cycles.
    //
    // We DON'T clear the capture dir between runs: cargo only
    // invokes rustc for crates it considers stale, so a re-run
    // against a warm target dir would leave gaps for the cached
    // crates. The captures are upsert — each rustc invocation
    // overwrites its own `.json` if it actually runs, otherwise
    // the previous capture (from when that crate WAS compiled) is
    // still valid.
    let capture_dir = mode_dir.join(".rustc-args");
    std::fs::create_dir_all(&capture_dir)
        .with_context(|| format!("create rustc-capture dir {}", capture_dir.display()))?;
    let cli_binary = std::env::current_exe()
        .context("locate the idealyst CLI binary path for RUSTC_WRAPPER")?;
    cmd.env("RUSTC_WRAPPER", &cli_binary);
    cmd.env("IDEALYST_RUSTC_CAPTURE_DIR", &capture_dir);
    // Cargo invokes the wrapper as `<wrapper> <real-rustc> <args>`.
    // Our CLI's `rustc-capture` subcommand consumes that shape — but
    // since we set RUSTC_WRAPPER to the bare CLI path, cargo's
    // invocation looks like `<cli-path> <rustc> <args>`. We need
    // the CLI to dispatch to the `rustc-capture` subcommand on
    // entry. Easiest: set the env so the CLI knows to behave as a
    // wrapper, and check that env early in `main`. See
    // `crates/cli/src/main.rs` — the `IDEALYST_RUSTC_CAPTURE_DIR`
    // env var IS the signal.
    //
    // (Alternative: write a tiny shell-script shim. Avoided to keep
    // the build self-contained.)

    eprintln!(
        "[build-aas:dylib] cargo build -p host -p patch{} (in {})",
        if release { " --release" } else { "" },
        mode_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-aas:dylib] cargo build exited with {status}");
    }
    Ok(())
}

/// Patch-only rebuild path. After the initial `cargo_build_both`
/// produced consistent host + patch fingerprints, every subsequent
/// rebuild is just the patch (the host is long-lived and doesn't
/// re-link on edits). Same RUSTFLAGS so the framework-core
/// fingerprint stays put.
#[allow(dead_code)] // wired up by the host's generated rebuild loop;
                    // kept here so the build code owns the flag set.
fn cargo_build_patch_only(mode_dir: &Path, release: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "-p", "patch"]).current_dir(mode_dir);
    if release {
        cmd.arg("--release");
    }
    cmd.env("RUSTFLAGS", rustflags_for_dylib_mode()?);
    let status = cmd.status().context("spawn cargo")?;
    if !status.success() {
        anyhow::bail!("patch rebuild failed: {status}");
    }
    Ok(())
}

/// Parse the host bin's symbol table and write it to
/// `host-symbols.json` so subsequent patch links can resolve
/// undefined refs against absolute addresses. The structure
/// mirrors `cli::cmd::link_patch::HostSymbolTable`; we duplicate
/// the schema here rather than carve out a shared crate because
/// both ends are leaf consumers and the shape is two fields.
fn capture_host_symbols(host_bin: &Path, out: &Path) -> Result<()> {
    use serde::Serialize;
    #[derive(Serialize, Default)]
    struct HostSymbolTable {
        symbols: HashMap<String, u64>,
        main_addr: u64,
    }
    let data = std::fs::read(host_bin)
        .with_context(|| format!("read host bin {}", host_bin.display()))?;
    let obj = object::File::parse(&*data)
        .with_context(|| format!("parse host bin {}", host_bin.display()))?;
    let mut table = HostSymbolTable::default();
    for sym in obj.symbols() {
        let Ok(name) = sym.name() else { continue };
        if name.is_empty() {
            continue;
        }
        let addr = sym.address();
        if addr == 0 {
            // Undefined / external — useless for resolving stubs.
            continue;
        }
        // Last write wins for duplicate names. Rust rarely emits
        // exact dupes; if it does they're aliases pointing at the
        // same address anyway.
        table.symbols.insert(name.to_string(), addr);
        if name == "_main" || name == "main" {
            table.main_addr = addr;
        }
    }
    if table.main_addr == 0 {
        anyhow::bail!(
            "host bin {} has no `_main`/`main` symbol — was it linked as a bin crate?",
            host_bin.display()
        );
    }
    let json = serde_json::to_string_pretty(&table)?;
    std::fs::write(out, json)
        .with_context(|| format!("write host-symbols.json to {}", out.display()))?;
    eprintln!(
        "[build-aas:dylib] captured {} host symbols → {}",
        table.symbols.len(),
        out.display(),
    );
    Ok(())
}

/// RUSTFLAGS used for both the host and the patch builds. Three
/// concerns are folded in:
///
/// 1. `-C prefer-dynamic` — rustc picks the `.dylib` variant of any
///    dep that ships both rlib and dylib (`framework-core` is the
///    one we care about). The host bin and the patch dylib therefore
///    both reference the same `libframework_core.dylib` at runtime,
///    so thread-local storage, statics, and singletons stay
///    consistent across both code paths.
///
/// 2. `-C link-arg=-Wl,-rpath,<rust-sysroot-lib>` — the dylib
///    `libframework_core.dylib` links against `libstd-<hash>.dylib`
///    in the rust toolchain's lib dir. By default the host bin has
///    no `LC_RPATH`, so dyld fails to find `libstd`. Adding the rust
///    sysroot to the rpath fixes the dependency resolution at
///    runtime without forcing every dev to set `DYLD_LIBRARY_PATH`.
///
/// 3. `-Wl,-export_dynamic` is NOT needed here because the patch
///    resolves its framework references against `libframework_core
///    .dylib` (a separate dyld image), not against the host bin's
///    private symbols. We pay nothing for the simpler symbol surface.
fn rustflags_for_dylib_mode() -> Result<String> {
    // ARCHITECTURE: dx-style "fat bin + thin patch" hot reload.
    //
    // We deliberately statically link every framework crate into
    // the host bin (no `-C prefer-dynamic`) — that's what makes
    // every framework symbol live inside the bin's text section,
    // visible to the bin's dynamic-symbol export table. The patch
    // dylib resolves its references against those addresses via
    // synthesized stubs (see `cli::cmd::link_patch`).
    //
    // `-Wl,-export_dynamic` is the key link flag: it expands the
    // bin's export table to include every internal symbol so a
    // separate process (the patch linker) can read them out of
    // the bin file. Without this, only `main` and the
    // user-declared `pub extern "C"` symbols would be exported.
    let libdir = rust_lib_dir()?;
    Ok(format!(
        "-C link-arg=-Wl,-rpath,{libdir} -C link-arg=-Wl,-export_dynamic"
    ))
}

fn rust_lib_dir() -> Result<String> {
    // Resolve the host's libstd dylib directory.
    //
    // It lives at `<sysroot>/lib/rustlib/<target-triple>/lib/`, NOT
    // at `<sysroot>/lib/` — the latter is the rustlib metadata dir,
    // empty of the actual `.dylib` shipped per target. Without this
    // rpath the host bin aborts at startup with `dyld: Library not
    // loaded: @rpath/libstd-<hash>.dylib`.
    let sysroot = Command::new("rustc")
        .arg("--print=sysroot")
        .output()
        .context("invoke `rustc --print=sysroot` to locate libstd dylib")?;
    if !sysroot.status.success() {
        anyhow::bail!("rustc --print=sysroot failed: {}", sysroot.status);
    }
    let sysroot = String::from_utf8(sysroot.stdout)
        .context("rustc --print=sysroot output is not utf-8")?
        .trim()
        .to_string();
    let host_triple = Command::new("rustc")
        .args(["-vV"])
        .output()
        .context("invoke `rustc -vV` to discover host triple")?;
    let host_triple = String::from_utf8(host_triple.stdout)
        .context("rustc -vV output is not utf-8")?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .context("rustc -vV did not print a `host:` line")?;
    Ok(format!("{}/lib/rustlib/{}/lib", sysroot, host_triple))
}

// ---------------------------------------------------------------------------
// Artifact lookup
// ---------------------------------------------------------------------------

/// Locate the freshly-built patch dylib. Same convention as the
/// host's runtime `patch_path()` (un-hashed canonical name first,
/// hashed `deps/` entry as fallback) — kept in sync.
fn find_patch_dylib(workspace_target: &Path, profile: &str) -> Result<PathBuf> {
    let canonical = workspace_target.join(profile).join("libpatch.dylib");
    if canonical.is_file() {
        return Ok(canonical);
    }
    let deps = workspace_target.join(profile).join("deps");
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    let read = fs::read_dir(&deps).with_context(|| {
        format!("read deps dir {} to locate patch dylib", deps.display())
    })?;
    for entry in read.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with("libpatch-") && name.ends_with(".dylib") {
            let mt = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            match &newest {
                Some((_, t)) if *t >= mt => {}
                _ => newest = Some((path, mt)),
            }
        }
    }
    newest.map(|(p, _)| p).with_context(|| {
        format!(
            "patch dylib not produced; expected {} or {}/libpatch-*.dylib",
            canonical.display(),
            deps.display()
        )
    })
}
