//! AAS dev-host build orchestration for `idealyst build aas`.
//!
//! AAS (Application-as-a-Server) runs the user's reactive runtime on
//! a dev-host process and lets browsers / native shells connect as
//! thin clients that ship primitive commands over a WebSocket.
//!
//! ## Split-process architecture
//!
//! The AAS dev host is **two** binaries, generated side by side under
//! `<workspace>/target/idealyst/<project>/aas/`:
//!
//! - `host/`   → `<project>-aas-host`  — long-lived infra (WebSocket
//!                                       server, mDNS, file watcher).
//!                                       Statically links `dev-server`
//!                                       but NOT the user crate.
//! - `app/`    → `<project>-aas-app`   — short-lived sidecar that
//!                                       statically links the user
//!                                       crate, runs `render(app())`,
//!                                       and streams wire commands /
//!                                       reads events over its
//!                                       stdout / stdin pipes.
//!
//! On file change the host rebuilds the **sidecar** (NOT itself),
//! SIGKILLs the running sidecar, and respawns. The WebSocket listener
//! stays up the entire time — connected clients (Android, iOS) never
//! disconnect, so the perceived hot-reload latency drops to roughly
//! the user-crate rebuild + sidecar startup time. This is the win
//! over the legacy single-process model where every save did a
//! self-exec and forced every client to reconnect.
//!
//! ## Layout
//!
//! ```text
//! <workspace>/target/idealyst/<project>/aas/
//! ├── host/
//! │   ├── Cargo.toml
//! │   ├── .cargo/config.toml      ← share workspace target dir
//! │   └── src/main.rs             ← spawns sidecar + serves
//! └── app/
//!     ├── Cargo.toml
//!     ├── .cargo/config.toml      ← share workspace target dir
//!     └── src/main.rs             ← runs render(app()) + frame I/O
//! ```
//!
//! Both crates are regenerated on every build. `idealyst scaffold
//! aas` (future) will materialize editable copies into the repo for
//! users who want to customize either side.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{parse_manifest, require_workspace_root, Manifest};

pub mod hotpatch;

#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug. The host and sidecar
    /// both run locally — release is almost never worth the slower
    /// rebuild cycle here.
    pub release: bool,
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced infra-host executable.
    pub host_binary: PathBuf,
    /// Path to the produced sidecar executable.
    pub sidecar_binary: PathBuf,
    /// Host wrapper crate directory.
    pub wrapper_dir: PathBuf,
    /// Sidecar wrapper crate directory.
    pub sidecar_dir: PathBuf,
}

/// Bind address the generated host uses unless overridden via its
/// first CLI arg. `0.0.0.0:0` means "OS picks a free port and listen
/// on every interface" — the chosen port is published over mDNS so
/// clients (iOS, Android, …) discover the host without anyone having
/// to coordinate a fixed number.
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:0";

pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let manifest = parse_manifest(&project_dir)?;
    // AAS mode reaches into `<workspace>/target/` for the host's
    // hot-patch builder + sidecar binary lookups, and statically
    // links the in-workspace `build-aas` crate into the generated
    // host. None of that is reachable through git, so AAS strictly
    // requires the framework workspace on disk.
    let workspace_root = require_workspace_root(&project_dir)?;

    build_sidecar_mode(&project_dir, &workspace_root, &manifest, &opts)
}

