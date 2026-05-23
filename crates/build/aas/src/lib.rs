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
//! Statically links `{lib}::app()` and hosts N **independent author
//! runtimes**, one per dev-host session. Each session runs on its own
//! thread with its own `WireRecordingBackend`; commands flow OUT over
//! stdout tagged with the session id, events flow IN tagged with the
//! session id and dispatch through the right session's recorder.
//!
//! Why threads not processes: `framework_core`'s reactive runtime is
//! thread-local (ARENA / CURRENT / RUNNING). Spawning one thread per
//! session gives each its own isolated runtime "for free." Subsecond's
//! hot-patch jump table is process-wide, so a single `apply_patch`
//! covers every session simultaneously — exactly what we want when the
//! source code is the same.
//!
//! Lifecycle: spawned by the host on startup and re-spawned only on
//! hot-patch failure. Exits when stdin closes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{{stdin, stdout, BufReader, Write}};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread::JoinHandle;

use dev_server::sidecar::{{is_eof, read_frame, write_frame, SidecarIn, SidecarOut}};
use dev_server::WireRecordingBackend;
use framework_core::{{render, Owner}};
use {lib}::app;

/// Per-session control message dispatched from the main thread into
/// the session's owned thread. Each thread blocks on `recv()` for
/// these; the main thread routes by session id.
enum SessionMsg {{
    /// Forward an app→dev event into this session's recorder.
    Event(wire::AppToDev),
    /// Hot-patch has been applied process-wide. Tear down this
    /// session's `Owner`, reset its scene log, and re-render to pick
    /// up patched component bodies.
    Rerender,
    /// Graceful shutdown — the host has closed the session. The thread
    /// drops its `Owner` (firing any teardown effects) and exits.
    Shutdown,
}}

struct SessionHandle {{
    tx: mpsc::Sender<SessionMsg>,
    join: JoinHandle<()>,
}}

