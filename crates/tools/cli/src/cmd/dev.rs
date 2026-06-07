//! `idealyst dev` — orchestrate the dev pipeline for one or more
//! platform targets.
//!
//! Two orthogonal axes:
//!
//! - **Mode**: runtime-server (default) or local-render (`--local`).
//!   - runtime-server (default): a single dev-server process runs the
//!     user's reactive runtime; every platform's client connects over a
//!     WebSocket and renders whatever wire commands arrive. Source
//!     changes only rebuild the dev-host binary, the navigator stack
//!     survives, every client stays in sync. This is the hot-reload
//!     experience — saves apply in place with state preserved.
//!   - `--local`: each platform builds the user's `app()` for itself
//!     with a file-watcher rebuild loop. Web uses livereload (full
//!     page reload on change); native platforms restart the app on
//!     rebuild. State does not survive saves. Use when the workspace
//!     isn't available for the runtime-server sidecar build, or to
//!     bypass the wire protocol entirely.
//!
//! - **Targets**: `--web`, `--ios`, `--android`, `--macos`. If none
//!   are passed explicitly, the active set comes from `[package
//!   .metadata.idealyst.app].targets` in `Cargo.toml`. `--all`
//!   expands to every platform the host can build for (web + android
//!   anywhere, plus ios + macos on darwin); use it for a side-by-side
//!   comparison of every backend at once.
//!
//! Multiple platforms run in parallel — each in its own thread —
//! and Ctrl-C tears all of them down together. A failure in one
//! target prints a `[dev <target>] launch failed: …` line and the
//! remaining targets keep running.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use build_ios::{parse_manifest, Target};

/// Cargo features for dev-mode builds. The macOS wrapper template
/// declares its own `dev = ["runtime-core/dev"]` feature that
/// gates `--emit-catalog` mode in main; passing the wrapper's
/// own feature is what we want there. iOS / Android / Web wrappers
/// don't (yet) have a wrapper-side `dev` feature, so we activate
/// `runtime-core/dev` directly across the dep boundary — same
/// effect on the framework, just no per-wrapper feature gates.
fn dev_user_features_macos() -> Vec<String> {
    vec!["dev".to_string()]
}

fn dev_user_features_other() -> Vec<String> {
    vec!["runtime-core/dev".to_string()]
}

/// Compute the env vars the dev launcher sets on the spawned app.
///
/// - `IDEALYST_BRIDGE_PORT` (optional): pins the Robot bridge to a
///   specific port instead of letting it pick ephemeral.
/// - `IDEALYST_BRIDGE_PORT_FILE`: where the bridge writes its bound
///   port + identity JSON. Project-scoped MCP servers (cwd-anchored)
///   read this; the bridge ALSO writes `~/.idealyst/apps/<name>-<pid>.json`
///   for host-wide MCP discovery.
/// - `IDEALYST_DEV_ENDPOINT` (runtime-server mode only): the
///   `ws://host:port` URL the wrapper connects to. Set for platforms
///   whose process the CLI spawns directly (macOS / terminal / wgpu
///   sim / web-host); iOS / Android bake the URL into their build
///   manifests instead.
fn dev_env_vars(
    project_dir: &Path,
    args: &Args,
    _app_name: &str,
    _catalog_bin: Option<&Path>,
    endpoint: Option<&str>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dev_cfg = crate::dev_config::DevConfig::load(project_dir).unwrap_or_default();
    let pinned = args.bridge_port.or(dev_cfg.bridge_port);
    if let Some(p) = pinned {
        out.push(("IDEALYST_BRIDGE_PORT".to_string(), p.to_string()));
    }
    out.push((
        "IDEALYST_BRIDGE_PORT_FILE".to_string(),
        bridge_port_file(project_dir).to_string_lossy().into_owned(),
    ));
    if let Some(endpoint) = endpoint {
        out.push(("IDEALYST_DEV_ENDPOINT".to_string(), endpoint.to_string()));
    }
    out
}

/// `<project>/.idealyst/bridge.port` — the per-project location the
/// running app's Robot bridge writes its bound port JSON to. Read by
/// project-scoped MCP servers (which run with cwd == project root).
fn bridge_port_file(project_dir: &Path) -> PathBuf {
    project_dir.join(".idealyst").join("bridge.port")
}