fn build_sidecar_mode(
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<BuildArtifact> {
    let wrapper_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("aas/host");
    let sidecar_dir = workspace_root
        .join("target/idealyst")
        .join(&manifest.name)
        .join("aas/app");
    let workspace_target = workspace_root.join("target");

    generate_sidecar_wrapper(&sidecar_dir, project_dir, workspace_root, manifest)?;
    generate_host_wrapper(
        &wrapper_dir,
        &sidecar_dir,
        project_dir,
        workspace_root,
        manifest,
    )?;

    // Captures dir for the sidecar's fat build. The hot-patch
    // builder reads `<dir>/<crate>.<crate-type>.json` per crate
    // when re-emitting the user crate's .rcgu.o files.
    let captures_dir = wrapper_dir
        .parent()
        .map(|p| p.join("captures"))
        .unwrap_or_else(|| sidecar_dir.join("captures"));
    fs::create_dir_all(&captures_dir)
        .with_context(|| format!("create captures dir {}", captures_dir.display()))?;
    // Same parent layout for the per-edit patch dylibs.
    let patches_dir = wrapper_dir
        .parent()
        .map(|p| p.join("patches"))
        .unwrap_or_else(|| sidecar_dir.join("patches"));
    fs::create_dir_all(&patches_dir)
        .with_context(|| format!("create patches dir {}", patches_dir.display()))?;

    // Force the user crate to recompile through the wrapper so we
    // get a fresh capture. Otherwise — if the user crate is already
    // cached from a prior workspace build — cargo skips rustc for
    // it and our wrapper never fires for the very crate we need to
    // replay on hot-patch.
    //
    // This costs one extra ~300ms rebuild per `idealyst dev` start,
    // which is a one-time price; subsequent file-change cycles use
    // the cached capture directly.
    let user_lib = project_dir.join("src/lib.rs");
    if user_lib.exists() {
        let now = std::time::SystemTime::now();
        let _ = filetime_set(&user_lib, now);
    }

    // Build the sidecar first so the host (which spawns it on
    // startup) finds the binary present. The sidecar's build is
    // the "fat" build — RUSTC_WRAPPER captures each member's rustc
    // invocation; -Csave-temps + -Clink-dead-code keep .rcgu.o
    // files on disk + every symbol in the bin's text section so
    // the per-edit patch link can resolve them.
    cargo_build_fat(&sidecar_dir, opts.release, "sidecar", &captures_dir)?;
    cargo_build(&wrapper_dir, opts.release, "host")?;

    let profile = if opts.release { "release" } else { "debug" };
    let host_bin_name = host_binary_name(&manifest.name);
    let sidecar_bin_name = sidecar_binary_name(&manifest.name);

    let host_binary = workspace_target.join(profile).join(&host_bin_name);
    let sidecar_binary = workspace_target.join(profile).join(&sidecar_bin_name);

    for (label, path) in [
        ("host", &host_binary),
        ("sidecar", &sidecar_binary),
    ] {
        if !path.is_file() {
            anyhow::bail!(
                "cargo build reported success but {label} binary not at {}",
                path.display(),
            );
        }
    }

    Ok(BuildArtifact {
        host_binary,
        sidecar_binary,
        wrapper_dir,
        sidecar_dir,
    })
}

/// Update a file's mtime to `t`. Wrapper around stdlib utime
/// because we don't want to pull in `filetime` for one call.
/// Returns Err if the path doesn't exist; caller can ignore.
#[cfg(unix)]
fn filetime_set(path: &Path, t: std::time::SystemTime) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    // Easiest portable way: open + close. The OS bumps mtime on
    // metadata-write via futimens, but we'd need libc bindings.
    // Re-write the contents byte-for-byte instead — it's a hot path
    // only on dev-host startup, ~1ms.
    let data = std::fs::read(path)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(false)
        .custom_flags(0)
        .open(path)?;
    use std::io::Write;
    f.write_all(&data)?;
    let _ = t; // mtime gets bumped automatically by the OS on write
    Ok(())
}

#[cfg(not(unix))]
fn filetime_set(path: &Path, _t: std::time::SystemTime) -> std::io::Result<()> {
    let data = std::fs::read(path)?;
    std::fs::write(path, data)
}

fn host_binary_name(project_name: &str) -> String {
    format!("{project_name}-aas-host")
}

fn sidecar_binary_name(project_name: &str) -> String {
    format!("{project_name}-aas-app")
}

// ---------------------------------------------------------------------------
// Sidecar wrapper generation
// ---------------------------------------------------------------------------

