//! Watch + rebuild loop for `idealyst dev` reload mode.
//!
//! On a source change under the project's `src/` (or its `Cargo.toml`):
//!
//! 1. Delegate to [`build_web::build`], which regenerates the
//!    `target/idealyst/<name>/web/wrapper/` crate, runs `wasm-pack`
//!    against the wrapper, and copies the resulting `pkg/` into the
//!    user project. The user crate stays a plain `rlib` — no
//!    `web.rs`, no `cdylib` crate-type, no `wasm-bindgen` dep.
//! 2. Bump a shared generation counter on success and notify waiters.
//!
//! That counter is the contract with `dev-http`: every connected
//! browser holds an SSE connection to the static server and reloads
//! itself when the value advances. Failed builds leave the counter
//! alone — the page keeps running the last good wasm until the user
//! fixes the error.
//!
//! runtime-server mode reuses this path with `user_features =
//! vec!["dev-hot-reload"]`; `build-web`'s wrapper grows a matching
//! `[features]` block that forwards the flag to the user-crate dep,
//! so the resulting wasm connects to the host's WebSocket instead
//! of rendering the local `app()` tree.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use build_ios::FrameworkSource;
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::RecursiveMode;

const DEBOUNCE_MS: u64 = 150;

/// Shared "the build just changed" signal between the watcher and any
/// consumers (the SSE endpoint in `dev-http`, the server-bin respawn
/// loop in the CLI). `gen` is the canonical "which build is live"
/// counter; the condvar lets blocking consumers wake immediately on
/// rebuild instead of polling.
///
/// Construct once per dev session; clone the `Arc` to share. The
/// type is intentionally lock-light on the read side (atomic load),
/// with the mutex/condvar pair carrying only the wake notification.
#[derive(Default)]
pub struct ReloadSignal {
    gen: AtomicU64,
    notify: (Mutex<()>, Condvar),
}

impl ReloadSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Current generation. `0` until the first successful build, then
    /// monotonically increasing.
    pub fn current(&self) -> u64 {
        self.gen.load(Ordering::Acquire)
    }

    /// Set the generation to a specific value and wake waiters. Used
    /// by the initial-build path which sets `1` rather than
    /// fetch-add-from-zero (callers shouldn't see a transient `0` in
    /// the middle of `start_with`).
    fn set(&self, value: u64) {
        self.gen.store(value, Ordering::Release);
        let _g = self.notify.0.lock().unwrap();
        self.notify.1.notify_all();
    }

    /// Increment and wake waiters. Returns the new generation.
    /// `pub` so external producers (manual-reload triggers, tests)
    /// can drive the signal; the watcher loop is just the most
    /// common caller, not the only one.
    pub fn bump(&self) -> u64 {
        let new = self.gen.fetch_add(1, Ordering::AcqRel) + 1;
        let _g = self.notify.0.lock().unwrap();
        self.notify.1.notify_all();
        new
    }

    /// Block until `current() > seen`, or until `timeout` elapses.
    /// Returns the current generation (which equals `seen` on timeout
    /// with no intervening bump). The mutex protects the condvar
    /// only — the actual state is the atomic counter.
    pub fn wait_past(&self, seen: u64, timeout: Duration) -> u64 {
        let mut g = self.notify.0.lock().unwrap();
        loop {
            let cur = self.gen.load(Ordering::Acquire);
            if cur > seen {
                return cur;
            }
            let (gn, res) = self.notify.1.wait_timeout(g, timeout).unwrap();
            g = gn;
            if res.timed_out() {
                return self.gen.load(Ordering::Acquire);
            }
        }
    }
}

/// Options for each rebuild. `source` is required because the
/// generated wrapper Cargo.toml needs to know whether to pull
/// framework crates by workspace path or by git rev (the CLI's
/// `framework_source::resolve` produces this for both web and
/// native paths).
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Framework-source resolution result. Passed through to
    /// [`build_web::BuildOptions`] verbatim.
    pub source: FrameworkSource,
    /// Cargo features to enable on the user crate. runtime-server mode passes
    /// `["dev-hot-reload"]` so the user crate compiles its
    /// hot-reload integration. Empty == default features.
    pub features: Vec<String>,
}

/// Run a single rebuild. Useful for callers that want one build
/// with specific features but don't need the watch loop.
pub fn build_once(dir: &Path, opts: &BuildOptions) -> Result<()> {
    build_wasm(dir, opts)
}

/// Run an initial build, then spawn a background thread that
/// watches the project's `src/` and `Cargo.toml`, rebuilds on
/// change, and bumps the signal on success.
///
/// The returned `JoinHandle` owns the watch thread. Callers usually
/// hold it for the lifetime of the dev server; dropping it before
/// then ends watching. Build/watch errors are logged to stderr but
/// never propagate — a failing build shouldn't tear the dev server
/// down; the user fixes the code and the next change re-triggers.
pub fn start(
    dir: &Path,
    signal: Arc<ReloadSignal>,
    source: FrameworkSource,
) -> Result<JoinHandle<()>> {
    start_with(
        dir,
        signal,
        BuildOptions {
            source,
            features: Vec::new(),
        },
    )
}