/// Resolve a stable LAN IP for this host. Used when baking
/// `IDEALYST_DEV_ENDPOINT` for builds that will run on a separate
/// device (currently only iOS, when device builds are wired up).
/// Falls back to `127.0.0.1` when no non-loopback interface is found —
/// fine for sim / emulator paths which the CLI handles separately
/// via `localhost` / `10.0.2.2`.
#[allow(dead_code)] // Wired when iOS device builds land; sim/emulator use loopback today.
fn resolve_lan_ip() -> String {
    // UDP-connect trick: bind 0.0.0.0:0, "connect" to a routable
    // address (no packets sent — UDP connect is just a routing-table
    // lookup), then read `local_addr()`. Picks whatever interface
    // the OS would route external traffic out of, which is what a
    // device on the same Wi-Fi reaches us at.
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip().to_string();
                if !ip.starts_with("127.") {
                    return ip;
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

/// Project name = `[package].name` from the user's Cargo.toml. Used
/// as the `name` field on the registry's `AppEntry`. Falls back to
/// the project directory's basename if Cargo.toml parsing fails.
fn project_app_name(project_dir: &Path) -> String {
    let cargo = project_dir.join("Cargo.toml");
    if let Ok(raw) = std::fs::read_to_string(&cargo) {
        if let Ok(value) = toml::from_str::<toml::Value>(&raw) {
            if let Some(name) = value
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                return name.to_string();
            }
        }
    }
    project_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".to_string())
}

/// Legacy registry-cleanup hook from before discovery moved to the
/// per-process `~/.idealyst/apps/<name>-<pid>.json` files (whose
/// RAII drop handles cleanup). Kept as a no-op so the call sites
/// don't all need to disappear in this diff.
fn pre_launch_clear_registry(_project_dir: &Path) {}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Opt out of the runtime-server: build + run the user's `app()`
    /// natively on each platform with its own file-watcher rebuild
    /// loop. Web uses livereload (full page reload on save); native
    /// platforms restart the app. State does not survive saves.
    ///
    /// The default (off) starts the runtime-server sidecar — a single
    /// dev process holds the user's reactive tree and every platform
    /// client connects over WebSocket. Source changes apply as
    /// hot-patches with state preserved.
    ///
    /// Use `--local` when the framework workspace isn't reachable
    /// for the sidecar build, or for a faster cold-start at the cost
    /// of in-place saves.
    #[arg(long)]
    pub local: bool,

    /// Build and run the web target.
    #[arg(long)]
    pub web: bool,

    /// Build and run on the iOS simulator.
    #[arg(long)]
    pub ios: bool,

    /// Build and run on the Android emulator.
    #[arg(long)]
    pub android: bool,

    /// Build and run as a native macOS app (AppKit-backed via
    /// `host-appkit` + `backend-macos`).
    #[arg(long)]
    pub macos: bool,

    /// Build and run as a TTY app (crossterm-backed via
    /// `host-terminal` + `backend-terminal`). Takes over the current
    /// terminal — incompatible with `--web` / `--all` in the same
    /// session because both want stdout.
    #[arg(long)]
    pub terminal: bool,

    /// Launch every platform the host can build for in parallel —
    /// web + android always; ios + macos additionally on darwin.
    /// Targets that fail to launch don't abort the others. Useful
    /// for a side-by-side comparison of every backend at once.
    #[arg(long)]
    pub all: bool,

    /// Serve the project as a server-side-rendered site with hydration
    /// at `--ssr-port` (default 8081). Builds a native SSR binary that
    /// renders `app()` per request, emitting the boot `<script>` so the
    /// live web bundle adopts the server DOM. Requires the wasm bundle
    /// to be staged (build automatically; suppressed with `--no-build`).
    /// Can stack with `--web` (static-file server on `--port`) and
    /// `--static` (no-hydration variant on `--static-port`).
    #[arg(long)]
    pub ssr: bool,

    /// Serve the project as pure server-side-rendered HTML (no
    /// hydration, no `<script>`) at `--static-port` (default 8082).
    /// Useful for SEO / unfurls / static previews — the page is exactly
    /// what the server paints, with no client takeover.
    #[arg(long = "static")]
    pub static_only: bool,

    /// HTTP port for the web target's static-file server.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// HTTP port for the `--ssr` SSR-with-hydration server.
    #[arg(long, default_value_t = 8081)]
    pub ssr_port: u16,

    /// HTTP port for the `--static` SSR-without-hydration server.
    #[arg(long, default_value_t = 8082)]
    pub static_port: u16,

    /// Interface for the web target's static-file server.
    /// `0.0.0.0` to expose to the LAN.
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Pin the Robot bridge to a specific port on the launched app.
    /// Overrides `dev.toml` and the default (ephemeral). Use when an
    /// external tool needs a stable target; otherwise omit and let
    /// the bridge pick a free port (its address is written to
    /// `.idealyst/bridge.port` for the MCP server to read).
    #[arg(long, value_name = "PORT")]
    pub bridge_port: Option<u16>,

    /// Web only: skip the initial build and just start the static
    /// server. Use when `pkg/` is already up to date.
    #[arg(long)]
    pub no_build: bool,

    /// Boot the interactive panel (dev-tui) on the current terminal.
    /// Renders per-target state + a live log stream using the
    /// framework's own terminal backend, dogfooding `host-terminal`.
    ///
    /// Disabled in CI: if stderr isn't a TTY, the flag is ignored
    /// and the CLI falls back to today's line-oriented output, so
    /// scripted invocations stay untouched.
    ///
    /// Incompatible with `--terminal` (both want the foreground TTY).
    #[arg(long)]
    pub interactive: bool,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir).with_context(|| {
        format!("cannot resolve project dir {}", args.dir.display())
    })?;

    // Resolve the active target set. Explicit flags win; if none are
    // passed, fall back to the manifest's `targets`. We parse the
    // manifest either way so the user sees the same error message
    // shape ("missing targets") regardless of how they invoked.
    let manifest = parse_manifest(&dir)?;
    let active_targets = resolve_targets(&args, &manifest.app.targets)?;

    // Decide if the interactive panel should actually boot. The flag
    // is the user's request; we still gate on a real TTY (so piped
    // invocations and CI keep getting line-oriented output) and on
    // not colliding with the `--terminal` build target — both would
    // fight for the foreground TTY and corrupt each other.
    let interactive = args.interactive
        && {
            use std::io::IsTerminal;
            std::io::stderr().is_terminal()
        }
        && !active_targets.contains(&Target::Terminal);
    if args.interactive && !interactive {
        // Tell the user *why* the flag was ignored so they don't think
        // it silently failed. Goes through `eprintln!` because the
        // panel isn't up yet.
        eprintln!(
            "[dev] --interactive ignored (stderr is not a TTY, or --terminal target is active)"
        );
    }

    crate::dlog!(
        "dev",
        "{} mode, targets: {}",
        if args.local { "local" } else { "runtime-server" },
        active_targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );

    // Interactive mode: redirect stderr to a log file BEFORE we
    // spawn any worker thread. Workers immediately kick off cargo
    // builds via the platform `build-*` / `run-*` crates, which
    // inherit our stdio — cargo's `Compiling …` chatter goes to
    // stderr, which without this redirect lands on the TTY for the
    // ~30s before host-terminal's own redirect installs inside
    // `dev_tui::run`. The bytes look indistinguishable from a panic
    // backtrace once they interleave with the eventual crossterm
    // alternate-screen escapes. Held alive until the end of `run()`
    // so the file stays the active stderr for the whole session.
    //
    // We keep fd 1 (stdout) on the TTY because crossterm writes its
    // ANSI paint stream through `io::stdout()`. Subprocesses can
    // still write to fd 1 (most don't — cargo / xcrun / simctl talk
    // on stderr) but those bursts are rare and short. Real fix is
    // plumbing a `Stdio` arg through the build crates; tracked as
    // a follow-up.
    let _stderr_guard = if interactive {
        Some(EarlyStderrRedirect::install(&dir.join(".idealyst").join("dev.log")))
    } else {
        None
    };

    // Child handles for cleanup-on-Ctrl-C. Each platform launcher
    // pushes any subprocesses it spawns here; the signal handler
    // walks the vec and kills everything.
    let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(Vec::new()));
    // PID of the spawned macOS app, if any. The macOS launcher records it
    // here so the tail of this fn can wait on the app's lifetime: closing
    // the app window should tear the dev session down (and bring the
    // terminal back), the mirror of the launcher-watchdog that makes the
    // app die when the CLI does. See the wait loop after the worker join.
    let macos_app_pid: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    // Interactive mode: crossterm (via host-terminal inside dev-tui)
    // captures Ctrl-C in raw mode. Installing the usual handler would
    // call `std::process::exit(0)` mid-frame, skipping host-terminal's
    // Drop-based terminal-restore and leaving the user in raw mode
    // with no echo. The TUI's quit path already kills children, so
    // skip the handler entirely.
    if !interactive {
        install_ctrlc_handler(children.clone())?;
    }

    // Bus published to by `dlog(...)` once installed; the dev-tui
    // app drains it every frame on the main thread.
    let dev_bus = if interactive {
        let bus = dev_tui::DevBus::new();
        crate::dev_log::install(bus.clone());
        Some(bus)
    } else {
        None
    };

    // In runtime-server mode (the default), start the dev-server once
    // before launching any platform — all clients connect to the same
    // server. The host self-execs on source change so we don't need
    // to restart it.
    //
    // We point the host at a sentinel file via
    // `IDEALYST_RUNTIME_SERVER_PORT_FILE`; it writes its bound port
    // there as soon as `TcpListener::bind` succeeds. The CLI waits
    // (below) for the file to appear, then bakes
    // `IDEALYST_DEV_ENDPOINT=ws://host:port` into every platform
    // launch — no in-app discovery needed.
    // PID of the spawned dev-host child, if any. Used by the tail of
    // this fn to keep the CLI alive while the host serves — see the
    // wait loop after the worker join.
    let mut host_pid: Option<u32> = None;
    let runtime_server_port: Option<u16> = if !args.local {
        let host_binary = build_runtime_server_host(&dir)?;
        let port_file = runtime_server_port_file(&dir);
        // Clear any stale value from a previous session before
        // letting the host overwrite it — keeps reads from picking
        // up the previous run's number during the bind window.
        let _ = std::fs::remove_file(&port_file);
        let mut cmd = Command::new(&host_binary);
        cmd.env("IDEALYST_RUNTIME_SERVER_PORT_FILE", &port_file);
        // When the terminal target is in the active set, redirect the
        // dev-host's stdio to a log file — its `[runtime-server-host]
        // hot-patch applied …` chatter (every save!) would otherwise
        // splatter ANSI-escape-unaware bytes onto the same TTY where
        // crossterm is diff-painting the user's app, shredding the
        // cell grid. Same reasoning applies to `--interactive`: the
        // dev-tui panel paints over the same TTY, and inherited
        // dev-host stdout looks like a panic backtrace once it
        // interleaves with crossterm's ANSI sequences. Other targets
        // keep inherited stdio so dev-host logs stay visible in the
        // orchestrator's terminal.
        if active_targets.contains(&Target::Terminal) || interactive {
            let log_dir = dir.join(".idealyst");
            let _ = std::fs::create_dir_all(&log_dir);
            let log_path = log_dir.join("dev-host.log");
            match std::fs::File::create(&log_path) {
                Ok(file) => {
                    let stderr = file.try_clone().unwrap_or_else(|_| {
                        std::fs::File::create("/dev/null").expect("open /dev/null")
                    });
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::from(file))
                        .stderr(std::process::Stdio::from(stderr));
                }
                Err(e) => {
                    eprintln!(
                        "[dev] could not open {} for dev-host log; falling back to /dev/null: {}",
                        log_path.display(),
                        e
                    );
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                }
            }
        }
        let child = cmd
            .spawn()
            .with_context(|| {
                format!(
                    "spawn runtime-server host {} — build succeeded but the binary won't run",
                    host_binary.display(),
                )
            })?;
        crate::dlog!(
            "dev",
            "runtime-server host running ({}); port file {}",
            host_binary.display(),
            port_file.display(),
        );
        host_pid = Some(child.id());
        children.lock().unwrap().push(child);

        // Block until the host writes its bound port. Every platform
        // launcher needs this to bake `IDEALYST_DEV_ENDPOINT`. 10s is
        // generous — `TcpListener::bind` returns synchronously, so
        // the port lands as soon as the host's runtime is up.
        match read_host_port_file(&port_file, std::time::Duration::from_secs(10)) {
            Some(port) => {
                crate::dlog!("dev", "runtime-server bound port = {}", port);
                Some(port)
            }
            None => {
                anyhow::bail!(
                    "runtime-server host never wrote its port to {} \
                     within 10s — the host process likely crashed at startup",
                    port_file.display(),
                );
            }
        }
    } else {
        None
    };

    // Spawn one worker thread per active target. We pre-build the
    // per-target context outside the thread so a setup error
    // surfaces synchronously.
    // Terminal target needs the foreground TTY (raw mode + alternate
    // screen), so we run it on the main thread instead of in a worker
    // and tear everything else down when it exits. Forbid combining
    // with `--web` / `--all` (web's `serve_static` would print into
    // the terminal app's alternate screen and corrupt the rendering).
    let terminal_only = active_targets.contains(&Target::Terminal);
    if terminal_only {
        let other_targets: Vec<_> = active_targets
            .iter()
            .copied()
            .filter(|t| *t != Target::Terminal)
            .collect();
        for target in &other_targets {
            let dir = dir.clone();
            let args_clone = args.shallow_clone();
            let target = *target;
            let children_for_worker = children.clone();
            let macos_pid_for_worker = macos_app_pid.clone();
            std::thread::spawn(move || {
                if let Err(e) = launch_target(target, &dir, &args_clone, children_for_worker, macos_pid_for_worker, runtime_server_port) {
                    crate::dlog!(&format!("dev {}", target), "launch failed: {e:#}");
                }
            });
        }
        // Run terminal foreground on the main thread; blocks until
        // the user quits. Ctrl-C while in the terminal app falls
        // through to crossterm, not our handler — but when the app
        // exits, control returns here and we drop into the children-
        // kill loop below.
        if let Err(e) = launch_target(Target::Terminal, &dir, &args, children.clone(), macos_app_pid.clone(), runtime_server_port) {
            crate::dlog!("dev terminal", "launch failed: {e:#}");
        }
        // Clean up sibling targets.
        if let Ok(mut guard) = children.lock() {
            for mut child in guard.drain(..) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        return Ok(());
    }

    let mut workers = Vec::new();
    for target in &active_targets {
        let dir = dir.clone();
        let args_clone = args.shallow_clone();
        let target = *target;
        let children_for_worker = children.clone();
        let macos_pid_for_worker = macos_app_pid.clone();
        let worker = std::thread::spawn(move || {
            if let Err(e) = launch_target(target, &dir, &args_clone, children_for_worker, macos_pid_for_worker, runtime_server_port) {
                crate::dlog!(&format!("dev {}", target), "launch failed: {e:#}");
            }
        });
        workers.push(worker);
    }

    // Spawn SSR workers — one per requested SSR variant. They live
    // alongside the per-target workers but aren't a "platform" in the
    // Target enum sense (they're independent HTTP servers, each with
    // its own port). Each calls `launch_ssr` which builds the SSR
    // wrapper binary and spawns it; the spawned process is registered
    // with `children` so the Ctrl-C handler tears it down.
    if args.ssr {
        let dir = dir.clone();
        let args_clone = args.shallow_clone();
        let children_for_worker = children.clone();
        let worker = std::thread::spawn(move || {
            if let Err(e) = launch_ssr(&dir, &args_clone, false, children_for_worker) {
                crate::dlog!("dev ssr", "launch failed: {e:#}");
            }
        });
        workers.push(worker);
    }
    if args.static_only {
        let dir = dir.clone();
        let args_clone = args.shallow_clone();
        let children_for_worker = children.clone();
        let worker = std::thread::spawn(move || {
            if let Err(e) = launch_ssr(&dir, &args_clone, true, children_for_worker) {
                crate::dlog!("dev static", "launch failed: {e:#}");
            }
        });
        workers.push(worker);
    }

    // Interactive panel takes the foreground TTY and blocks until
    // the user quits (q / Esc / Ctrl-C, handled by crossterm inside
    // host-terminal). On return we tear down spawned children and
    // drop the worker handles — they'll exit on broken pipes /
    // killed subprocesses.
    if let Some(bus) = dev_bus {
        let project_name = project_app_name(&dir);
        let targets: Vec<dev_tui::TargetInfo> = active_targets
            .iter()
            .map(|t| dev_tui::TargetInfo {
                name: t.as_str().to_string(),
            })
            .collect();
        let opts = dev_tui::RunOptions {
            project_name,
            targets,
            runtime_server: !args.local,
        };
        // Blocks. host-terminal handles raw mode + alternate screen +
        // stderr redirect for the duration; on quit it restores
        // everything before returning.
        match dev_tui::run(bus, opts) {
            Ok(()) => eprintln!("[dev] interactive panel exited cleanly"),
            Err(e) => eprintln!("[dev] interactive panel errored: {:?}", e),
        }

        // Clean up spawned subprocesses. Mirrors the terminal-target
        // path's drain-and-kill loop below.
        if let Ok(mut guard) = children.lock() {
            for mut child in guard.drain(..) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        return Ok(());
    }

    // Wait for all workers to settle. In practice a foreground worker
    // (web's `serve_static` HTTP loop) blocks forever — so this join
    // effectively waits for Ctrl-C, which terminates the process via
    // the handler installed above. We still `.join` so a
    // background-only target (iOS / Android launch + return) doesn't
    // make us exit immediately when its worker finishes.
    for w in workers {
        let _ = w.join();
    }

    // If every active target launched-and-returned (the common case
    // for an `android`- or `ios`-only run — adb/simctl install the app
    // and the worker exits) AND we're in runtime-server mode, we must
    // NOT fall off the end of `main` here. The dev-host is a CHILD of
    // this process; once the CLI exits, the host reparents to launchd
    // (pid 1) and its parent-pid watchdog (added in `b5bf102` to reap
    // orphaned hosts) SIGKILLs it within ~500ms — leaving the app we
    // just launched connected to a dead server, i.e. a blank screen.
    //
    // Keep the CLI alive (and thus the host parented to it) until the
    // user interrupts — Ctrl-C drains+kills `children` (host included)
    // and exits — or the host dies on its own (crash / self-exec gone
    // wrong), in which case we tear down and exit cleanly so the
    // terminal isn't left hanging.
    // PIDs whose exit should end the whole dev session: the runtime-server
    // host (so a host crash tears down cleanly) and the macOS app (so
    // closing its window brings the terminal back — symptom: "I close the
    // app and the terminal just keeps running"). Whichever exits first
    // triggers teardown.
    let mut watch_pids: Vec<u32> = Vec::new();
    if let Some(pid) = host_pid {
        watch_pids.push(pid);
    }
    if let Some(pid) = *macos_app_pid.lock().unwrap() {
        watch_pids.push(pid);
    }
    if !watch_pids.is_empty() {
        wait_for_any_child_exit(&children, &watch_pids);
        if let Ok(mut guard) = children.lock() {
            for mut child in guard.drain(..) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    Ok(())
}

/// Block until ANY of `watch_pids` is no longer running, then return.
/// Polls rather than `Child::wait()` because the `Child` handles live in
/// the shared `children` vec (the Ctrl-C handler needs them there to tear
/// everything down), and `wait` would take ownership / hold the lock
/// across a blocking call. A short poll interval keeps teardown latency
/// low without busy-spinning.
///
/// The watched pids are the long-lived "session anchors" — the
/// runtime-server host and/or the macOS app. The first to exit ends the
/// wait, and the caller then drains+kills the rest. Returns early if a
/// watched pid is no longer tracked in `children` at all — that means the
/// Ctrl-C handler already drained the vec (the process is exiting
/// underneath us).
fn wait_for_any_child_exit(children: &Arc<Mutex<Vec<Child>>>, watch_pids: &[u32]) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut guard = match children.lock() {
            Ok(g) => g,
            // Poisoned (a launcher thread panicked while holding it) —
            // nothing more we can sensibly wait on; let `main` return.
            Err(_) => return,
        };
        for pid in watch_pids {
            match guard.iter_mut().find(|c| c.id() == *pid) {
                // Still tracked — check whether it has exited.
                Some(child) => match child.try_wait() {
                    Ok(Some(_status)) => {
                        eprintln!("[dev] watched process {pid} exited; stopping.");
                        return;
                    }
                    Ok(None) => { /* still running */ }
                    Err(_) => return,
                },
                // Drained by the Ctrl-C handler; the process is on its way
                // out, so stop waiting.
                None => return,
            }
        }
    }
}

/// Compute which targets to launch.
///
/// - `--all` expands to every platform the host can build for (see
///   [`all_targets_for_host`]). Combines with any explicit
///   `--web` / `--ios` / `--android` / `--macos` flags as a union.
/// - Otherwise, if any per-platform flag is set, take that union.
/// - Otherwise, if `--ssr` / `--static` is set, return no platform
///   targets — the SSR/static HTTP server is the whole intent and
///   silently launching the manifest's mobile / web targets would
///   spawn emulators the user wasn't asking for.
/// - Otherwise, fall back to the manifest's declared `targets`.
/// - If everything is empty (and SSR/static aren't set), error — the
///   user has to declare somewhere what they want.
///
/// Roku is intentionally excluded from `--all` because it has no
/// dev-mode pipeline yet (see [`launch_target`]); spawning it would
/// just emit a launch-failed line for no reason.
fn resolve_targets(args: &Args, manifest_targets: &[Target]) -> Result<Vec<Target>> {
    let mut from_flags: Vec<Target> = Vec::new();
    if args.all {
        from_flags.extend(all_targets_for_host());
    }
    if args.web {
        from_flags.push(Target::Web);
    }
    if args.ios {
        from_flags.push(Target::Ios);
    }
    if args.android {
        from_flags.push(Target::Android);
    }
    if args.macos {
        from_flags.push(Target::Macos);
    }
    if args.terminal {
        from_flags.push(Target::Terminal);
    }

    if !from_flags.is_empty() {
        return Ok(dedup_preserve_order(from_flags));
    }
    // `--ssr` / `--static` are pseudo-targets (their own HTTP servers,
    // independent of the platform target dispatch). If the user passed
    // ONLY those flags, they want just the SSR/static server — falling
    // through to the manifest's declared targets would silently launch
    // emulators they didn't ask for.
    if args.ssr || args.static_only {
        return Ok(Vec::new());
    }
    if !manifest_targets.is_empty() {
        return Ok(dedup_preserve_order(manifest_targets.to_vec()));
    }
    anyhow::bail!(
        "no targets to run: pass `--all`, or `--web` / `--ios` / `--android` / \
         `--macos`, or add `targets = [\"web\", ...]` to \
         `[package.metadata.idealyst.app]`"
    )
}

/// Targets `--all` expands to. iOS and macOS toolchains only exist on
/// darwin, so we filter those out on other hosts rather than queueing
/// up workers that are guaranteed to fail. Roku is excluded because
/// dev-mode isn't wired for it.
fn all_targets_for_host() -> Vec<Target> {
    let mut targets = vec![Target::Web, Target::Android];
    if cfg!(target_os = "macos") {
        targets.push(Target::Ios);
        targets.push(Target::Macos);
    }
    targets
}

fn dedup_preserve_order(xs: Vec<Target>) -> Vec<Target> {
    let mut seen: HashSet<Target> = HashSet::new();
    let mut out = Vec::new();
    for x in xs {
        if seen.insert(x) {
            out.push(x);
        }
    }
    out
}

/// Build (or rebuild) the runtime-server host binary for this project. The host
/// is what serves the wire WebSocket; runs as a child process for the
/// rest of this session.
fn build_runtime_server_host(dir: &Path) -> Result<PathBuf> {
    crate::dlog!("dev", "building runtime-server host…");
    let source = crate::framework_source::resolve(dir)?;
    let artifact = build_runtime_server::build(
        dir,
        build_runtime_server::BuildOptions {
            release: false,
            source,
        },
    )?;
    Ok(artifact.host_binary)
}

/// Per-target launcher. Each variant handles its own runtime-server-vs-local
/// branching internally, then either blocks (web's static server) or
/// returns (iOS / Android, which fire-and-forget the device launch).
///
/// `children` is the shared Vec the Ctrl-C handler walks on
/// SIGINT — launchers that produce a `Child` (currently macOS in
/// background mode) push it here so the binary gets killed when
/// the dev session ends.
///
/// `runtime_server_port` is `Some(port)` in runtime-server mode (the
/// CLI has already waited for the host's port-file to land); each
/// per-platform launcher uses it to bake `IDEALYST_DEV_ENDPOINT` into
/// the wrapper build / spawn env.
fn launch_target(
    target: Target,
    dir: &Path,
    args: &Args,
    children: Arc<Mutex<Vec<Child>>>,
    macos_app_pid: Arc<Mutex<Option<u32>>>,
    runtime_server_port: Option<u16>,
) -> Result<()> {
    match target {
        Target::Web => launch_web(dir, args, runtime_server_port),
        Target::Ios => launch_ios(dir, args, runtime_server_port),
        Target::Android => launch_android(dir, args, runtime_server_port),
        Target::Roku => anyhow::bail!(
            "Roku has no dev-mode story yet; use `idealyst build roku` for the package"
        ),
        Target::Macos => launch_macos(dir, args, children, macos_app_pid, runtime_server_port),
        Target::Terminal => launch_terminal(dir, args, runtime_server_port),
    }
}

/// Terminal launcher — build the TTY binary via `build-terminal`,
/// then `run-terminal` spawns it inheriting stdio. Blocks until the
/// user quits the terminal app (Ctrl-C, `q`, etc.).
///
/// In runtime-server mode the wrapper drops the user crate and
/// connects to the dev-host over WebSocket like the other native
/// runtime-server clients. In local mode it mounts `app()` in
/// process — same shape as the macOS local path.
fn launch_terminal(dir: &Path, args: &Args, runtime_server_port: Option<u16>) -> Result<()> {
    let mode = if args.local {
        run_terminal::RunMode::Local
    } else {
        run_terminal::RunMode::RuntimeServer
    };
    crate::dlog!(
        "dev terminal",
        "building + launching TTY app (mode: {:?})…",
        mode,
    );
    let source = crate::framework_source::resolve(dir)?;
    // Terminal runs on the host machine, so loopback is always the right
    // address. Empty when in local mode.
    let endpoint = runtime_server_port.map(|p| format!("ws://127.0.0.1:{p}"));
    let env_vars = dev_env_vars(dir, args, &project_app_name(dir), None, endpoint.as_deref());
    let artifact = run_terminal::run(
        dir,
        run_terminal::RunOptions {
            release: false,
            mode,
            source,
            user_features: dev_user_features_other(),
            env_vars,
        },
    )
    .context("terminal dev launch failed")?;
    crate::dlog!("dev terminal", "exited ({})", artifact.binary.display());
    Ok(())
}

/// Web launcher.
///
/// - runtime-server mode (default): build the wasm bundle with the
///   `dev-hot-reload` feature, then start `web-dev-host` which serves
///   the bundle + reads the dev-server's port-file sentinel +
///   injects `window.IDEALYST_RUNTIME_SERVER_URL` into served HTML.
///   Source changes apply as hot-patches; the page survives saves.
/// - `--local` mode: build the wasm bundle without `dev-hot-reload`,
///   then serve via `dev-http::serve_static` with livereload polling.
///   A file watcher rebuilds the bundle on source change and bumps
///   the generation counter so the browser reloads (full page
///   reload — state does not survive saves).
fn launch_web(dir: &Path, args: &Args, runtime_server_port: Option<u16>) -> Result<()> {
    use dev_http::{serve_static, AasContext, PreloadContext, ReloadContext};

    let source = crate::framework_source::resolve(dir)?;

    // Full-stack projects (those declaring `server_bin` in their
    // manifest) bypass the built-in static-file dev server entirely —
    // their own server binary serves both the wasm bundle AND the
    // `/_srv/*` API. We build the wasm into `pkg/` first (one-shot, no
    // hot-reload yet for this path) then cargo-run the bin with
    // `--features server`. Watch + restart on save is a follow-up.
    let manifest = parse_manifest(dir)?;
    if let Some(server_bin) = manifest.app.server_bin.clone() {
        return launch_web_with_server_bin(dir, args, &source, &server_bin);
    }

    // `[package.metadata.idealyst.app.web].preload_fonts` from the
    // manifest. dev-http splices these into served HTML so the dev
    // loop matches what `build-web`'s `stage_bundle` ships in the
    // deployed bundle. Empty list = no preload tags, default behavior.
    let preload_ctx = (!manifest.app.web.preload_fonts.is_empty()).then(|| PreloadContext {
        font_paths: manifest.app.web.preload_fonts.clone(),
    });

    // Default index.html served when the project ships none, so
    // `idealyst dev --web` works without hand-authored boilerplate. A
    // project's own index.html still wins — dev-http only falls back to
    // this when the file is absent. Same generator `build-web` uses when
    // staging a deploy bundle, so dev and build behave identically.
    let fallback_index = build_web::default_index_html(&manifest.app.name, &manifest.lib_name);

    // Framework-managed dev assets — favicons today; other generated
    // dev-time outputs as they land. Lives under `target/idealyst/dev/web/`
    // by convention (matches what build writes to its staging dir, but
    // out of the way of the user's project tree). `dev-http` overlays
    // this on top of the project root so `/favicon.ico` resolves
    // without any committed file.
    //
    // In local mode we allocate the `ReloadSignal` up-front so the
    // icon-source watcher can bump it on SVG edits — same signal the
    // wasm rebuild watcher uses, so SSE clients reload on any change
    // regardless of origin. Runtime-server mode skips the watcher
    // (its hot-patch loop is separate; favicon updates wait for a
    // manual reload, which is a rare-edge concern).
    let local_signal = (args.local).then(dev_reload::ReloadSignal::new);
    let (overlay_ctx, head_ctx) = sync_dev_web_overlay(dir, local_signal.clone())?;

    if !args.local {
        // ── 1. wasm shim that connects to the runtime-server host ────────────
        if !args.no_build {
            crate::dlog!(
                "dev web",
                "building wasm shim with runtime-server + runtime-core/hot-reload…"
            );
            dev_reload::build_once(
                dir,
                &dev_reload::BuildOptions {
                    source: source.clone(),
                    // Two features flipped on for the wasm build:
                    //
                    // 1. `runtime-server` (bare feature) — wrapper-
                    //    local; switches the generated `start()` from
                    //    local `mount(app)` to a `WireBackend` +
                    //    `connect_web` against the URL injected as
                    //    `window.IDEALYST_RUNTIME_SERVER_URL`. Without
                    //    this the browser would render locally and
                    //    never open the WebSocket, so the runtime-
                    //    server sidecar would log `notifying 0
                    //    session(s) to re-render` on every hot-patch.
                    //
                    // 2. `runtime-core/hot-reload` (cross-crate) —
                    //    flips the `#[component]` macro into its
                    //    split form (`__<Name>_hot_impl` + outer
                    //    dispatch via `dev_hot::call`) on the
                    //    USER crate's compilation. Even though the
                    //    browser doesn't apply patches, the wire
                    //    protocol carries `HandlerId`s minted against
                    //    the hot-reload-aware handler table, so the
                    //    user crate must compile with the same
                    //    flavor as the runtime-server sidecar.
                    features: vec![
                        // Wrapper-local feature; MUST match the
                        // template's declaration or the
                        // `user_feature_forwards` filter would emit
                        // `runtime-server = ["<user>/runtime-server"]`
                        // and require every user crate to declare an
                        // unused `runtime-server` feature.
                        "runtime-server".to_string(),
                        "runtime-core/hot-reload".to_string(),
                    ],
                },
            )
            .context("web build failed (runtime-server + runtime-core/hot-reload)")?;
        }

        // The dev-server lives on this machine, so the browser always
        // reaches it at loopback. The CLI has the port (resolved
        // synchronously above before any platform launch); inject it
        // into the served HTML as `window.IDEALYST_RUNTIME_SERVER_URL`.
        let url = runtime_server_port.map(|p| format!("ws://127.0.0.1:{p}"));
        let aas_url = Arc::new(Mutex::new(url));

        let ctx = AasContext { aas_url };
        crate::dlog!(
            "dev web",
            "runtime-server-bridged HTTP at http://{}:{}",
            args.host, args.port
        );
        // Fire-and-forget browser open — matches the iOS sim
        // `open -a Simulator` UX. Spawned before `serve_static`
        // (which blocks forever) and TCP-polls until the bind lands
        // so we don't beat the server to the punch.
        spawn_browser_opener(&args.host, args.port);
        serve_static(
            &args.host,
            args.port,
            dir,
            None,
            Some(ctx),
            preload_ctx,
            overlay_ctx.clone(),
            head_ctx.clone(),
            Some(fallback_index.clone()),
        )?;
        Ok(())
    } else {
        // ── Local-render mode: livereload-driven hot-reload. ───────
        // Signal was allocated above so the icon-source watcher could
        // attach to it; reuse it for the wasm rebuild watcher and the
        // SSE stream so all change sources fan into one reload event.
        let signal = local_signal.expect("local_signal allocated for local mode");
        if !args.no_build {
            // `dev_reload::start_with` does the first build
            // synchronously and then keeps a watcher thread alive in
            // the returned handle. Forget the handle: it lives as
            // long as the HTTP serve loop below.
            //
            // `runtime-core/dev` is what activates the Robot
            // bridge auto-start + the MCP catalog inventory. It's
            // part of the dev configuration — not an opt-in — so
            // the MCP server can attach without any user action.
            let handle = dev_reload::start_with(
                dir,
                signal.clone(),
                dev_reload::BuildOptions {
                    source: source.clone(),
                    features: vec!["runtime-core/dev".to_string()],
                },
            )?;
            std::mem::forget(handle);

            // TODO(lazy-primitive): wasm-split-cli post-build step
            // for the local-mode dev path. Splits the wasm-pack
            // output into base + chunks, emits chunks into
            // <project>/pkg/. Mirrors the build path; coming up next.
        }
        let ctx = ReloadContext { signal };
        crate::dlog!(
            "dev web",
            "livereload HTTP at http://{}:{}",
            args.host, args.port
        );
        spawn_browser_opener(&args.host, args.port);
        serve_static(
            &args.host,
            args.port,
            dir,
            Some(ctx),
            None,
            preload_ctx,
            overlay_ctx,
            head_ctx,
            Some(fallback_index),
        )?;
        Ok(())
    }
}

/// Generate dev-time framework assets (favicons today) into
/// `<project>/target/idealyst/dev/web/` and produce the dev-http
/// contexts that overlay-serve them and splice the corresponding
/// `<head>` tags into served HTML. Returns `(None, None)` when the
/// project has no `[package.metadata.idealyst.app.icon]` block —
/// nothing to overlay, nothing to inject.
///
/// Also spawns an SVG-source watcher when `reload_signal` is
/// supplied: when the project's icon source files change, the
/// closure re-runs `sync_web_icons` (cache busts on content change)
/// and bumps the signal so connected browsers reload. Detached
/// thread; runs for the lifetime of the dev server.
fn sync_dev_web_overlay(
    project_dir: &Path,
    reload_signal: Option<Arc<dev_reload::ReloadSignal>>,
) -> Result<(
    Option<dev_http::OverlayContext>,
    Option<dev_http::HeadInjectionContext>,
)> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        return Ok((None, None));
    };
    let block = config.resolved_for(icon_gen::Target::Web);
    let overlay_dir = project_dir
        .join("target")
        .join("idealyst")
        .join("dev")
        .join("web");
    if icon_gen::sync_web_icons(Some(&block), &overlay_dir)?.is_none() {
        return Ok((None, None));
    }
    crate::dlog!(
        "dev web",
        "icon overlay → {} (favicon.ico + 192/512/180)",
        overlay_dir.display()
    );

    // Spawn the SVG watcher. The block carries already-canonical
    // paths to the source/foreground SVGs; we hand them straight
    // to the watcher and rerun the same sync on change. The
    // closure rebuilds the resolved block fresh each time so a
    // mid-session manifest edit (e.g. swapping `foreground` to a
    // different file) is picked up too.
    if let Some(signal) = reload_signal {
        let mut watch_paths = Vec::new();
        if let Some(p) = block.source.as_ref() {
            watch_paths.push(p.clone());
        }
        if let Some(p) = block.foreground.as_ref() {
            watch_paths.push(p.clone());
        }
        // Also watch Cargo.toml so manifest-level changes (gradient
        // stops, padding, swapping the source path) trigger a
        // regen. dev-reload's main watcher already covers
        // Cargo.toml for code rebuilds, but it doesn't know to
        // also bump icons.
        watch_paths.push(project_dir.join("Cargo.toml"));
        let project_owned = project_dir.to_path_buf();
        let overlay_owned = overlay_dir.clone();
        let handle = dev_reload::start_watch(
            watch_paths,
            signal,
            "icon",
            move || {
                let cfg = icon_gen::load_config_from_manifest(&project_owned)?;
                let Some(cfg) = cfg else {
                    return Ok(());
                };
                let block = cfg.resolved_for(icon_gen::Target::Web);
                icon_gen::sync_web_icons(Some(&block), &overlay_owned)?;
                Ok(())
            },
        )?;
        // Detach: the watcher lives for the lifetime of the dev
        // server, same as dev-reload's main watcher handle.
        std::mem::forget(handle);
    }

    Ok((
        Some(dev_http::OverlayContext {
            roots: vec![overlay_dir],
        }),
        Some(dev_http::HeadInjectionContext {
            html: icon_gen::web_icon_link_tags(),
        }),
    ))
}

