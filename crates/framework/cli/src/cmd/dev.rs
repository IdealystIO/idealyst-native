//! `idealyst dev` — orchestrate the dev pipeline for one or more
//! platform targets.
//!
//! Two orthogonal axes:
//!
//! - **Mode**: local-render (default) or AAS (`--aas`).
//!   - Local-render: each platform builds the user's `app()` for itself
//!     with a file-watcher rebuild loop. Web uses livereload; native
//!     platforms restart the app on rebuild.
//!   - AAS: a single dev-server process runs the user's reactive
//!     runtime; every platform's client connects over a WebSocket
//!     and renders whatever wire commands arrive. Source changes
//!     only rebuild the dev-host binary, the navigator stack
//!     survives, every client stays in sync.
//!
//! - **Targets**: `--web`, `--ios`, `--android`. If none are passed
//!   explicitly, the active set comes from `[package.metadata
//!   .idealyst.app].targets` in `Cargo.toml`.
//!
//! Multiple platforms run in parallel — each in its own thread —
//! and Ctrl-C tears all of them down together.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use build_ios::{parse_manifest, Target};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory containing the `Cargo.toml` (the same place
    /// you'd run `wasm-pack build` by hand for the web target).
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Run in Application-as-a-Server mode. A single dev-server
    /// process runs the user's reactive runtime; each platform's
    /// client connects over WebSocket and replays wire commands
    /// against its native backend. Default (off) is local-render
    /// mode where each platform builds + runs the user's app
    /// natively with its own file-watcher rebuild loop.
    #[arg(long)]
    pub aas: bool,

    /// Build + run the web target (browser).
    #[arg(long)]
    pub web: bool,

    /// Build + run the iOS target (simulator).
    #[arg(long)]
    pub ios: bool,

    /// Build + run the Android target. (Not wired yet — fails fast.)
    #[arg(long)]
    pub android: bool,

    /// HTTP port for the web target's static-file server.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Interface for the web target's static-file server. Loopback
    /// covers most cases; `0.0.0.0` exposes to the LAN (handy for
    /// testing the same bundle on a phone).
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Web local-mode only: skip the initial `wasm-pack build`. Use
    /// when the `pkg/` directory is already current and you just
    /// want a static server.
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
        if args.aas { "AAS" } else { "local" },
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

    // In AAS mode, start the dev-server once before launching any
    // platform — all clients connect to the same server. The host
    // self-execs on source change so we don't need to restart it.
    if args.aas {
        let host_binary = build_aas_host(&dir)?;
        let child = Command::new(&host_binary).spawn().with_context(|| {
            format!(
                "spawn AAS host {} — build succeeded but the binary won't run",
                host_binary.display(),
            )
        })?;
        eprintln!(
            "[dev] AAS host running ({}), mDNS-advertised",
            host_binary.display()
        );
        children.lock().unwrap().push(child);
    }

    // Spawn one worker thread per active target. We pre-build the
    // per-target context outside the thread so a setup error
    // surfaces synchronously.
    let mut workers = Vec::new();
    for target in &active_targets {
        let dir = dir.clone();
        let args_clone = args.shallow_clone();
        let target = *target;
        let worker = std::thread::spawn(move || {
            if let Err(e) = launch_target(target, &dir, &args_clone) {
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
/// - If any of `--web` / `--ios` / `--android` is set, take that
///   union (so `idealyst dev --web --ios` runs both, explicitly).
/// - Otherwise, fall back to the manifest's declared `targets`.
/// - If both are empty, error — the user has to declare somewhere
///   what they want.
fn resolve_targets(args: &Args, manifest_targets: &[Target]) -> Result<Vec<Target>> {
    let mut from_flags: Vec<Target> = Vec::new();
    if args.web {
        from_flags.push(Target::Web);
    }
    if args.ios {
        from_flags.push(Target::Ios);
    }
    if args.android {
        from_flags.push(Target::Android);
    }

    if !from_flags.is_empty() {
        return Ok(dedup_preserve_order(from_flags));
    }
    if !manifest_targets.is_empty() {
        return Ok(dedup_preserve_order(manifest_targets.to_vec()));
    }
    anyhow::bail!(
        "no targets to run: pass `--web` / `--ios` / `--android`, or add \
         `targets = [\"web\", ...]` to `[package.metadata.idealyst.app]`"
    )
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

/// Build (or rebuild) the AAS host binary for this project. The host
/// is what serves the wire WebSocket; runs as a child process for the
/// rest of this session.
fn build_aas_host(dir: &Path) -> Result<PathBuf> {
    eprintln!("[dev] building AAS host…");
    let artifact = build_aas::build(dir, build_aas::BuildOptions { release: false })?;
    Ok(artifact.host_binary)
}

/// Per-target launcher. Each variant handles its own AAS-vs-local
/// branching internally, then either blocks (web's static server) or
/// returns (iOS / Android, which fire-and-forget the device launch).
fn launch_target(target: Target, dir: &Path, args: &Args) -> Result<()> {
    match target {
        Target::Web => launch_web(dir, args),
        Target::Ios => launch_ios(dir, args),
        Target::Android => launch_android(dir, args),
        Target::Roku => anyhow::bail!(
            "Roku has no dev-mode story yet; use `idealyst build roku` for the package"
        ),
    }
}

/// Web launcher.
///
/// - AAS mode: build the wasm bundle with the `dev-hot-reload`
///   feature, then start `web-dev-host` which serves the bundle +
///   discovers the AAS server via Bonjour + injects
///   `window.IDEALYST_AAS_URL` into served HTML.
/// - Local mode: build the wasm bundle without `dev-hot-reload`,
///   then serve via `dev-http::serve_static` with livereload
///   polling. A file watcher rebuilds the bundle on source change
///   and bumps the generation counter so the browser reloads.
fn launch_web(dir: &Path, args: &Args) -> Result<()> {
    use dev_http::{serve_static, AasContext, ReloadContext};

    if args.aas {
        // ── 1. wasm shim that connects to the AAS host ────────────
        if !args.no_build {
            eprintln!("[dev web] building wasm shim with dev-hot-reload…");
            dev_reload::build_once(
                dir,
                &dev_reload::BuildOptions {
                    features: vec!["dev-hot-reload".to_string()],
                },
            )
            .context("wasm-pack build failed (dev-hot-reload feature)")?;
        }

        // ── 2. mDNS browser thread fills `AasContext.aas_url` so
        //       the HTTP layer can inject `window.IDEALYST_AAS_URL`
        //       into served pages.
        let app_id = parse_manifest(dir)?.app.require_bundle_id()?.to_string();
        let aas_url = Arc::new(Mutex::new(None));
        spawn_aas_browser(app_id, aas_url.clone());

        let ctx = AasContext { aas_url };
        eprintln!(
            "[dev web] AAS-bridged HTTP at http://{}:{}",
            args.host, args.port
        );
        serve_static(&args.host, args.port, dir, None, Some(ctx))?;
        Ok(())
    } else {
        // ── Local-render mode: livereload-driven hot-reload. ───────
        let gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        if !args.no_build {
            // `dev_reload::start` does the first build synchronously
            // and then keeps a watcher thread alive in the returned
            // handle. Forget the handle: it lives as long as the
            // HTTP serve loop below.
            let handle = dev_reload::start(dir, gen.clone())?;
            std::mem::forget(handle);
        }
        let ctx = ReloadContext { gen };
        eprintln!(
            "[dev web] livereload HTTP at http://{}:{}",
            args.host, args.port
        );
        serve_static(&args.host, args.port, dir, Some(ctx), None)?;
        Ok(())
    }
}

/// iOS launcher. Reuses the `run-ios` crate's pipeline:
/// builds the staticlib (with or without the AAS shell feature),
/// drops it into an Xcode bundle, and launches the simulator.
///
/// Live local-mode hot reload isn't wired yet — `--ios` without
/// `--aas` does a one-shot build + run; the user restarts when
/// they want a new build. AAS mode is the live path: the dev-server
/// already handles reload on source change, the iOS app re-renders
/// automatically.
fn launch_ios(dir: &Path, args: &Args) -> Result<()> {
    let mode = if args.aas {
        run_ios::RunMode::Aas
    } else {
        run_ios::RunMode::Local
    };
    eprintln!("[dev ios] building + launching simulator (mode: {:?})…", mode);
    let artifact = run_ios::run(
        dir,
        run_ios::RunOptions {
            release: false,
            mode,
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
/// AAS mode swaps the cdylib (backend-android with `aas-shell`) and
/// the Java sources (MainActivity reads `IdealystAppId` from manifest
/// meta-data, acquires a MulticastLock so mDNS browse works on
/// Wi-Fi, runs a `Handler` tick into `drainAas`). Local mode keeps
/// the in-process mount path.
fn launch_android(dir: &Path, args: &Args) -> Result<()> {
    let mode = if args.aas {
        run_android::RunMode::Aas
    } else {
        run_android::RunMode::Local
    };
    eprintln!(
        "[dev android] building + launching emulator (mode: {:?})…",
        mode
    );
    let artifact = run_android::run(
        dir,
        run_android::RunOptions {
            release: false,
            avd: None,
            mode,
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

/// Long-lived mDNS browser thread shared by the web launcher's AAS
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
            "[dev web] mDNS browsing for AAS dev-server with app_id={:?}",
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
                        eprintln!("[dev web] discovered AAS at {u}");
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
/// list and kills each child before exiting.
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

impl Args {
    /// Cheap clone of the bits each per-target worker needs. We
    /// don't `derive(Clone)` because `PathBuf` already clones cheaply
    /// and the rest is `Copy` / small `String`s — a tiny manual
    /// clone keeps it explicit which fields cross the thread
    /// boundary.
    fn shallow_clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            aas: self.aas,
            web: self.web,
            ios: self.ios,
            android: self.android,
            port: self.port,
            host: self.host.clone(),
            no_build: self.no_build,
        }
    }
}