/// Same as [`start`], with explicit build options. Used by callers
/// that need to pin cargo features (e.g. `dev-hot-reload` for runtime-server).
pub fn start_with(
    dir: &Path,
    signal: Arc<ReloadSignal>,
    opts: BuildOptions,
) -> Result<JoinHandle<()>> {
    eprintln!("[dev-reload] initial build…");
    build_wasm(dir, &opts).context("initial web build failed")?;
    signal.set(1);

    let dir_owned = dir.to_path_buf();
    thread::Builder::new()
        .name("idealyst-watch".into())
        .spawn(move || watch_loop(dir_owned, signal, opts))
        .context("spawn watch thread")
}

/// Watch `src/` + `Cargo.toml` under `dir`. Each debounced event
/// batch triggers one `wasm-pack` build; the build is synchronous on
/// this thread so events arriving while a build is in flight queue
/// up naturally on the channel and we collapse them by draining
/// before the next build.
fn watch_loop(dir: PathBuf, signal: Arc<ReloadSignal>, opts: BuildOptions) {
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
        match build_wasm(&dir, &opts) {
            Ok(()) => {
                let new_gen = signal.bump();
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

fn build_wasm(dir: &Path, opts: &BuildOptions) -> Result<()> {
    // Delegate to `build_web::build` — it generates the wrapper,
    // runs wasm-pack against it, and copies `pkg/` into `dir`.
    // Same path `idealyst build web` uses; the dev loop is just
    // "do that, but on debounced file changes".
    build_web::build(
        dir,
        build_web::BuildOptions {
            release: false,
            source: opts.source.clone(),
            user_features: opts.features.clone(),
            // Dev loop never stages a deploy bundle — that's only for
            // `idealyst build --web --gzip` / `--out-dir`.
            bundle_out_dir: None,
            gzip: false,
            // Dev loop always inlines `lazy!` bodies (one binary, stable
            // toolchain, fast iteration). Splitting is a production-build
            // concern (`idealyst build --web`).
            split: build_web::SplitMode::Off,
        },
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn signal_starts_at_zero() {
        let s = ReloadSignal::new();
        assert_eq!(s.current(), 0);
    }

    #[test]
    fn bump_increments_monotonically() {
        let s = ReloadSignal::new();
        assert_eq!(s.bump(), 1);
        assert_eq!(s.bump(), 2);
        assert_eq!(s.bump(), 3);
        assert_eq!(s.current(), 3);
    }

    #[test]
    fn set_replaces_current() {
        let s = ReloadSignal::new();
        s.set(1);
        assert_eq!(s.current(), 1);
        assert_eq!(s.bump(), 2);
    }

    #[test]
    fn wait_past_returns_immediately_when_already_past() {
        let s = ReloadSignal::new();
        s.set(5);
        let start = Instant::now();
        let got = s.wait_past(3, Duration::from_secs(60));
        assert_eq!(got, 5);
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "wait_past should not block when already past seen; took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn wait_past_times_out_with_no_bump() {
        let s = ReloadSignal::new();
        s.set(2);
        let start = Instant::now();
        // No bump → must block until timeout, then return current.
        let got = s.wait_past(2, Duration::from_millis(80));
        assert_eq!(got, 2);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(60),
            "wait_past should have waited near full timeout; took {:?}",
            elapsed
        );
    }

    #[test]
    fn wait_past_wakes_on_bump_from_other_thread() {
        let s = ReloadSignal::new();
        let s2 = s.clone();
        let waiter = thread::spawn(move || {
            let start = Instant::now();
            let got = s2.wait_past(0, Duration::from_secs(5));
            (got, start.elapsed())
        });

        // Give the waiter a moment to actually park on the condvar
        // before we bump. Without this, the bump can race ahead of
        // the wait and the test still passes — but only because of
        // the fast-path check inside `wait_past`. Sleeping ensures
        // we actually exercise the notify path.
        thread::sleep(Duration::from_millis(50));
        let new = s.bump();
        assert_eq!(new, 1);

        let (got, elapsed) = waiter.join().expect("waiter panicked");
        assert_eq!(got, 1);
        assert!(
            elapsed < Duration::from_millis(500),
            "waiter should have woken promptly on bump; took {:?}",
            elapsed
        );
    }

    #[test]
    fn wait_past_wakes_all_waiters() {
        let s = ReloadSignal::new();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let s = s.clone();
            handles.push(thread::spawn(move || {
                s.wait_past(0, Duration::from_secs(5))
            }));
        }
        thread::sleep(Duration::from_millis(50));
        s.bump();
        for h in handles {
            assert_eq!(h.join().expect("waiter panicked"), 1);
        }
    }
}