fn main() -> std::io::Result<()> {{
    // Report our `main` runtime address before anything else. The
    // host uses this to compute the ASLR slide for the symbol-table
    // diff in hot-patch builds. Doing it first keeps the host's
    // hotpatch builder usable from the very first file-change event.
    let main_addr: u64 = unsafe {{
        libc::dlsym(libc::RTLD_DEFAULT, b"main\0".as_ptr() as *const _) as u64
    }};

    // Outbound stdout is shared across all session threads. A `Mutex`
    // is the simplest way to serialize length-prefixed JSON frames
    // without a dedicated writer thread — frame writes are infrequent
    // (per-event or per-tick) so contention is minimal.
    let out = std::sync::Arc::new(Mutex::new(stdout()));

    {{
        let mut o = out.lock().expect("stdout lock");
        write_frame(&mut *o, &SidecarOut::Hello {{ aslr_reference: main_addr }})?;
        let _ = o.flush();
    }}

    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();

    let mut input = BufReader::new(stdin());
    loop {{
        let msg: SidecarIn = match read_frame(&mut input) {{
            Ok(f) => f,
            Err(e) if is_eof(&e) => {{
                eprintln!("[aas-app] host pipe closed; exiting");
                break;
            }}
            Err(e) => {{
                eprintln!("[aas-app] frame read error: {{e}} — exiting");
                return Err(e);
            }}
        }};

        match msg {{
            SidecarIn::CreateSession {{ session }} => {{
                if sessions.contains_key(&session) {{
                    eprintln!("[aas-app] CreateSession({{}}): already exists; ignoring", session);
                    continue;
                }}
                let (tx, rx) = mpsc::channel::<SessionMsg>();
                let out_clone = out.clone();
                let session_for_thread = session.clone();
                let join = std::thread::Builder::new()
                    .name(format!("aas-session-{{}}", session))
                    .spawn(move || {{
                        run_session_thread(session_for_thread, rx, out_clone);
                    }})
                    .expect("spawn session thread");
                sessions.insert(session.clone(), SessionHandle {{ tx, join }});
                // Acknowledge so the host's logs line up cleanly. We
                // don't gate readiness on initial render — the session
                // thread will ship `Commands` as soon as the first
                // render finishes, which is effectively the readiness
                // signal.
                let mut o = out.lock().expect("stdout lock");
                write_frame(
                    &mut *o,
                    &SidecarOut::SessionReady {{ session: session.clone() }},
                )?;
                let _ = o.flush();
            }}
            SidecarIn::CloseSession {{ session }} => {{
                let Some(handle) = sessions.remove(&session) else {{
                    eprintln!("[aas-app] CloseSession({{}}): no such session", session);
                    continue;
                }};
                let _ = handle.tx.send(SessionMsg::Shutdown);
                // Drop the sender so even if Shutdown never makes it
                // through, the thread's `recv()` returns Err and the
                // thread exits.
                drop(handle.tx);
                if let Err(e) = handle.join.join() {{
                    eprintln!("[aas-app] session thread panicked: {{:?}}", e);
                }}
                let mut o = out.lock().expect("stdout lock");
                write_frame(&mut *o, &SidecarOut::SessionEnded {{ session }})?;
                let _ = o.flush();
            }}
            SidecarIn::Event {{ session, event }} => {{
                let Some(handle) = sessions.get(&session) else {{
                    eprintln!("[aas-app] Event for unknown session {{:?}}; dropping", session);
                    continue;
                }};
                if handle.tx.send(SessionMsg::Event(event)).is_err() {{
                    eprintln!("[aas-app] session {{:?}} channel closed; pruning", session);
                    sessions.remove(&session);
                }}
            }}
            SidecarIn::ApplyPatch {{ table_json }} => {{
                match serde_json::from_str::<subsecond_types::JumpTable>(&table_json) {{
                    Ok(table) => {{
                        eprintln!(
                            "[aas-app] applying patch ({{}} jump-table entries)",
                            table.map.len(),
                        );
                        match unsafe {{ framework_hot::apply_patch(table) }} {{
                            Ok(()) => {{
                                eprintln!(
                                    "[aas-app] patch applied; notifying {{}} session(s) to re-render",
                                    sessions.len(),
                                );
                                // Fan out to every live session. If a
                                // send fails the thread is gone —
                                // it'll be pruned on its next message
                                // attempt.
                                for (id, handle) in &sessions {{
                                    if handle.tx.send(SessionMsg::Rerender).is_err() {{
                                        eprintln!(
                                            "[aas-app] session {{}} unreachable during rerender fan-out",
                                            id
                                        );
                                    }}
                                }}
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
    }}

    // Best-effort shutdown of any sessions still running when the host
    // closes its pipe.
    for (_, handle) in sessions.drain() {{
        let _ = handle.tx.send(SessionMsg::Shutdown);
        drop(handle.tx);
        let _ = handle.join.join();
    }}

    Ok(())
}}

/// Per-session worker. Owns its own `WireRecordingBackend` + `Owner`;
/// drains `SessionMsg`s from the main thread's router. Every emitted
/// command goes onto stdout tagged with this session's id.
fn run_session_thread(
    session: String,
    rx: mpsc::Receiver<SessionMsg>,
    out: std::sync::Arc<Mutex<std::io::Stdout>>,
) {{
    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let mut owner: Option<Owner> = Some(render(backend_rc.clone(), app()));
    let mut cursor = recorder.command_count();

    // Ship the initial render's snapshot up to the host.
    let initial = recorder.snapshot();
    if !initial.is_empty() {{
        if let Ok(mut o) = out.lock() {{
            let _ = write_frame(
                &mut *o,
                &SidecarOut::Commands {{
                    session: session.clone(),
                    cmds: initial,
                }},
            );
            let _ = o.flush();
        }}
    }}

    while let Ok(msg) = rx.recv() {{
        match msg {{
            SessionMsg::Event(app_to_dev) => {{
                dispatch_app_to_dev(&recorder, app_to_dev);
            }}
            SessionMsg::Rerender => {{
                // Hot-patch landed. Tear down + re-render so structural
                // changes propagate. Notify the host that its mirror
                // for this session needs to drop its log + scene
                // BEFORE we ship the post-rerender commands — otherwise
                // the host would treat the new batch as a delta on top
                // of stale state.
                owner = None;
                recorder.reset_log_and_scene();
                owner = Some(render(backend_rc.clone(), app()));
                cursor = 0;
                if let Ok(mut o) = out.lock() {{
                    let _ = write_frame(
                        &mut *o,
                        &SidecarOut::SessionReset {{ session: session.clone() }},
                    );
                    let _ = o.flush();
                }}
            }}
            SessionMsg::Shutdown => {{
                eprintln!("[aas-app] session {{}} shutting down", session);
                drop(owner);
                return;
            }}
        }}

        let count_now = recorder.command_count();
        if count_now > cursor {{
            let new_cmds = recorder.commands_since(cursor);
            cursor = count_now;
            if let Ok(mut o) = out.lock() {{
                let _ = write_frame(
                    &mut *o,
                    &SidecarOut::Commands {{
                        session: session.clone(),
                        cmds: new_cmds,
                    }},
                );
                let _ = o.flush();
            }}
        }}
    }}
    drop(owner);
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
    serve_with_sidecar_and_tracker, spawn_change_loop, SessionMode, SessionTracker, Sidecar,
    SidecarSlot, WireRecordingBackend,
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
    let session_tracker = SessionTracker::new();

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
    let tracker_for_rebuild = session_tracker.clone();
    let user_src = PathBuf::from("{user_src}");
    spawn_change_loop(
        vec![user_src],
        std::time::Duration::from_millis(100),
        Box::new(move || {{
            let t_total = std::time::Instant::now();
            let force_respawn = std::env::var("IDEALYST_AAS_NO_HOTPATCH")
                .ok()
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
            if force_respawn {{
                respawn_sidecar(&sidecar_for_rebuild, &tracker_for_rebuild);
                eprintln!(
                    "[aas-host] respawn applied in {{}}ms (force_respawn)",
                    t_total.elapsed().as_millis()
                );
                return;
            }}
            if let Err(e) = try_hotpatch(&hotpatch_for_rebuild, &sidecar_for_rebuild) {{
                eprintln!("[aas-host] hot-patch failed: {{e:#}} — respawning sidecar");
                respawn_sidecar(&sidecar_for_rebuild, &tracker_for_rebuild);
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

    // Replay any sessions the host knows about to the initial
    // sidecar. The very first time around the tracker is empty (no
    // clients have connected yet), but this same call happens after
    // every respawn — that's how a hot-patch-failure-fallback respawn
    // re-creates the author runtime threads for already-connected
    // clients.
    replay_sessions_to_sidecar(&sidecar_slot, &session_tracker);

    // Session mode is a host-side decision driven by the env. Default
    // is PerClient — every connecting device gets its own scene. Set
    // `IDEALYST_AAS_MULTI_SESSION=0` (or `false`/`no`/`off`) to flip to
    // Shared mode where every device drives one common scene (legacy
    // "synced devices" behavior).
    let session_mode = SessionMode::from_env();
    eprintln!(
        "[aas-host] starting (advertising app_id={{}} via mDNS, session mode = {{:?}})",
        APP_ID,
        session_mode,
    );
    serve_with_sidecar_and_tracker(
        addr,
        recorder,
        APP_ID,
        port_mirror,
        sidecar_slot,
        session_tracker,
        session_mode,
    )
}}

/// Send `CreateSession` to the live sidecar for every session id the
/// tracker knows about. No-op when the slot is empty. Called once on
/// startup (idempotent for an empty tracker) and after every respawn.
fn replay_sessions_to_sidecar(slot: &SidecarSlot, tracker: &SessionTracker) {{
    let sessions = tracker.snapshot();
    if sessions.is_empty() {{
        return;
    }}
    let Ok(guard) = slot.lock() else {{ return }};
    let Some(sidecar) = guard.as_ref() else {{ return }};
    eprintln!(
        "[aas-host] replaying {{}} session(s) to fresh sidecar",
        sessions.len(),
    );
    for s in sessions {{
        sidecar.send(SidecarIn::CreateSession {{ session: s }});
    }}
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

/// Fallback: rebuild sidecar via cargo, kill old, spawn new. After the
/// new sidecar comes up, replay every session the host believes is
/// alive so connected clients pick up where they left off.
fn respawn_sidecar(sidecar_slot: &SidecarSlot, tracker: &SessionTracker) {{
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
    // Slot guard is dropped here. Now reach into the slot again to
    // replay sessions to the freshly-spawned sidecar.
    replay_sessions_to_sidecar(sidecar_slot, tracker);
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
