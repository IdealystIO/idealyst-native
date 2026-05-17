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
}

impl RebuildCommand {
    pub fn cargo_build(package: &str, bin: &str) -> Self {
        Self {
            program: "cargo".into(),
            args: vec!["build".into(), "-p".into(), package.into(), "--bin".into(), bin.into()],
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

fn run(config: RebuildConfig) {
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
                // Capture the moment-of-change so we can report
                // end-to-end "change → apply" latency on the app
                // side after the rebuild completes.
                let detected_at_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                eprintln!(
                    "[dev-server] source changed ({} event(s)), rebuilding…",
                    events.len()
                );
                rebuild_and_replace(&config.command, detected_at_ms);
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[dev-server] watcher error: {}", e);
            }
        }
    }
}

fn rebuild_and_replace(cmd: &RebuildCommand, detected_at_ms: u64) {
    let mut child = std::process::Command::new(&cmd.program);
    child.args(&cmd.args);

    match child.status() {
        Ok(status) if status.success() => {
            eprintln!("[dev-server] rebuild OK, restarting…");
            self_exec(detected_at_ms);
        }
        Ok(status) => {
            eprintln!("[dev-server] rebuild failed (exit {}); keeping current build", status);
        }
        Err(e) => {
            eprintln!("[dev-server] failed to spawn `{}`: {}", cmd.program, e);
        }
    }
}

#[cfg(unix)]
fn self_exec(detected_at_ms: u64) {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[dev-server] cannot find current_exe: {}", e);
            return;
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Hand the new process the timestamp via env var; it'll
    // include it in the next `Hello` so the app can log
    // end-to-end latency.
    let err = std::process::Command::new(exe)
        .args(&args)
        .env("IDEALYST_REBUILT_AT_MS", detected_at_ms.to_string())
        .exec();
    eprintln!("[dev-server] exec failed: {}", err);
}

#[cfg(not(unix))]
fn self_exec(_detected_at_ms: u64) {
    eprintln!(
        "[dev-server] self-exec not implemented on this platform; \
         please restart the dev server manually or run under `cargo watch`"
    );
}
