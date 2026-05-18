//! File-watch + auto-rebuild + self-exec for the dev server.
//!
//! Run [`spawn_rebuild_loop`] in a background thread. It watches the
//! supplied paths for source changes, runs the supplied `cargo build`
//! invocation when something changes, and on success replaces the
//! current process image with the freshly-built binary via `exec`.
//!
//! The currently-connected WebSocket dies as the process image swaps;
//! the web client reconnects via its own auto-reload behavior and
//! receives the new initial snapshot.
//!
//! Unix only (uses `std::os::unix::process::CommandExt::exec`). The
//! self-restart path silently no-ops on other platforms — pair with
//! an external supervisor (`cargo watch`) there.

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebounceEventResult};

/// Description of the cargo invocation to run when source changes.
/// Defaults to `cargo build` with no args.
#[derive(Clone)]
pub struct RebuildCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory for the spawned process. `None` inherits the
    /// host's cwd (rarely what you want — cargo's `.cargo/config.toml`
    /// discovery starts from cwd, so without this the watcher's cargo
    /// can miss workspace config and rebuild from scratch every time).
    pub cwd: Option<PathBuf>,
}

impl RebuildCommand {
    pub fn cargo_build(package: &str, bin: &str) -> Self {
        Self {
            program: "cargo".into(),
            args: vec!["build".into(), "-p".into(), package.into(), "--bin".into(), bin.into()],
            cwd: None,
        }
    }
}

/// Configuration for the rebuild loop.
pub struct RebuildConfig {
    pub watch_paths: Vec<PathBuf>,
    pub command: RebuildCommand,
    /// Debounce window. Multiple writes that arrive within this
    /// window collapse into one rebuild. 300ms tracks the typical
    /// editor save-burst on macOS.
    pub debounce: Duration,
    /// Optional snapshot hook. Called on each successful rebuild,
    /// just before `exec`. Returns a list of `(env_var, value)` to
    /// set on the re-execed process. Used to persist things like
    /// the navigator URL stack across the process image swap.
    ///
    /// Only consulted when `on_success` is `None` (the legacy
    /// single-process AAS host path).
    ///
    /// `Send` because it runs on the file-watcher thread; usually
    /// captures an `Arc<Mutex<...>>` shared with the main thread.
    pub before_exec: Option<Box<dyn FnMut() -> Vec<(String, String)> + Send>>,
    /// Alternative to `before_exec` for the split-process AAS host:
    /// instead of self-execing on rebuild success, invoke this
    /// callback. The host uses it to SIGKILL + respawn the sidecar
    /// child without dropping the long-lived WebSocket listener.
    ///
    /// Mutually exclusive with `before_exec` — if both are set,
    /// `on_success` wins.
    pub on_success: Option<Box<dyn FnMut() + Send>>,
}

/// Spawn a background thread that watches `config.watch_paths`,
/// runs `config.command` on debounced changes, and self-execs on
/// successful builds.
///
/// Returns a join handle the caller can stash — dropping it lets
/// the thread keep running.
pub fn spawn_rebuild_loop(config: RebuildConfig) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || run(config))
}

/// Lightweight variant: just call `on_change` on every debounced
/// burst. No command execution, no exec, no `on_success` ladder.
/// Used by the AAS host's hot-patch driver, which owns the entire
/// build pipeline itself.
pub fn spawn_change_loop(
    watch_paths: Vec<PathBuf>,
    debounce: Duration,
    mut on_change: Box<dyn FnMut() + Send>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (tx, rx) = std_mpsc::channel::<DebounceEventResult>();
        let mut debouncer = match new_debouncer(debounce, tx) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[dev-server] failed to start change watcher: {}", e);
                return;
            }
        };
        for path in &watch_paths {
            if let Err(e) = debouncer.watcher().watch(path, RecursiveMode::Recursive) {
                eprintln!("[dev-server] watch {:?} failed: {}", path, e);
            } else {
                eprintln!("[dev-server] watching {:?}", path);
            }
        }
        for evt in rx {
            match evt {
                Ok(ref ev) if !ev.is_empty() => {
                    eprintln!("[dev-server] change detected ({} event(s))", ev.len());
                    on_change();
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[dev-server] watch error: {:?}", e);
                }
            }
        }
    })
}