/// SSR launcher — builds the wasm bundle (so the SSR server can serve
/// it for hydration / serve fonts alongside), then builds the SSR
/// wrapper binary (via `build-ssr`) and spawns it. `static_only=true`
/// passes `--static` to the spawned binary, suppressing the boot
/// `<script>`. The wrapper binary itself reads the same args the
/// website's `examples/serve.rs` does, so the CLI just translates the
/// flag set; the wrapper handles the actual SSR loop.
///
/// Stays on the local-mode wasm bundle (no `aas`/`hot-reload`
/// features). Runtime-server-bridged hydration is conceptually out of
/// scope here — `--ssr` is for "render the site server-side and let
/// the in-page wasm hydrate it"; the runtime-server sidecar is a
/// different mode that doesn't pair naturally with per-request SSR.
fn launch_ssr(
    dir: &Path,
    args: &Args,
    static_only: bool,
    children: Arc<Mutex<Vec<Child>>>,
) -> Result<()> {
    let label = if static_only { "dev static" } else { "dev ssr" };
    let port = if static_only { args.static_port } else { args.ssr_port };
    let addr = format!("{}:{}", args.host, port);

    let source = crate::framework_source::resolve(dir)?;

    // Stage the bundle at `<project>/dist/web`. SSR (hydrate) needs
    // `/pkg/<lib>.js` so the page can boot; `--static` doesn't need
    // the JS but still needs the fonts in `<project>/dist/web/fonts/`
    // for the first paint to use the real typeface. One build covers
    // both — same flags as `idealyst build --web` (local-mode bundle,
    // no `aas` / `hot-reload`).
    let bundle_dir = dir.join("dist").join("web");
    if !args.no_build {
        crate::dlog!(label, "building wasm bundle (for hydration / fonts)…");
        let _ = build_web::build(
            dir,
            build_web::BuildOptions {
                release: false,
                source: source.clone(),
                user_features: Vec::new(),
                bundle_out_dir: Some(bundle_dir.clone()),
                gzip: false,
                strip_panics: false,
                // Always on in `dev --ssr` mode — the bundle is going
                // to hydrate the server's DOM.
                hydrate: true,
                // Dev builds skip data pruning — iteration speed beats
                // bundle size, and the heuristic adds a pass per
                // rebuild.
                prune_dead_data_min: None,
            },
        )
        .with_context(|| "wasm build for SSR mode failed")?;
    }

    // Build the native SSR wrapper binary.
    crate::dlog!(label, "building SSR wrapper binary…");
    let artifact = build_ssr::build(
        dir,
        build_ssr::BuildOptions {
            release: false,
            source: source.clone(),
            user_features: Vec::new(),
        },
    )
    .with_context(|| "SSR wrapper build failed")?;

    // Spawn the binary. It blocks accepting connections; tracking it
    // in `children` ensures Ctrl-C tears it down with the rest.
    let mut cmd = Command::new(&artifact.binary);
    cmd.arg("--addr").arg(&addr);
    cmd.arg("--static-dir").arg(&bundle_dir);
    if static_only {
        cmd.arg("--static");
    }
    crate::dlog!(
        label,
        "{} HTTP at http://{}",
        if static_only { "SSR (static)" } else { "SSR + hydration" },
        addr,
    );
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn SSR binary {}", artifact.binary.display()))?;
    children.lock().unwrap().push(child);

    // The worker thread blocks here until the process is killed (by
    // the Ctrl-C handler walking `children`). We wait by polling — the
    // `Child` itself is owned by `children` now and we can't take a
    // reference back out cleanly through the Mutex without a more
    // involved structure. Sleep-loop is fine; the loop exits when the
    // process dies and the dev process winds down.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

/// Full-stack web launcher. Used when the project's manifest sets
/// `[package.metadata.idealyst.app].server_bin = "<bin>"`.
///
/// The user's own binary serves both the API (`/_srv/*` via
/// `server::router()`) AND the static wasm bundle at `/` (via
/// `tower_http::services::ServeDir`). We:
///
/// 1. Build the wasm bundle into `pkg/` and `cargo run` the user's
///    server bin against the project with `--features server`.
/// 2. Use `dev_reload::start_with` to watch `src/` + `Cargo.toml`;
///    every successful rebuild bumps a generation counter.
/// 3. A polling loop on the main thread watches that counter and,
///    on each bump, kills the cargo child and respawns it. Cargo
///    sees its source changed and recompiles + reruns the server.
///    Each rebuild = one full server restart (no hot reload — the
///    server bin's process state is small enough that a kill-and-
///    respawn is the right shape).
fn launch_web_with_server_bin(
    dir: &Path,
    args: &Args,
    source: &build_ios::FrameworkSource,
    server_bin: &str,
) -> Result<()> {
    use std::time::Duration;

    let package = parse_manifest(dir)?.name;

    // Phase 1: initial wasm build + spawn the watcher. `start_with`
    // also runs the build before returning, so by the time we move
    // on, `pkg/` is populated and the watcher thread is live.
    let signal = dev_reload::ReloadSignal::new();
    if !args.no_build {
        crate::dlog!(
            "dev web",
            "full-stack: starting watcher for {} (server_bin = {})",
            dir.display(),
            server_bin,
        );
        let handle = dev_reload::start_with(
            dir,
            signal.clone(),
            dev_reload::BuildOptions {
                source: source.clone(),
                features: Vec::new(),
            },
        )
        .context("wasm initial build + watcher start failed")?;
        // Hand the watcher thread to the runtime — it lives as long
        // as the dev session. Dropping the JoinHandle here would NOT
        // stop the thread (it's a detached child), but `mem::forget`
        // makes the intent explicit and silences the unused warning.
        std::mem::forget(handle);
    }

    // Phase 2: spawn the server bin. Captured so we can kill + respawn
    // on each rebuild.
    let mut child = spawn_server_bin(&package, server_bin)?;
    crate::dlog!(
        "dev web",
        "full-stack: spawned `cargo run -p {} --bin {} --features server` (pid {})",
        package,
        server_bin,
        child.id(),
    );
    let mut last_gen = signal.current();

    // Phase 3: wait on the watcher's generation signal. dev_reload bumps
    // the counter (and notifies the condvar) after each successful build;
    // we block here until that happens or until the keepalive interval
    // elapses, at which point we re-check whether the server child died.
    //
    // 500ms keepalive == upper bound on how long it takes us to notice a
    // server-bin crash. Bumps wake us immediately.
    loop {
        let cur = signal.wait_past(last_gen, Duration::from_millis(500));
        if cur != last_gen {
            last_gen = cur;
            eprintln!(
                "[dev web] source change → restarting server bin `{}`",
                server_bin,
            );
            let _ = child.kill();
            let _ = child.wait();
            child = match spawn_server_bin(&package, server_bin) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[dev web] server respawn failed: {e:#}");
                    // Try again on the next gen bump rather than
                    // tearing down the whole dev session.
                    continue;
                }
            };
            continue;
        }

        // Did the server bin exit on its own (panic, port-in-use,
        // ...)?  Surface the exit code and stop the dev session.
        if let Some(status) = child.try_wait().ok().flatten() {
            anyhow::bail!(
                "server bin `{}` exited with {} — fix the issue and re-run `idealyst dev --web`",
                server_bin,
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into())
            );
        }
    }
}