fn generate_sidecar_wrapper(
    sidecar_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(sidecar_dir.join("src"))
        .with_context(|| format!("create {}", sidecar_dir.display()))?;

    let sidecar_name = sidecar_binary_name(&manifest.name);
    let fcore = workspace_root.join("crates/framework/core");
    let fhot = workspace_root.join("crates/framework/hot");
    let dev_server = workspace_root.join("crates/dev/server");
    let wire = workspace_root.join("crates/framework/wire");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Sidecar binary: statically links the user's crate, runs the
# reactive runtime, and streams wire commands / reads events over
# stdout / stdin. The infra host (`<name>-aas-host`) spawns this as a
# child process; subsecond hot-patches its component bodies on every
# source change without process restart, falling back to respawn on
# any failure.

[workspace]

[package]
name = "{sidecar_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
# `hot-reload` is what flips the `#[component]` macro into its split
# form (__<Name>_hot_impl + outer dispatch via framework_hot::call).
# Without this feature, subsecond's jump table never gets consulted.
framework-core = {{ path = "{fcore}", features = ["hot-reload"] }}
# `hot` is subsecond's runtime; `diff` pulls in the symbol-diff
# generator the sidecar uses to verify patches before applying.
framework-hot = {{ path = "{fhot}", features = ["hot", "diff"] }}
dev-server = {{ path = "{dev_server}" }}
wire = {{ path = "{wire}" }}
{user_name} = {{ path = "{user_path}" }}
# JumpTable + AddressMap, deserialized from ApplyPatch frames sent
# by the host. The host owns construction; the sidecar just
# deserializes + applies.
subsecond-types = "0.7"
# libc::dlsym for the C-ABI `main` runtime address that we ship in
# the Hello frame so the host can compute the ASLR slide.
libc = "0.2"
serde_json = "1"

# Sidecar is short-lived dev infra — strip everything that costs
# link time. debug = 0 cuts ~half the link work; the patch dylib's
# stub generator doesn't care about debug info.
[profile.dev]
debug = 0
strip = "debuginfo"
"#,
        sidecar_name = sidecar_name,
        fcore = fcore.display(),
        fhot = fhot.display(),
        dev_server = dev_server.display(),
        wire = wire.display(),
        user_name = manifest.name,
        user_path = project_dir.display(),
    );

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build aas`. Sidecar binary for the
//! split-process AAS dev host.
//!
//! Statically links `{lib}::app()` and runs the framework's reactive
//! runtime against a `dev-server::WireRecordingBackend`. Wire commands
//! flow OUT over stdout (length-prefixed JSON frames of
//! `SidecarOut`); app→dev events arrive IN over stdin
//! (`SidecarIn` frames) and dispatch through the local recorder.
//!
//! Lifecycle: spawned by the host on startup and re-spawned on every
//! source change. Exits when stdin closes (host died / sidecar is
//! being recycled).

use std::cell::RefCell;
use std::io::{{stdin, stdout, BufReader, Write}};
use std::rc::Rc;

use dev_server::sidecar::{{is_eof, read_frame, write_frame, SidecarIn, SidecarOut}};
use dev_server::WireRecordingBackend;
use framework_core::{{render, Owner}};
use {lib}::app;

fn main() -> std::io::Result<()> {{
    // Report our `main` runtime address before anything else. The
    // host uses this to compute the ASLR slide for the symbol-table
    // diff in hot-patch builds. Doing it first keeps the host's
    // hotpatch builder usable from the very first file-change event.
    let main_addr: u64 = unsafe {{
        libc::dlsym(libc::RTLD_DEFAULT, b"main\0".as_ptr() as *const _) as u64
    }};
    let mut out = stdout();
    write_frame(&mut out, &SidecarOut::Hello {{ aslr_reference: main_addr }})?;
    let _ = out.flush();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Drive the user's tree through the real walker once at startup.
    // The recorder accumulates wire commands into its append-only
    // log; we drain that log into a single initial `Commands` frame
    // for the host. Owner is kept in a RefCell so the hot-patch
    // path can replace it with a fresh `render(...)` to propagate
    // structural changes.
    let owner_cell: Rc<RefCell<Option<Owner>>> =
        Rc::new(RefCell::new(Some(render(backend_rc.clone(), app()))));

    let initial = recorder.snapshot();
    let mut outbound_cursor = recorder.command_count();
    write_frame(&mut out, &SidecarOut::Commands(initial))?;
    let _ = out.flush();

    // Main loop: wait for an event from the host, dispatch it (which
    // may fire signals → walker → more recorder commands), then ship
    // any new commands back. Blocks on stdin between events.
    let mut input = BufReader::new(stdin());
    loop {{
        let msg: SidecarIn = match read_frame(&mut input) {{
            Ok(f) => f,
            Err(e) if is_eof(&e) => {{
                eprintln!("[aas-app] host pipe closed; exiting");
                return Ok(());
            }}
            Err(e) => {{
                eprintln!("[aas-app] frame read error: {{e}} — exiting");
                return Err(e);
            }}
        }};

        match msg {{
            SidecarIn::Event(app_to_dev) => dispatch_app_to_dev(&recorder, app_to_dev),
            SidecarIn::ApplyPatch {{ table_json }} => {{
                // Parse → apply → re-render. Failure is logged but
                // doesn't terminate the sidecar; the host will fall
                // back to respawn if the patch was wrong, which
                // kills us cleanly anyway.
                match serde_json::from_str::<subsecond_types::JumpTable>(&table_json) {{
                    Ok(table) => {{
                        eprintln!(
                            "[aas-app] applying patch ({{}} jump-table entries)",
                            table.map.len(),
                        );
                        match unsafe {{ framework_hot::apply_patch(table) }} {{
                            Ok(()) => {{
                                // Tear down old reactive tree + re-render
                                // so structural changes propagate. The
                                // recorder's log resets too — clients
                                // catch up from a fresh snapshot.
                                {{
                                    let mut slot = owner_cell.borrow_mut();
                                    *slot = None;
                                }}
                                recorder.reset_log_and_scene();
                                let new_owner = render(backend_rc.clone(), app());
                                *owner_cell.borrow_mut() = Some(new_owner);
                                outbound_cursor = 0;
                                eprintln!("[aas-app] patch applied + re-rendered");
                            }}
                            Err(e) => {{
                                eprintln!("[aas-app] apply_patch failed: {{:?}}", e);
                            }}
                        }}
                    }}
                    Err(e) => {{
                        eprintln!("[aas-app] failed to parse JumpTable JSON: {{}}", e);
                    }}
                }}
            }}
        }}

        // Drain any commands the event produced. `commands_since` is
        // append-only relative to the recorder's log, so the cursor
        // advances by exactly the number of new commands.
        let count_now = recorder.command_count();
        if count_now > outbound_cursor {{
            let new_cmds = recorder.commands_since(outbound_cursor);
            outbound_cursor = count_now;
            write_frame(&mut out, &SidecarOut::Commands(new_cmds))?;
            let _ = out.flush();
        }}
    }}
}}

/// Mirror of the legacy `handle_app_msg` in `dev-server::transport`.
/// The split moves this logic into the sidecar because the recorder
/// here is the one with registered handler closures — the host's
/// recorder is purely a transport mirror.
fn dispatch_app_to_dev(recorder: &WireRecordingBackend, msg: wire::AppToDev) {{
    use wire::AppToDev::*;
    match msg {{
        Hello {{ .. }} => {{}}
        Event {{ handler, args }} => {{
            let _ = recorder.dispatch_event(handler, args);
        }}
        StateChanged {{ node, bit, on }} => {{
            let _ = recorder.dispatch_state(node, bit, on);
        }}
        ColorSchemeChanged {{ scheme: _ }} => {{}}
        ScreenReleased {{ scope }} => {{
            recorder.handle_screen_released(scope.0);
        }}
        NavigatorDepthChanged {{ .. }} => {{}}
        DrawerStateChanged {{ navigator, is_open }} => {{
            recorder.handle_drawer_state_changed(navigator, is_open);
        }}
        TabSelected {{ navigator, index }} => {{
            recorder.handle_tab_selected(navigator, index);
        }}
        VirtualizerMountItem {{ .. }}
        | VirtualizerReleaseItem {{ .. }}
        | VirtualizerMeasuredSize {{ .. }} => {{}}
        Error {{ message }} => {{
            eprintln!("[aas-app] client reported error: {{}}", message);
        }}
    }}
}}
"#,
        lib = manifest.lib_name,
    );

    write_shared_target_config(sidecar_dir, workspace_root)?;
    fs::write(sidecar_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(sidecar_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Host wrapper generation
// ---------------------------------------------------------------------------

fn generate_host_wrapper(
    wrapper_dir: &Path,
    sidecar_dir: &Path,
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = host_binary_name(&manifest.name);
    let dev_server = workspace_root.join("crates/dev/server");
    let build_aas = workspace_root.join("crates/build/aas");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Infra-only AAS host: WebSocket server, mDNS, file watcher, hot-patch
# builder. Does NOT link the user's crate — that lives in the sibling
# `aas/app` sidecar binary, which this host spawns as a child process
# and either hot-patches via subsecond (preferred) or SIGKILLs +
# respawns (fallback when patching can't apply).

[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
dev-server = {{ path = "{dev_server}" }}
# Owns the hot-patch builder: captured-rustc replay, stub-object
# synthesis, dylib link, jump-table construction. The host owns
# this work because the sidecar shouldn't take a build-tools dep.
build-aas = {{ path = "{build_aas}" }}
# Used by the host's try_hotpatch / respawn fallback ladder for
# ergonomic error contexts. The hotpatch builder itself returns
# anyhow::Error.
anyhow = "1"
serde_json = "1"
"#,
        wrapper_name = wrapper_name,
        dev_server = dev_server.display(),
        build_aas = build_aas.display(),
    );

    let profile_dir = "debug"; // Mirror what the sidecar lands in. The
                               // host doesn't currently support
                               // release mode through this template.
    let sidecar_bin = workspace_root
        .join("target")
        .join(profile_dir)
        .join(sidecar_binary_name(&manifest.name));
    let sidecar_manifest = sidecar_dir.join("Cargo.toml");
    let workspace_target = workspace_root.join("target");
    let user_src = project_dir.join("src");

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build aas`. Long-lived infra host for
//! the split-process AAS dev server.
//!
//! Owns: WebSocket listener (mDNS-advertised), file watcher, sidecar
//! child, and the hot-patch builder. The user's reactive runtime
//! lives in the sibling sidecar (`{sidecar_name}`); on each source
//! change the host tries to hot-patch the running sidecar via
//! subsecond, and only falls back to SIGKILL+respawn if hot-patch
//! fails (build error, unresolved symbol, dlopen failure, …).
//! Either way the WebSocket listener stays up the entire time so
//! connected clients (web, Android, iOS) never see a disconnect.

