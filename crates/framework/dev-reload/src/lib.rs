//! Watch + wasm-pack rebuild loop for `idealyst dev` reload mode.
//!
//! On a source change under the project's `src/` (or its `Cargo.toml`):
//!
//! 1. Run `wasm-pack build --target web --dev` in the project dir.
//! 2. Bump a shared generation counter on success.
//!
//! That counter is the contract with `dev-http`: every connected
//! browser polls the static server's `/__idealyst/gen` endpoint and
//! reloads itself when the value advances. Failed builds leave the
//! counter alone — the page keeps running the last good wasm until
//! the user fixes the error.
//!
//! Why this lives in its own crate: rebuild orchestration is a
//! discrete concern. The CLI assembles it next to [`dev-http`]; tests
//! and tools can drive it directly without pulling in HTTP.
//!
//! Future: when AAS mode is wired up, the cargo-build+exec loop in
//! `dev-server::watch` and this wasm-pack loop should share a common
//! "watch then run a command, bump a signal" core — likely a small
//! `dev-watch` crate that both depend on. Holding off until both
//! consumers exist so the API doesn't get shaped by one of them
//! alone.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::RecursiveMode;

const WASM_PACK: &str = "wasm-pack";
const DEBOUNCE_MS: u64 = 150;

/// Run an initial `wasm-pack` build, then spawn a background thread
/// that watches the project's `src/` and `Cargo.toml`, rebuilds on
/// change, and bumps `gen` on success.
///
/// The returned `JoinHandle` owns the watch thread. Callers usually
/// hold it for the lifetime of the dev server; dropping it before
/// then ends watching. Build/watch errors are logged to stderr but
/// never propagate — a failing build shouldn't tear the dev server
/// down; the user fixes the code and the next change re-triggers.
pub fn start(dir: &Path, gen: Arc<AtomicU64>) -> Result<JoinHandle<()>> {
    eprintln!("[dev-reload] initial build…");
    build_wasm(dir).context("initial wasm-pack build failed")?;
    gen.store(1, Ordering::Relaxed);

    let dir_owned = dir.to_path_buf();
    thread::Builder::new()
        .name("idealyst-watch".into())
        .spawn(move || watch_loop(dir_owned, gen))
        .context("spawn watch thread")
}

/// Watch `src/` + `Cargo.toml` under `dir`. Each debounced event
/// batch triggers one `wasm-pack` build; the build is synchronous on
/// this thread so events arriving while a build is in flight queue
/// up naturally on the channel and we collapse them by draining
/// before the next build.
fn watch_loop(dir: PathBuf, gen: Arc<AtomicU64>) {
    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(Duration::from_millis(DEBOUNCE_MS), tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[dev-reload] could not start file watcher: {e}");
            return;
        }
    };

    let watch_paths = [dir.join("src"), dir.join("Cargo.toml")];
    for path in &watch_paths {
        if let Err(e) = debouncer
            .watcher()
            .watch(path, RecursiveMode::Recursive)
        {
            eprintln!("[dev-reload] cannot watch {}: {e}", path.display());
        }
    }

    eprintln!(
        "[dev-reload] watching {} for changes",
        watch_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );

    while let Ok(events) = rx.recv() {
        drain(&rx);
        if events.is_err() {
            continue;
        }
        eprintln!("[dev-reload] change detected, rebuilding…");
        match build_wasm(&dir) {
            Ok(()) => {
                let new_gen = gen.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[dev-reload] rebuilt — gen={new_gen}");
            }
            Err(e) => eprintln!("[dev-reload] rebuild failed: {e}"),
        }
        // Coalesce anything queued during the build — wasm-pack
        // writes to `pkg/` (not watched) and cargo touches
        // `target/` (not watched), but defensively draining keeps
        // editor save-bursts from triggering N consecutive builds.
        drain(&rx);
    }
}

fn drain<T>(rx: &mpsc::Receiver<T>) {
    while rx.try_recv().is_ok() {}
}

fn build_wasm(dir: &Path) -> Result<()> {
    let status = Command::new(WASM_PACK)
        .args(["build", "--target", "web", "--dev", "--out-dir", "pkg"])
        .current_dir(dir)
        .status()
        .with_context(|| {
            format!(
                "failed to spawn `{WASM_PACK}` — install with `cargo install wasm-pack`"
            )
        })?;
    if !status.success() {
        anyhow::bail!("`wasm-pack build` exited with {status}");
    }
    Ok(())
}