/// Spawn `cargo run -p <pkg> --bin <bin> --features server`, inheriting
/// stdio so the user sees the server's log lines in real time.
/// Returns the child for kill-on-restart bookkeeping.
fn spawn_server_bin(package: &str, server_bin: &str) -> Result<std::process::Child> {
    std::process::Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg(package)
        .arg("--bin")
        .arg(server_bin)
        .arg("--features")
        .arg("server")
        .spawn()
        .with_context(|| format!("spawn `cargo run -p {} --bin {}`", package, server_bin))
}

/// Open the browser at the project's web URL once the server is
/// actually accepting connections. Background-threaded because
/// `serve_static` blocks the caller.
///
/// Host translation: the server may bind `0.0.0.0` / `::` to expose
/// over LAN, but the *connect* address has to be loopback —
/// `http://0.0.0.0:…` doesn't resolve in any browser. We rewrite any
/// wildcard host to `localhost`.
///
/// If `open` (macOS) / `xdg-open` (Linux) / `start` (Windows) isn't
/// available, or the TCP poll times out, we exit silently — the URL
/// is already logged above, so the user can click that.
fn spawn_browser_opener(host: &str, port: u16) {
    let connect_host = match host {
        "0.0.0.0" | "::" | "[::]" => "localhost".to_string(),
        other => other.to_string(),
    };
    let url = format!("http://{}:{}", connect_host, port);
    std::thread::spawn(move || {
        // Poll until the listener is up. Short cap — the bind is
        // synchronous from the spawning thread's perspective so this
        // usually resolves in <50 ms.
        use std::net::TcpStream;
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_secs(5);
        let probe_addr = format!("127.0.0.1:{port}");
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(
                &probe_addr.parse().expect("valid socket addr"),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        open_url_in_browser(&url);
    });
}

fn open_url_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let (cmd, args): (&str, Vec<&str>) = ("", vec![url]);

    if cmd.is_empty() {
        return;
    }
    let _ = Command::new(cmd)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// iOS launcher. Reuses the `run-ios` crate's pipeline:
/// builds the staticlib (with or without the runtime-server shell feature),
/// drops it into an Xcode bundle, and launches the simulator.
///
/// Default (runtime-server) is the live path: the dev-server handles
/// reload on source change, the iOS app re-renders automatically.
/// `--local` does a one-shot build + run; the user restarts when they
/// want a new build.
fn launch_ios(dir: &Path, args: &Args, runtime_server_port: Option<u16>) -> Result<()> {
    let mode = if args.local {
        run_ios::RunMode::Local
    } else {
        // iOS simulator shares the host's localhost, so loopback is
        // the right address; the CLI bakes the URL into Info.plist's
        // `IdealystDevEndpoint`. (Physical device builds would use
        // `resolve_lan_ip()` here, but those aren't wired yet.)
        let endpoint = runtime_server_port
            .map(|p| format!("ws://127.0.0.1:{p}"))
            .unwrap_or_default();
        run_ios::RunMode::RuntimeServer { endpoint }
    };
    crate::dlog!("dev ios", "building + launching simulator (mode: {:?})…", mode);
    let source = crate::framework_source::resolve(dir)?;
    let artifact = run_ios::run(
        dir,
        run_ios::RunOptions {
            release: false,
            mode,
            source,
            user_features: dev_user_features_other(),
            // The dev loop relaunches on every change; the default
            // terminate-before-install (inside `run`) already keeps the
            // simulator from re-foregrounding a stale process. A full
            // uninstall would wipe app state on every reload, so keep it off.
            clean: false,
        },
    )
    .context("iOS dev launch failed")?;
    crate::dlog!(
        "dev ios",
        "running on simulator {} ({})",
        artifact.simulator_udid,
        artifact.app_bundle.display(),
    );
    Ok(())
}

/// Android launcher. Same shape as [`launch_ios`]: builds the cdylib
/// + Java glue + APK via `run-android`, installs to a connected
/// emulator (booting one if none is online), launches the app.
///
/// runtime-server mode swaps the cdylib (backend-android with
/// `runtime-server`) and the Java sources (MainActivity reads
/// `IdealystRuntimeServerUrl` from manifest meta-data and runs a
/// `Handler` tick into `drainRuntimeServer`). Local mode keeps the
/// in-process mount path.
///
/// The Android emulator's QEMU NAT can't reach the host's
/// `0.0.0.0:<port>` directly, but `adb reverse tcp:<port> tcp:<port>`
/// forwards the host port into the emulator's loopback. The CLI
/// sets that up inside `run-android` and bakes
/// `ws://127.0.0.1:<port>` into the APK manifest. Physical devices
/// over USB pick up the same tunnel for free.
fn launch_android(dir: &Path, args: &Args, runtime_server_port: Option<u16>) -> Result<()> {
    let mode = if args.local {
        run_android::RunMode::Local
    } else {
        run_android::RunMode::RuntimeServer
    };

    crate::dlog!(
        "dev android",
        "building + launching emulator (mode: {:?}{}…",
        mode,
        match runtime_server_port {
            Some(p) => format!(", port={p})"),
            None => ")".to_string(),
        }
    );
    let source = crate::framework_source::resolve(dir)?;
    let artifact = run_android::run(
        dir,
        run_android::RunOptions {
            release: false,
            avd: None,
            mode,
            runtime_server_port,
            source,
            user_features: dev_user_features_other(),
        },
    )
    .context("Android dev launch failed")?;
    crate::dlog!(
        "dev android",
        "running on {} ({})",
        artifact.serial,
        artifact.apk.display(),
    );
    Ok(())
}

/// macOS launcher — build the AppKit-backed binary via
/// `build-macos`, then `run-macos` spawns it. No simulator step
/// (we're already on macOS) and no runtime-server shell yet — the macOS
/// backend's first iteration is local-render only (see
/// `docs/macos-backend-plan.md`).
fn launch_macos(dir: &Path, args: &Args, children: Arc<Mutex<Vec<Child>>>, macos_app_pid: Arc<Mutex<Option<u32>>>, runtime_server_port: Option<u16>) -> Result<()> {
    let mode = if args.local {
        run_macos::RunMode::Local
    } else {
        run_macos::RunMode::RuntimeServer
    };
    let build_mode = if args.local {
        build_macos::BuildMode::Local
    } else {
        build_macos::BuildMode::RuntimeServer
    };
    crate::dlog!(
        "dev macos",
        "building + launching native AppKit app (mode: {:?})…",
        mode
    );
    let source = crate::framework_source::resolve(dir)?;

    // Build first so we have the wrapper binary path to pass to the
    // launcher as `IDEALYST_CATALOG_BIN`. The build crate's
    // `build` returns the artifact without running it. runtime-server mode
    // builds the runtime-server wrapper (no user-crate dep); local mode builds
    // the standard mount wrapper.
    let built = build_macos::build(
        dir,
        build_macos::BuildOptions {
            release: false,
            mode: build_mode,
            source: source.clone(),
            user_features: dev_user_features_macos(),
            universal: false, // dev: fast host-arch build
        },
    )
    .context("macOS dev build failed")?;

    let app_name = project_app_name(dir);
    pre_launch_clear_registry(dir);

    // Now launch. The launcher pipes `IDEALYST_CATALOG_BIN` etc. to
    // the spawned app so its bridge auto-start can register the
    // catalog binary in the registry alongside the bridge address.
    // macOS app runs on this host; loopback always reaches the
    // dev-server. Pass the endpoint as an env var the wrapper reads
    // via `runtime_server_shell_native::endpoint_or_panic()`.
    let endpoint = runtime_server_port.map(|p| format!("ws://127.0.0.1:{p}"));
    let mut env_vars = dev_env_vars(dir, args, &app_name, Some(&built.binary), endpoint.as_deref());
    // Tell the app which process launched it, so its launcher-watchdog
    // exits the app when this CLI dies (incl. SIGKILL). Paired with the
    // app-pid wait in the tail of `run` for the reverse direction.
    env_vars.push((
        "IDEALYST_LAUNCHER_PID".to_string(),
        std::process::id().to_string(),
    ));
    let artifact = run_macos::run(
        dir,
        run_macos::RunOptions {
            release: false,
            mode,
            source,
            background: true,
            user_features: dev_user_features_macos(),
            env_vars,
        },
    )
    .context("macOS dev launch failed")?;
    crate::dlog!("dev macos", "running detached ({})", artifact.binary.display());

    // Track the spawned macOS Child so the Ctrl-C handler can kill
    // it on exit. Pre-fix, `cmd.spawn()` returned a `Child` that
    // was immediately dropped, leaving the binary orphaned to
    // `init` (PID 1) — every `idealyst dev --macos` session piled
    // up one zombie `<project>-macos[-aas]` process per
    // invocation, with no way to reach them short of `killall -9`.
    if let Some(child) = artifact.child {
        // Record the app pid before moving the handle into `children`, so
        // the tail wait can detect window-close → app-exit and tear down.
        *macos_app_pid.lock().unwrap() = Some(child.id());
        children.lock().unwrap().push(child);
    }

    // Legacy: write `.idealyst/catalog.path` too. Kept for back-
    // compat with `bridge_discovery` path-walking callers; the
    // registry is now the canonical source.
    write_catalog_path(dir, &artifact.binary);
    // Record so the Ctrl-C handler can deregister.
    track_project_root(dir);
    Ok(())
}

/// Write `<project>/.idealyst/catalog.path` containing the absolute
/// path to a binary that supports `--emit-catalog`. The MCP server
/// discovers this file via `bridge_discovery` and auto-applies it as
/// `--from-bin`. Best-effort — write failures are logged but don't
/// fail the dev launch (the catalog tools will just return empty).
fn write_catalog_path(project_dir: &Path, binary: &Path) {
    let target = project_dir.join(".idealyst").join("catalog.path");
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[dev] could not create {}: {}", parent.display(), e);
            return;
        }
    }
    // Canonicalize the binary path so the MCP server doesn't have to
    // resolve relative paths from a different cwd.
    let canon = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    if let Err(e) = std::fs::write(&target, canon.to_string_lossy().as_bytes()) {
        eprintln!("[dev] could not write {}: {}", target.display(), e);
    }
}