use std::path::PathBuf;
use std::sync::{{Arc, Mutex}};

use build_aas::hotpatch::HotPatchBuilder;
use dev_server::sidecar::SidecarIn;
use dev_server::{{
    serve_with_sidecar, spawn_change_loop, Sidecar, SidecarSlot, WireRecordingBackend,
}};

const DEFAULT_ADDR: &str = "{default_addr}";
/// mDNS-published app identifier.
const APP_ID: &str = "{app_id}";
/// Absolute path to the sidecar binary the host spawns on startup.
const SIDECAR_BINARY: &str = "{sidecar_bin}";
/// Captured-rustc args directory written by the initial fat build.
/// Read by the hot-patch builder on every source change.
const CAPTURES_DIR: &str = "{captures_dir}";
/// The user crate name — the "tip" of the hot-patch (its rustc
/// invocation gets replayed with `--emit=obj` to produce the patch
/// dylib's body).
const USER_CRATE: &str = "{user_crate}";
/// Where the hot-patch builder drops `libpatch-*.dylib` per cycle.
const PATCH_TARGET_DIR: &str = "{patch_target_dir}";

fn main() -> std::io::Result<()> {{
    let addr = if let Some(a) = std::env::args().nth(1) {{
        a
    }} else if let Ok(p) = std::env::var("IDEALYST_AAS_BIND_PORT") {{
        format!("0.0.0.0:{{}}", p)
    }} else {{
        DEFAULT_ADDR.to_string()
    }};

    let recorder = WireRecordingBackend::new();
    let sidecar_slot: SidecarSlot = Arc::new(Mutex::new(None));

    // Spawn the initial sidecar.
    let sidecar_path = std::path::PathBuf::from(SIDECAR_BINARY);
    match Sidecar::spawn(&sidecar_path) {{
        Ok(s) => {{
            *sidecar_slot.lock().unwrap() = Some(s);
            eprintln!("[aas-host] sidecar spawned: {{}}", sidecar_path.display());
        }}
        Err(e) => {{
            eprintln!(
                "[aas-host] sidecar spawn failed: {{e}} — host running idle (no UI will render)"
            );
        }}
    }}

    // Construct the hot-patch builder once. It parses the sidecar
    // bin's symbol table (~50ms) and caches it; per-rebuild work is
    // just the rustc replay + stub-object link.
    let hotpatch = match HotPatchBuilder::new(
        PathBuf::from(CAPTURES_DIR),
        &PathBuf::from(SIDECAR_BINARY),
        PathBuf::from(PATCH_TARGET_DIR),
    ) {{
        Ok(b) => Some(Arc::new(b)),
        Err(e) => {{
            eprintln!(
                "[aas-host] hot-patch builder init failed: {{e:#}} — falling back to \
                 respawn on every change"
            );
            None
        }}
    }};

    // File watcher: on debounced change, try hot-patch first;
    // respawn on failure. The hot-patch path doesn't drop the
    // sidecar process — components swap in place under
    // subsecond — so client connections survive even cleaner than
    // the respawn path.
    let sidecar_for_rebuild = sidecar_slot.clone();
    let hotpatch_for_rebuild = hotpatch.clone();
    let user_src = PathBuf::from("{user_src}");
    spawn_change_loop(
        vec![user_src],
        std::time::Duration::from_millis(100),
        Box::new(move || {{
            let t_total = std::time::Instant::now();
            // `IDEALYST_AAS_NO_HOTPATCH=1` forces every change
            // through the respawn fallback — used by the scaling
            // benchmark to measure the "without hot-patch" path
            // against a controlled fixture.
            let force_respawn = std::env::var("IDEALYST_AAS_NO_HOTPATCH")
                .ok()
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
            if force_respawn {{
                respawn_sidecar(&sidecar_for_rebuild);
                eprintln!(
                    "[aas-host] respawn applied in {{}}ms (force_respawn)",
                    t_total.elapsed().as_millis()
                );
                return;
            }}
            if let Err(e) = try_hotpatch(&hotpatch_for_rebuild, &sidecar_for_rebuild) {{
                eprintln!("[aas-host] hot-patch failed: {{e:#}} — respawning sidecar");
                respawn_sidecar(&sidecar_for_rebuild);
                eprintln!(
                    "[aas-host] respawn applied in {{}}ms (after hot-patch failure)",
                    t_total.elapsed().as_millis()
                );
            }} else {{
                eprintln!(
                    "[aas-host] hot-patch applied in {{}}ms",
                    t_total.elapsed().as_millis()
                );
            }}
        }}),
    );

    let port_mirror: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));

    // Sentinel-file writer so the CLI parent can learn our bound
    // port without trusting mDNS browse (which can return stale
    // entries from earlier macOS dev sessions).
    if let Ok(path) = std::env::var("IDEALYST_AAS_PORT_FILE") {{
        let port_for_file = port_mirror.clone();
        std::thread::spawn(move || {{
            for _ in 0..200 {{
                if let Ok(g) = port_for_file.lock() {{
                    if let Some(p) = *g {{
                        if let Err(e) = std::fs::write(&path, p.to_string()) {{
                            eprintln!(
                                "[aas-host] could not write port sentinel {{}}: {{}}",
                                path, e
                            );
                        }} else {{
                            eprintln!("[aas-host] wrote bound port {{}} to {{}}", p, path);
                        }}
                        return;
                    }}
                }}
                std::thread::sleep(std::time::Duration::from_millis(50));
            }}
            eprintln!("[aas-host] timed out waiting for serve to bind; no port sentinel written");
        }});
    }}

    eprintln!("[aas-host] starting (advertising app_id={{}} via mDNS)", APP_ID);
    serve_with_sidecar(addr, recorder, APP_ID, port_mirror, sidecar_slot)
}}

