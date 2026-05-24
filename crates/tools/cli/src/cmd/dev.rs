//! `idealyst dev` — orchestrate the dev pipeline for one or more
//! platform targets.
//!
//! Two orthogonal axes:
//!
//! - **Mode**: local-render (default) or runtime-server (`--aas`).
//!   - Local-render: each platform builds the user's `app()` for itself
//!     with a file-watcher rebuild loop. Web uses livereload; native
//!     platforms restart the app on rebuild.
//!   - runtime-server: a single dev-server process runs the user's reactive
//!     runtime; every platform's client connects over a WebSocket
//!     and renders whatever wire commands arrive. Source changes
//!     only rebuild the dev-host binary, the navigator stack
//!     survives, every client stays in sync.
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
/// mDNS discovery handles app→server routing now, so the only thing
/// we still pass is the optional pinned bridge port (rare — most
/// users let the bridge pick ephemeral).
fn dev_env_vars(
    project_dir: &Path,
    args: &Args,
    _app_name: &str,
    _catalog_bin: Option<&Path>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dev_cfg = crate::dev_config::DevConfig::load(project_dir).unwrap_or_default();
    let pinned = args.bridge_port.or(dev_cfg.bridge_port);
    if let Some(p) = pinned {
        out.push(("IDEALYST_BRIDGE_PORT".to_string(), p.to_string()));
    }
    out
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

/// Registry-cleanup hook from the pre-mDNS era — no-op now that
/// discovery runs through Bonjour. Kept as a function (rather than
/// deleting at all call sites) so the diff stays focused; can be
/// inlined / removed in a follow-up.
fn pre_launch_clear_registry(_project_dir: &Path) {}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Run the runtime-server: a single dev process holds the user's
    /// reactive tree, every platform client connects over WebSocket
    /// and renders wire commands. The default (off) runs the app
    /// natively on each platform with its own rebuild loop.
    ///
    /// `--aas` is accepted as a deprecated alias for one release —
    /// originally the mode was called "runtime-server" (application-as-a-server)
    /// before the project's runtime-server rename. Remove the alias
    /// once external scaffolds stop referencing it.
    #[arg(long, alias = "aas")]
    pub runtime_server: bool,

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

    /// HTTP port for the web target's static-file server.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

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

    eprintln!(
        "[dev] {} mode, targets: {}",
        if args.runtime_server { "runtime-server" } else { "local" },
        active_targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );

    // Child handles for cleanup-on-Ctrl-C. Each platform launcher
    // pushes any subprocesses it spawns here; the signal handler
    // walks the vec and kills everything.
    let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(Vec::new()));
    install_ctrlc_handler(children.clone())?;

    // In runtime-server mode, start the dev-server once before launching any
    // platform — all clients connect to the same server. The host
    // self-execs on source change so we don't need to restart it.
    //
    // We point the host at a sentinel file via
    // `IDEALYST_RUNTIME_SERVER_PORT_FILE`; it writes its bound port there so
    // platform launchers (Android in particular) can pick the
    // current port up reliably even when stale mDNS records linger.
    if args.runtime_server {
        let host_binary = build_runtime_server_host(&dir)?;
        let port_file = aas_port_file(&dir);
        // Clear any stale value from a previous session before
        // letting the host overwrite it — keeps reads from picking
        // up the previous run's number during the bind window.
        let _ = std::fs::remove_file(&port_file);
        let child = Command::new(&host_binary)
            .env("IDEALYST_RUNTIME_SERVER_PORT_FILE", &port_file)
            .spawn()
            .with_context(|| {
                format!(
                    "spawn runtime-server host {} — build succeeded but the binary won't run",
                    host_binary.display(),
                )
            })?;
        eprintln!(
            "[dev] runtime-server host running ({}), mDNS-advertised; port file {}",
            host_binary.display(),
            port_file.display(),
        );
        children.lock().unwrap().push(child);
    }

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
            std::thread::spawn(move || {
                if let Err(e) = launch_target(target, &dir, &args_clone, children_for_worker) {
                    eprintln!("[dev {}] launch failed: {e:#}", target);
                }
            });
        }
        // Run terminal foreground on the main thread; blocks until
        // the user quits. Ctrl-C while in the terminal app falls
        // through to crossterm, not our handler — but when the app
        // exits, control returns here and we drop into the children-
        // kill loop below.
        if let Err(e) = launch_target(Target::Terminal, &dir, &args, children.clone()) {
            eprintln!("[dev terminal] launch failed: {e:#}");
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
        let worker = std::thread::spawn(move || {
            if let Err(e) = launch_target(target, &dir, &args_clone, children_for_worker) {
                eprintln!("[dev {}] launch failed: {e:#}", target);
            }
        });
        workers.push(worker);
    }

    // Wait for all workers to settle. In practice the web launcher
    // is a foreground HTTP serve loop that blocks forever — so this
    // join effectively waits for Ctrl-C, which terminates the process
    // via the handler installed above. We still `.join` so any
    // background-only target (iOS launch + return) doesn't make us
    // exit immediately when its worker finishes.
    for w in workers {
        let _ = w.join();
    }

    Ok(())
}