/// Path the runtime-server host writes its bound port to. Lives next to the
/// wrapper crate's Cargo.toml so it's automatically scoped per
/// project and gets wiped along with `target/idealyst/` when the
/// user runs `cargo clean`.
fn runtime_server_port_file(project_dir: &Path) -> PathBuf {
    // Mirror `build-runtime-server`'s wrapper dir layout. The wrapper itself
    // lives at `target/idealyst/<project>/runtime-server/host/`; we drop
    // the sentinel one level up so it's discoverable even if the wrapper
    // gets regenerated mid-session.
    // Resolve project name from the project dir's basename — same
    // shape `build-runtime-server::build` uses to compute the wrapper path.
    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    // runtime-server dev mode requires the framework workspace anyway (the
    // hot-patch builder + sidecar templating live there). Fall back
    // to the project dir if we somehow can't find a workspace — the
    // caller is dev-only, so the worst case is the sentinel never
    // appears and we time out cleanly.
    let workspace_root = build_ios::require_workspace_root(project_dir)
        .unwrap_or_else(|_| project_dir.to_path_buf());
    workspace_root
        .join("target/idealyst")
        .join(project_name)
        .join("runtime-server")
        .join("host-port")
}

/// Poll the runtime-server host's port sentinel file. The host writes
/// its bound port there once `TcpListener::bind` succeeds; the CLI
/// reads it once before launching any platform and bakes the result
/// into every wrapper's `IDEALYST_DEV_ENDPOINT`.
///
/// Returns `None` on timeout — caller bails with a clear error since
/// there's no fallback discovery path.
fn read_host_port_file(path: &Path, timeout: std::time::Duration) -> Option<u16> {
    use std::time::Instant;
    eprintln!("[dev] reading host port from {}", path.display());
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(p) = s.trim().parse::<u16>() {
                eprintln!("[dev] host bound port = {p}");
                return Some(p);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!(
        "[dev] no port written to {} within {:?}",
        path.display(),
        timeout
    );
    None
}

/// Install the global Ctrl-C handler. Walks the shared `children`
/// list and kills each child before exiting. Per-process app
/// registrations clean themselves up via RAII on the bridge side; no
/// extra teardown pass needed here.
fn install_ctrlc_handler(children: Arc<Mutex<Vec<Child>>>) -> Result<()> {
    ctrlc::set_handler(move || {
        eprintln!("\n[dev] received Ctrl-C — stopping…");
        if let Ok(mut guard) = children.lock() {
            for mut child in guard.drain(..) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        std::process::exit(0);
    })
    .context("install Ctrl-C handler")?;
    Ok(())
}

fn track_project_root(_dir: &Path) {
    // No-op: bridge.rs writes `~/.idealyst/apps/<name>-<pid>.json` on
    // start and removes it via RAII on graceful shutdown, so there's
    // no separate registry for the CLI to clean up.
}

impl Args {
    /// Cheap clone of the bits each per-target worker needs. We
    /// don't `derive(Clone)` because `PathBuf` already clones cheaply
    /// and the rest is `Copy` / small `String`s — a tiny manual
    /// clone keeps it explicit which fields cross the thread
    /// boundary.
    fn shallow_clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            local: self.local,
            web: self.web,
            ios: self.ios,
            android: self.android,
            macos: self.macos,
            terminal: self.terminal,
            all: self.all,
            ssr: self.ssr,
            static_only: self.static_only,
            port: self.port,
            ssr_port: self.ssr_port,
            static_port: self.static_port,
            host: self.host.clone(),
            no_build: self.no_build,
            bridge_port: self.bridge_port,
            interactive: self.interactive,
        }
    }
}