/// Attempt one hot-patch round. Reads the sidecar's cached ASLR
/// reference (populated by the sidecar's `Hello` frame), invokes
/// the builder, and ships the resulting JumpTable to the sidecar
/// over the existing host↔sidecar pipe. Returns Err on any
/// failure so the caller can fall back to respawn.
fn try_hotpatch(
    builder: &Option<Arc<HotPatchBuilder>>,
    sidecar_slot: &SidecarSlot,
) -> anyhow::Result<()> {{
    let builder = builder
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("hot-patch builder unavailable"))?;
    let aslr = {{
        let g = sidecar_slot
            .lock()
            .map_err(|_| anyhow::anyhow!("sidecar slot lock poisoned"))?;
        let s = g
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no sidecar running"))?;
        let v = s.aslr_reference();
        if v == 0 {{
            return Err(anyhow::anyhow!("sidecar has not reported aslr_reference yet"));
        }}
        v
    }};
    let artifact = builder.build(USER_CRATE, aslr)?;
    let table_json = serde_json::to_string(&artifact.table)?;
    let g = sidecar_slot
        .lock()
        .map_err(|_| anyhow::anyhow!("sidecar slot lock poisoned"))?;
    let s = g
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("sidecar slot empty"))?;
    s.send(SidecarIn::ApplyPatch {{ table_json }});
    Ok(())
}}