/// Compute which targets to launch.
///
/// - `--all` expands to every platform the host can build for (see
///   [`all_targets_for_host`]). Combines with any explicit
///   `--web` / `--ios` / `--android` / `--macos` flags as a union.
/// - Otherwise, if any per-platform flag is set, take that union.
/// - Otherwise, fall back to the manifest's declared `targets`.
/// - If everything is empty, error — the user has to declare
///   somewhere what they want.
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
    eprintln!("[dev] building runtime-server host…");
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
fn launch_target(
    target: Target,
    dir: &Path,
    args: &Args,
    children: Arc<Mutex<Vec<Child>>>,
) -> Result<()> {
    match target {
        Target::Web => launch_web(dir, args),
        Target::Ios => launch_ios(dir, args),
        Target::Android => launch_android(dir, args),
        Target::Roku => anyhow::bail!(
            "Roku has no dev-mode story yet; use `idealyst build roku` for the package"
        ),
        Target::Macos => launch_macos(dir, args, children),
        Target::Terminal => launch_terminal(dir, args),
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
fn launch_terminal(dir: &Path, args: &Args) -> Result<()> {
    let mode = if args.runtime_server {
        run_terminal::RunMode::RuntimeServer
    } else {
        run_terminal::RunMode::Local
    };
    eprintln!(
        "[dev terminal] building + launching TTY app (mode: {:?})…",
        mode,
    );
    let source = crate::framework_source::resolve(dir)?;
    let artifact = run_terminal::run(
        dir,
        run_terminal::RunOptions {
            release: false,
            mode,
            source,
            user_features: dev_user_features_other(),
            env_vars: Vec::new(),
        },
    )
    .context("terminal dev launch failed")?;
    eprintln!("[dev terminal] exited ({})", artifact.binary.display());
    Ok(())
}

/// Web launcher.
///
/// - runtime-server mode: build the wasm bundle with the `dev-hot-reload`
///   feature, then start `web-dev-host` which serves the bundle +
///   discovers the runtime-server server via Bonjour + injects
///   `window.IDEALYST_RUNTIME_SERVER_URL` into served HTML.
/// - Local mode: build the wasm bundle without `dev-hot-reload`,
///   then serve via `dev-http::serve_static` with livereload
///   polling. A file watcher rebuilds the bundle on source change
///   and bumps the generation counter so the browser reloads.
fn launch_web(dir: &Path, args: &Args) -> Result<()> {
    use dev_http::{serve_static, AasContext, ReloadContext};

    let source = crate::framework_source::resolve(dir)?;

    if args.runtime_server {
        // ── 1. wasm shim that connects to the runtime-server host ────────────
        if !args.no_build {
            eprintln!(
                "[dev web] building wasm shim with aas + runtime-core/hot-reload…"
            );
            dev_reload::build_once(
                dir,
                &dev_reload::BuildOptions {
                    source: source.clone(),
                    // Two features flipped on for the wasm build:
                    //
                    // 1. `aas` (bare feature) — wrapper-local;
                    //    switches the generated `start()` from local
                    //    `mount(app)` to a `WireBackend` +
                    //    `connect_web` against `window.IDEALYST_RUNTIME_SERVER_URL`.
                    //    Without this the browser would render
                    //    locally and never open the WebSocket, so
                    //    the runtime-server sidecar would log
                    //    `notifying 0 session(s) to re-render` on
                    //    every hot-patch.
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
                        "runtime-server".to_string(),
                        "runtime-core/hot-reload".to_string(),
                    ],
                },
            )
            .context("web build failed (aas + runtime-core/hot-reload)")?;
        }

        // ── 2. mDNS browser thread fills `AasContext.aas_url` so
        //       the HTTP layer can inject `window.IDEALYST_RUNTIME_SERVER_URL`
        //       into served pages.
        let app_id = parse_manifest(dir)?.app.require_bundle_id()?.to_string();
        let aas_url = Arc::new(Mutex::new(None));
        spawn_aas_browser(app_id, aas_url.clone());

        let ctx = AasContext { aas_url };
        eprintln!(
            "[dev web] runtime-server-bridged HTTP at http://{}:{}",
            args.host, args.port
        );
        // Fire-and-forget browser open — matches the iOS sim
        // `open -a Simulator` UX. Spawned before `serve_static`
        // (which blocks forever) and TCP-polls until the bind lands
        // so we don't beat the server to the punch.
        spawn_browser_opener(&args.host, args.port);
        serve_static(&args.host, args.port, dir, None, Some(ctx))?;
        Ok(())
    } else {
        // ── Local-render mode: livereload-driven hot-reload. ───────
        let gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
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
                gen.clone(),
                dev_reload::BuildOptions {
                    source: source.clone(),
                    features: vec!["runtime-core/dev".to_string()],
                },
            )?;
            std::mem::forget(handle);
        }
        let ctx = ReloadContext { gen };
        eprintln!(
            "[dev web] livereload HTTP at http://{}:{}",
            args.host, args.port
        );
        spawn_browser_opener(&args.host, args.port);
        serve_static(&args.host, args.port, dir, Some(ctx), None)?;
        Ok(())
    }
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
/// Live local-mode hot reload isn't wired yet — `--ios` without
/// `--aas` does a one-shot build + run; the user restarts when
/// they want a new build. runtime-server mode is the live path: the dev-server
/// already handles reload on source change, the iOS app re-renders
/// automatically.
fn launch_ios(dir: &Path, args: &Args) -> Result<()> {
    let mode = if args.runtime_server {
        run_ios::RunMode::RuntimeServer
    } else {
        run_ios::RunMode::Local
    };
    eprintln!("[dev ios] building + launching simulator (mode: {:?})…", mode);
    let source = crate::framework_source::resolve(dir)?;
    let artifact = run_ios::run(
        dir,
        run_ios::RunOptions {
            release: false,
            mode,
            source,
            user_features: dev_user_features_other(),
        },
    )
    .context("iOS dev launch failed")?;
    eprintln!(
        "[dev ios] running on simulator {} ({})",
        artifact.simulator_udid,
        artifact.app_bundle.display(),
    );
    Ok(())
}