/// Dup2-based stderr redirect for the duration of an interactive
/// dev session, installed before any worker thread spawns so cargo
/// subprocess stderr lands in a file instead of corrupting the TTY
/// host-terminal is about to paint into.
///
/// Mirrors `host_terminal::stderr_redirect::StderrRedirect`. Kept here
/// (rather than reusing that one) so the CLI doesn't need a public
/// dependency on host-terminal's internals. On drop, restores the
/// saved original fd 2 — leaving the inner host-terminal redirect's
/// own save/restore chain intact when the TUI exits.
struct EarlyStderrRedirect {
    #[cfg(unix)]
    saved_fd: std::os::raw::c_int,
}

impl EarlyStderrRedirect {
    fn install(log_path: &Path) -> Self {
        #[cfg(unix)]
        unsafe {
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let saved_fd = libc::dup(libc::STDERR_FILENO);
            if saved_fd < 0 {
                return Self { saved_fd: -1 };
            }
            let c_path = match std::ffi::CString::new(log_path.to_string_lossy().as_bytes()) {
                Ok(p) => p,
                Err(_) => {
                    libc::close(saved_fd);
                    return Self { saved_fd: -1 };
                }
            };
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
            let mode: libc::mode_t = 0o644;
            let log_fd = libc::open(c_path.as_ptr(), flags, mode as std::os::raw::c_int);
            if log_fd < 0 {
                libc::close(saved_fd);
                return Self { saved_fd: -1 };
            }
            if libc::dup2(log_fd, libc::STDERR_FILENO) < 0 {
                libc::close(log_fd);
                libc::close(saved_fd);
                return Self { saved_fd: -1 };
            }
            libc::close(log_fd);
            Self { saved_fd }
        }
        #[cfg(not(unix))]
        {
            let _ = log_path;
            Self {}
        }
    }
}