fn run(mut config: RebuildConfig) {
    let (tx, rx) = std_mpsc::channel::<DebounceEventResult>();
    let mut debouncer = match new_debouncer(config.debounce, tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[dev-server] failed to start file watcher: {}", e);
            return;
        }
    };
    for path in &config.watch_paths {
        if let Err(e) = debouncer.watcher().watch(path, RecursiveMode::Recursive) {
            eprintln!("[dev-server] watch {:?} failed: {}", path, e);
        } else {
            eprintln!("[dev-server] watching {:?}", path);
        }
    }

    for evt in rx {
        match evt {
            Ok(events) if !events.is_empty() => {
                let detected_at_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let t_watcher = std::time::Instant::now();
                eprintln!(
                    "[dev-server] source changed ({} event(s)), rebuilding…",
                    events.len()
                );
                if let Some(on_success) = &mut config.on_success {
                    // Split-process mode: rebuild succeeded → invoke
                    // the supplied callback (which is expected to
                    // SIGKILL + respawn the sidecar). Host process
                    // itself does NOT exec.
                    let t_build_start = std::time::Instant::now();
                    if rebuild_only(&config.command) {
                        let build_ms = t_build_start.elapsed().as_millis();
                        let total_ms = t_watcher.elapsed().as_millis();
                        eprintln!(
                            "[dev-server] timing: cargo build {}ms (watcher→build-done {}ms)",
                            build_ms, total_ms
                        );
                        on_success();
                    }
                } else {
                    // Legacy single-process mode: self-exec into the
                    // freshly-built binary, optionally seeding env
                    // vars via `before_exec`.
                    let extra_env = match &mut config.before_exec {
                        Some(f) => f(),
                        None => Vec::new(),
                    };
                    rebuild_and_replace(&config.command, detected_at_ms, &extra_env);
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[dev-server] watcher error: {}", e);
            }
        }
    }
}

/// Run the cargo build, return true on success. Split out from
/// `rebuild_and_replace` so the split-process path can react to the
/// success/failure without the self-exec semantics.
fn rebuild_only(cmd: &RebuildCommand) -> bool {
    let mut child = std::process::Command::new(&cmd.program);
    child.args(&cmd.args);
    if let Some(dir) = &cmd.cwd {
        child.current_dir(dir);
    }
    match child.status() {
        Ok(status) if status.success() => {
            eprintln!("[dev-server] rebuild OK");
            true
        }
        Ok(status) => {
            eprintln!("[dev-server] rebuild failed (exit {}); keeping current build", status);
            false
        }
        Err(e) => {
            eprintln!("[dev-server] failed to spawn `{}`: {}", cmd.program, e);
            false
        }
    }
}

fn rebuild_and_replace(cmd: &RebuildCommand, detected_at_ms: u64, extra_env: &[(String, String)]) {
    if rebuild_only(cmd) {
        eprintln!("[dev-server] restarting…");
        self_exec(detected_at_ms, extra_env);
    }
}

#[cfg(unix)]
fn self_exec(detected_at_ms: u64, extra_env: &[(String, String)]) {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[dev-server] cannot find current_exe: {}", e);
            return;
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&args)
        .env("IDEALYST_REBUILT_AT_MS", detected_at_ms.to_string());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let err = cmd.exec();
    eprintln!("[dev-server] exec failed: {}", err);
}

#[cfg(not(unix))]
fn self_exec(_detected_at_ms: u64, _extra_env: &[(String, String)]) {
    eprintln!(
        "[dev-server] self-exec not implemented on this platform; \
         please restart the dev server manually or run under `cargo watch`"
    );
}