/// Android launcher. Same shape as [`launch_ios`]: builds the cdylib
/// + Java glue + APK via `run-android`, installs to a connected
/// emulator (booting one if none is online), launches the app.
///
/// runtime-server mode swaps the cdylib (backend-android with `runtime-server`) and
/// the Java sources (MainActivity reads `IdealystAppId` from manifest
/// meta-data, acquires a MulticastLock so mDNS browse works on
/// Wi-Fi, runs a `Handler` tick into `drainRuntimeServer`). Local mode keeps
/// the in-process mount path.
fn launch_android(dir: &Path, args: &Args) -> Result<()> {
    let mode = if args.runtime_server {
        run_android::RunMode::RuntimeServer
    } else {
        run_android::RunMode::Local
    };

    // In runtime-server mode the Android emulator's QEMU NAT prevents Bonjour
    // from seeing the host's mDNS broadcasts, so we discover the
    // host's port *on the Mac side* and pass it through to
    // `run-android`, which sets up `adb reverse tcp:<port>` and
    // bakes the override URL into the APK manifest. Physical
    // devices on the same Wi-Fi go through the same code path
    // safely — adb reverse over USB works the same way, and the
    // resulting `ws://127.0.0.1:<port>` URL hits the host's port
    // either via the USB tunnel (device) or via QEMU's localhost
    // forwarding (emulator).
    //
    // We read the port from the sentinel file the host writes
    // (path supplied via `IDEALYST_RUNTIME_SERVER_PORT_FILE` in `run`).
    // Sidesteps macOS's mDNS cache, which often holds onto stale
    // entries from previously-killed hosts.
    let aas_port = if args.runtime_server {
        read_host_port_file(&aas_port_file(dir), std::time::Duration::from_secs(10))
    } else {
        None
    };

    eprintln!(
        "[dev android] building + launching emulator (mode: {:?}{}…",
        mode,
        match aas_port {
            Some(p) => format!(", aas_port={p})"),
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
            runtime_server_port: aas_port,
            source,
            user_features: dev_user_features_other(),
        },
    )
    .context("Android dev launch failed")?;
    eprintln!(
        "[dev android] running on {} ({})",
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
fn launch_macos(dir: &Path, args: &Args, children: Arc<Mutex<Vec<Child>>>) -> Result<()> {
    let mode = if args.runtime_server {
        run_macos::RunMode::RuntimeServer
    } else {
        run_macos::RunMode::Local
    };
    let build_mode = if args.runtime_server {
        build_macos::BuildMode::RuntimeServer
    } else {
        build_macos::BuildMode::Local
    };
    eprintln!(
        "[dev macos] building + launching native AppKit app (mode: {:?})…",
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
        },
    )
    .context("macOS dev build failed")?;

    let app_name = project_app_name(dir);
    pre_launch_clear_registry(dir);

    // Now launch. The launcher pipes `IDEALYST_CATALOG_BIN` etc. to
    // the spawned app so its bridge auto-start can register the
    // catalog binary in the registry alongside the bridge address.
    let artifact = run_macos::run(
        dir,
        run_macos::RunOptions {
            release: false,
            mode,
            source,
            background: true,
            user_features: dev_user_features_macos(),
            env_vars: dev_env_vars(dir, args, &app_name, Some(&built.binary)),
        },
    )
    .context("macOS dev launch failed")?;
    eprintln!("[dev macos] running detached ({})", artifact.binary.display());

    // Track the spawned macOS Child so the Ctrl-C handler can kill
    // it on exit. Pre-fix, `cmd.spawn()` returned a `Child` that
    // was immediately dropped, leaving the binary orphaned to
    // `init` (PID 1) — every `idealyst dev --macos` session piled
    // up one zombie `<project>-macos[-aas]` process per
    // invocation, with no way to reach them short of `killall -9`.
    if let Some(child) = artifact.child {
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
fn aas_port_file(project_dir: &Path) -> PathBuf {
    // Mirror `build-runtime-server`'s wrapper dir layout. The wrapper itself
    // lives at `target/idealyst/<project>/aas/host/`; we drop the
    // sentinel one level up so it's discoverable even if the wrapper
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

/// Poll the runtime-server host's port sentinel file. The host writes its
/// bound port there once `TcpListener::bind` succeeds; we read it
/// here and feed it to `run-android` for the `adb reverse` tunnel
/// and the manifest's `IdealystRuntimeServerUrl` override.
///
/// Returns `None` on timeout. Caller falls back to the in-app
/// Bonjour path, which works for physical devices on the same Wi-Fi
/// as the dev Mac (just not for the QEMU-NAT emulator).
fn read_host_port_file(path: &Path, timeout: std::time::Duration) -> Option<u16> {
    use std::time::Instant;
    eprintln!("[dev android] reading host port from {}", path.display());
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(p) = s.trim().parse::<u16>() {
                eprintln!("[dev android] host bound port = {p}");
                return Some(p);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!(
        "[dev android] no port written to {} within {:?}; \
         falling back to in-app Bonjour",
        path.display(),
        timeout
    );
    None
}

/// Long-lived mDNS browser thread shared by the web launcher's runtime-server
/// mode. Writes the discovered `ws://...` URL into `out`; the HTTP
/// layer (via `AasContext`) reads it on every served page.
fn spawn_aas_browser(app_id: String, out: Arc<Mutex<Option<String>>>) {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    std::thread::spawn(move || {
        let Ok(daemon) = ServiceDaemon::new() else {
            eprintln!("[dev web] mDNS daemon init failed");
            return;
        };
        let Ok(receiver) = daemon.browse("_idealyst-dev._tcp.local.") else {
            eprintln!("[dev web] mDNS browse failed");
            return;
        };
        eprintln!(
            "[dev web] mDNS browsing for runtime-server dev-server with app_id={:?}",
            app_id
        );
        for event in receiver.iter() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let matches = info.get_properties().iter().any(|p| {
                        p.key().eq_ignore_ascii_case("app_id") && p.val_str() == app_id
                    });
                    if !matches {
                        continue;
                    }
                    let url = info
                        .get_addresses()
                        .iter()
                        .find(|a| a.is_ipv4())
                        .map(|a| format!("ws://{}:{}", a, info.get_port()));
                    if let Some(u) = url {
                        eprintln!("[dev web] discovered runtime-server at {u}");
                        if let Ok(mut g) = out.lock() {
                            *g = Some(u);
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, _) => {
                    if let Ok(mut g) = out.lock() {
                        *g = None;
                    }
                }
                _ => {}
            }
        }
    });
}

/// Install the global Ctrl-C handler. Walks the shared `children`
/// list and kills each child before exiting. Also drops the launched
/// project's registry entry so the MCP server doesn't see ghost
/// apps after the dev session ends. macOS detached-spawn case isn't
/// tracked in `children` (the dev process forgot about it), so the
/// registry deregistration uses `project_root` as the key — the
/// dev launcher records that here on registration.
fn install_ctrlc_handler(children: Arc<Mutex<Vec<Child>>>) -> Result<()> {
    ctrlc::set_handler(move || {
        eprintln!("\n[dev] received Ctrl-C — stopping…");
        // mDNS service-removed events fire when each child exits;
        // no registry-cleanup pass needed.
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
    // No-op now that mDNS replaces the registry — there's nothing
    // to clean up on Ctrl-C. Kept as a function so the call sites
    // don't all need to disappear in this diff.
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
            runtime_server: self.runtime_server,
            web: self.web,
            ios: self.ios,
            android: self.android,
            macos: self.macos,
            terminal: self.terminal,
            all: self.all,
            port: self.port,
            host: self.host.clone(),
            no_build: self.no_build,
            bridge_port: self.bridge_port,
        }
    }
}