impl Drop for EarlyStderrRedirect {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if self.saved_fd >= 0 {
                libc::dup2(self.saved_fd, libc::STDERR_FILENO);
                libc::close(self.saved_fd);
                self.saved_fd = -1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Regression guard for the "android blank screen" bug: in
    /// runtime-server mode the CLI must keep running while the dev-host
    /// child serves. `wait_for_host_exit` is the kernel of that — it
    /// must block until the host process actually ends, NOT return
    /// immediately (the pre-fix behavior, which orphaned the host and
    /// let its watchdog SIGKILL it out from under a freshly-launched
    /// app). A tighter test of the full `dev --android` flow isn't
    /// reachable here (it needs adb + an emulator), so we pin the
    /// helper's contract directly.
    #[test]
    fn wait_for_host_exit_blocks_until_host_process_ends() {
        // Stand-in "host" that exits on its own after a short delay.
        let child = Command::new("sleep")
            .arg("0.4")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(vec![child]));

        let start = Instant::now();
        wait_for_host_exit(&children, pid);
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(250),
            "must poll until the host exits, not return instantly (returned in {elapsed:?}) — \
             returning early is exactly the bug that orphaned the dev-host",
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "must return promptly once the host exits (took {elapsed:?})",
        );
    }

    /// If the Ctrl-C handler has already drained `children` (the
    /// process is exiting), the host pid is no longer tracked — the
    /// wait must return on the first poll rather than hang forever.
    #[test]
    fn wait_for_host_exit_returns_when_host_not_tracked() {
        let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(Vec::new()));
        let start = Instant::now();
        wait_for_host_exit(&children, 999_999);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "a missing host pid must return promptly, not hang",
        );
    }
}
