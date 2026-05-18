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
use build_ios::{find_workspace_root, parse_manifest, Manifest};

mod dylib_mode;

/// Which AAS architecture to generate.
///
/// `Sidecar` is the production-ready default — two cooperating
/// processes joined by a stdin/stdout pipe. Per-edit rebuild is
/// dominated by the sidecar's cargo link step (~0.40s on macOS).
///
/// `Dylib` is an **experimental** alternative that compiles the user
/// crate as a Rust dylib and `dlopen`s it from a single long-lived
/// host. Targets ~150–200 ms per edit by skipping the host's
/// re-link entirely. On stable Rust this hits hard ABI issues
/// (mixed rlib/dylib generations of `framework-core` produce
/// `_RUST_STD_INTERNAL_VAL` hash mismatches at dlopen time); kept
/// here as an opt-in path for iterating on the fix without rolling
/// back the working sidecar mode.
///
/// Toggle via `IDEALYST_AAS_MODE=dylib` (env-var sniff in CLI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AasMode {
    Sidecar,
    Dylib,
}

impl Default for AasMode {
    fn default() -> Self {
        AasMode::Sidecar
    }
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Compile with `--release`. Default: debug. The host and sidecar
    /// both run locally — release is almost never worth the slower
    /// rebuild cycle here.
    pub release: bool,
    /// Sidecar (default) or experimental dlopen-driven dylib mode.
    pub mode: AasMode,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            release: false,
            mode: AasMode::default(),
        }
    }
}

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the produced infra-host executable.
    pub host_binary: PathBuf,
    /// Path to the produced sidecar executable, or to the user dylib
    /// in `AasMode::Dylib`. Naming kept for backwards compat with the
    /// CLI's existing artifact-display logic.
    pub sidecar_binary: PathBuf,
    /// Host wrapper crate directory.
    pub wrapper_dir: PathBuf,
    /// Sidecar / user-dylib wrapper crate directory.
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
    let workspace_root = find_workspace_root(&project_dir)?;

    match opts.mode {
        AasMode::Sidecar => build_sidecar_mode(&project_dir, &workspace_root, &manifest, &opts),
        AasMode::Dylib => build_dylib_mode(&project_dir, &workspace_root, &manifest, &opts),
    }
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

    // Build the sidecar first so the host (which spawns it on
    // startup) finds the binary present. The host build is
    // independent and only depends on dev-server.
    cargo_build(&sidecar_dir, opts.release, "sidecar")?;
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