/// Fallback: rebuild sidecar via cargo, kill old, spawn new.
/// Same flow as the pre-hotpatch implementation.
fn respawn_sidecar(sidecar_slot: &SidecarSlot) {{
    let manifest = "{sidecar_manifest}";
    let target_dir = "{workspace_target}";
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest,
            "--target-dir",
            target_dir,
        ])
        .status();
    match status {{
        Ok(s) if s.success() => {{}}
        Ok(s) => {{
            eprintln!("[aas-host] respawn cargo build exited with {{s}} — sidecar unchanged");
            return;
        }}
        Err(e) => {{
            eprintln!("[aas-host] respawn cargo build spawn failed: {{e}}");
            return;
        }}
    }}
    if let Ok(mut g) = sidecar_slot.lock() {{
        if let Some(mut old) = g.take() {{
            old.kill();
        }}
        match Sidecar::spawn(&PathBuf::from(SIDECAR_BINARY)) {{
            Ok(s) => {{
                *g = Some(s);
                eprintln!("[aas-host] sidecar respawned");
            }}
            Err(e) => eprintln!("[aas-host] sidecar respawn failed: {{e}}"),
        }}
    }}
}}
"#,
        sidecar_name = sidecar_binary_name(&manifest.name),
        default_addr = DEFAULT_BIND_ADDR,
        app_id = manifest.app.require_bundle_id()?,
        sidecar_bin = sidecar_bin.display(),
        sidecar_manifest = sidecar_manifest.display(),
        user_src = user_src.display(),
        workspace_target = workspace_target.display(),
        captures_dir = sidecar_dir
            .parent()
            .map(|p| p.join("captures"))
            .unwrap_or_else(|| sidecar_dir.join("captures"))
            .display()
            .to_string(),
        user_crate = manifest.name,
        patch_target_dir = sidecar_dir
            .parent()
            .map(|p| p.join("patches"))
            .unwrap_or_else(|| sidecar_dir.join("patches"))
            .display()
            .to_string(),
    );

    write_shared_target_config(wrapper_dir, workspace_root)?;
    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Write a `.cargo/config.toml` that redirects builds into the