fn build_dylib_mode(
    project_dir: &Path,
    workspace_root: &Path,
    manifest: &Manifest,
    opts: &BuildOptions,
) -> Result<BuildArtifact> {
    dylib_mode::build(project_dir, workspace_root, manifest, opts)
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
    let dev_server = workspace_root.join("crates/dev/server");
    let wire = workspace_root.join("crates/framework/wire");

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Sidecar binary: statically links the user's crate, runs the
# reactive runtime, and streams wire commands / reads events over
# stdout / stdin. The infra host (`<name>-aas-host`) spawns this as a
# child process and restarts it on every source change.

[workspace]

[package]
name = "{sidecar_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
framework-core = {{ path = "{fcore}" }}
dev-server = {{ path = "{dev_server}" }}
wire = {{ path = "{wire}" }}
{user_name} = {{ path = "{user_path}" }}
serde_json = "1"

# Sidecar is short-lived dev infra — strip everything that costs
# link time. We can't use `panic = "abort"` here because the host's
# dlopen-mode shares this same template path; dylibs are constrained
# to `unwind`. debug = 0 + strip is the next-best squeeze and gets
# the rebuild down to ~0.40s on macOS.
[profile.dev]
debug = 0
strip = "debuginfo"
"#,
        sidecar_name = sidecar_name,
        fcore = fcore.display(),
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
use framework_core::render;
use {lib}::app;

fn main() -> std::io::Result<()> {{
    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Drive the user's tree through the real walker once at startup.
    // The recorder accumulates wire commands into its append-only
    // log; we drain that log into a single initial `Commands` frame
    // for the host.
    let owner = render(backend_rc, app());
    // Keep the framework runtime alive for the lifetime of the
    // process — dropping `owner` would tear down every scope and
    // free every signal that backs reactive UI.
    std::mem::forget(owner);

    // Initial snapshot: every command emitted during the first walk.
    let initial = recorder.snapshot();
    let mut outbound_cursor = recorder.command_count();
    let mut out = stdout();
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
            SidecarIn::Event(app_to_dev) => {{
                eprintln!("[aas-app] inbound event: {{:?}}", std::mem::discriminant(&app_to_dev));
                dispatch_app_to_dev(&recorder, app_to_dev);
            }}
        }}

        // Drain any commands the event produced. `commands_since` is
        // append-only relative to the recorder's log, so the cursor
        // advances by exactly the number of new commands.
        let count_now = recorder.command_count();
        eprintln!("[aas-app] after dispatch: cursor={{}} count={{}}", outbound_cursor, count_now);
        if count_now > outbound_cursor {{
            let new_cmds = recorder.commands_since(outbound_cursor);
            outbound_cursor = count_now;
            eprintln!("[aas-app] writing {{}} new commands", new_cmds.len());
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
            let dispatched = recorder.dispatch_event(handler, args);
            eprintln!("[aas-app] dispatch_event handler={{:?}} dispatched={{}}", handler, dispatched);
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

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build aas`. Do not edit — rewritten
# every build.
#
# Infra-only AAS host: WebSocket server, mDNS, file watcher. Does NOT
# link the user's crate — that lives in the sibling `aas/app` sidecar
# binary, which this host spawns as a child process.

[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[dependencies]
dev-server = {{ path = "{dev_server}" }}
serde_json = "1"
"#,
        wrapper_name = wrapper_name,
        dev_server = dev_server.display(),
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
//! Owns: WebSocket listener (mDNS-advertised), file watcher,
//! sidecar-child orchestration. The user's reactive runtime lives in
//! the sibling sidecar (`{sidecar_name}`), which this host spawns
//! over stdin/stdout pipes. On source change the host rebuilds the
//! sidecar and SIGKILL+respawns it — the WebSocket listener stays up
//! the entire time so connected clients (Android, iOS) never see a
//! disconnect.

use std::path::PathBuf;
use std::sync::{{Arc, Mutex}};

use dev_server::{{
    serve_with_sidecar, spawn_rebuild_loop, RebuildCommand, RebuildConfig,
    Sidecar, SidecarSlot, WireRecordingBackend,
}};

const DEFAULT_ADDR: &str = "{default_addr}";
/// mDNS-published app identifier.
const APP_ID: &str = "{app_id}";
/// Absolute path to the sidecar binary the host spawns on startup
/// and respawns on every successful rebuild. Baked at host build
/// time — both binaries live under the workspace's shared target
/// directory.
const SIDECAR_BINARY: &str = "{sidecar_bin}";

fn main() -> std::io::Result<()> {{
    let addr = if let Some(a) = std::env::args().nth(1) {{
        a
    }} else if let Ok(p) = std::env::var("IDEALYST_AAS_BIND_PORT") {{
        format!("0.0.0.0:{{}}", p)
    }} else {{
        DEFAULT_ADDR.to_string()
    }};

    // The host's recorder is a passive command mirror — the sidecar
    // is what runs user code. New wire commands arrive on the
    // sidecar's stdout, get pushed into this recorder by the serve
    // loop's drain pass, and broadcast to clients from there.
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

    // File watcher → rebuild sidecar → kill old → respawn.
    // The host never self-execs in this architecture, so the
    // WebSocket listener stays up the entire time. The recorder is
    // !Send (Rc-based) and can't be touched from the watcher thread
    // — we don't reset it here. That's intentional: the scene model
    // overwrites stale NodeIds with new content as the new sidecar's
    // commands arrive, and existing clients only see commands past
    // their cursor (the new sidecar's emits), so AasClient's
    // idempotent replay reconciles same-NodeId updates correctly.
    let sidecar_for_rebuild = sidecar_slot.clone();
    let sidecar_manifest = PathBuf::from("{sidecar_manifest}");
    let user_src = PathBuf::from("{user_src}");
    spawn_rebuild_loop(RebuildConfig {{
        command: RebuildCommand {{
            program: "cargo".into(),
            args: vec![
                "build".into(),
                "--manifest-path".into(),
                sidecar_manifest.display().to_string(),
                "--target-dir".into(),
                "{workspace_target}".into(),
            ],
            cwd: None,
        }},
        watch_paths: vec![user_src],
        debounce: std::time::Duration::from_millis(100),
        before_exec: None,
        on_success: Some(Box::new(move || {{
            eprintln!("[aas-host] sidecar rebuilt → swapping");
            if let Ok(mut g) = sidecar_for_rebuild.lock() {{
                if let Some(mut old) = g.take() {{
                    old.kill();
                }}
                match Sidecar::spawn(&PathBuf::from(SIDECAR_BINARY)) {{
                    Ok(s) => *g = Some(s),
                    Err(e) => eprintln!("[aas-host] sidecar respawn failed: {{e}}"),
                }}
            }}
        }})),
    }});

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
"#,
        sidecar_name = sidecar_binary_name(&manifest.name),
        default_addr = DEFAULT_BIND_ADDR,
        app_id = manifest.app.require_bundle_id()?,
        sidecar_bin = sidecar_bin.display(),
        sidecar_manifest = sidecar_manifest.display(),
        user_src = user_src.display(),
        workspace_target = workspace_target.display(),
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