/// workspace's shared `target/` directory. Used by both wrappers so
/// `framework-core`, `dev-server`, etc. compile once across the
/// host, sidecar, and any workspace builds.
fn write_shared_target_config(dir: &Path, workspace_root: &Path) -> Result<()> {
    let target_dir = workspace_root.join("target");
    let config = format!(
        "# GENERATED. Share the workspace's `target/` so common\n\
         # dependencies aren't recompiled per-wrapper.\n\
         \n\
         [build]\n\
         target-dir = \"{}\"\n",
        target_dir.display(),
    );
    fs::create_dir_all(dir.join(".cargo"))?;
    fs::write(dir.join(".cargo/config.toml"), config)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cargo invocation
// ---------------------------------------------------------------------------

fn cargo_build(wrapper_dir: &Path, release: bool, label: &str) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }

    eprintln!(
        "[build-aas:{label}] cargo build{} (in {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-aas:{label}] cargo build exited with {status}");
    }
    Ok(())
}

/// "Fat" build for the sidecar: cargo with `RUSTC_WORKSPACE_WRAPPER`
/// pointing at the running idealyst CLI binary (which dispatches to
/// the `rustc-capture` subcommand via the env-var discriminator),
/// plus `RUSTFLAGS` augmented with `-Csave-temps=true
/// -Clink-dead-code`. Both flags are what dx ships for its
/// equivalent fat build — save-temps keeps the .rcgu.o files on disk
/// past link, and link-dead-code stops the linker from dropping
/// symbols the patch may want to resolve.
fn cargo_build_fat(
    wrapper_dir: &Path,
    release: bool,
    label: &str,
    captures_dir: &Path,
) -> Result<()> {
    let idealyst_bin = std::env::current_exe()
        .context("locate idealyst CLI binary for RUSTC_WORKSPACE_WRAPPER")?;
    let env = hotpatch::fat_build_env(&idealyst_bin, captures_dir);

    let mut cmd = Command::new("cargo");
    cmd.args(["build"]).current_dir(wrapper_dir);
    if release {
        cmd.arg("--release");
    }
    for (k, v) in &env {
        cmd.env(k, v);
    }

    eprintln!(
        "[build-aas:{label}] cargo build (fat){} (in {}; captures → {})",
        if release { " --release" } else { "" },
        wrapper_dir.display(),
        captures_dir.display(),
    );
    let status = cmd
        .status()
        .with_context(|| "spawn `cargo` — is it on your PATH?")?;
    if !status.success() {
        anyhow::bail!("[build-aas:{label}] cargo build exited with {status}");
    }
    Ok(())
}
