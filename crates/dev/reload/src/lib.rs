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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use build_ios::FrameworkSource;
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::RecursiveMode;

const DEBOUNCE_MS: u64 = 150;

/// How long the watcher waits for the filesystem to go quiet before it
/// starts a build, on top of [`DEBOUNCE_MS`].
///
/// The 150ms debounce is tuned for one editor writing one file: a human
/// hits ⌘S and exactly one batch arrives. It is much too short for the
/// way the tree actually changes now — a multi-file refactor, a
/// formatter sweeping a crate, or a second agent editing several files
/// lands as a *sequence* of batches spread over hundreds of milliseconds
/// to seconds. Each batch used to start its own full rebuild, and since
/// a rebuild is far slower than the burst that triggered it, the queue
/// never drained: the bundle was perpetually mid-build and the browser
/// perpetually stale.
///
/// Waiting for a quiet window collapses one burst into one build. 400ms
/// is below the threshold where a human notices their save "didn't do
/// anything" and comfortably above the gap between writes in a
/// multi-file edit.
const QUIET_WINDOW_MS: u64 = 400;

/// Ceiling on the coalescing wait, so a *continuous* trickle of writes
/// still gets built.
///
/// Without it, an agent editing steadily for a minute would push the
/// quiet window out that whole time and never trigger a rebuild — which
/// is the same "never see my change" symptom by the opposite mechanism.
const MAX_COALESCE_MS: u64 = 3_000;

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

/// What a watcher callback produced. Returned by the `on_change`
/// closures [`start_watch`] drives.
///
/// The distinction exists because a successful rebuild is NOT the same
/// event as a changed artifact. The full-stack server watcher rebuilds
/// on every save in the app crate's closure — including UI-only edits,
/// which for the in-crate shape live in the same package as the server
/// bin — but cargo relinks the binary only when something it actually
/// depends on moved. Bumping the generation regardless is what made a
/// pure UI edit kill and rebind the server port for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rebuilt {
    /// The artifact actually changed — bump the generation so browsers
    /// reload / the CLI restarts the server.
    Changed,
    /// The callback succeeded but produced an identical artifact. Do
    /// NOT bump: nothing downstream needs to react.
    Unchanged,
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
    /// When `Some(dir)`, stage the **full** static bundle (index.html +
    /// `pkg/` + fonts/icons) into `dir` each rebuild instead of only
    /// copying `pkg/` into the project root. The full-stack dev path
    /// uses this so a standalone server (which serves a complete
    /// `dist/web` over `ServeDir`) gets a refreshed bundle on every save.
    /// `None` keeps the default pkg-into-project behavior that the
    /// `dev-http` static server and in-crate full-stack server rely on.
    pub bundle_out_dir: Option<PathBuf>,
    /// Premint static styles on every rebuild (`idealyst dev … --premint`).
    /// Each build runs the native style dump and refreshes
    /// `pkg/premint.css` alongside the wasm, and the wasm compiles with
    /// `--cfg idealyst_premint` — so the dev loop exercises the exact
    /// premint attach paths (minted-class guard, engine fallback + its
    /// once-per-class console warning) the deployed bundle will run.
    /// Turning any premint flag on forces `hydrate` OFF for the build:
    /// premint cannot combine with SSR adoption (`build-web` refuses the
    /// pair), so a premint dev session gives up the `dev --ssr` hand-off.
    pub premint: bool,
    /// Additionally compile the style engine out (`--premint-only`).
    /// Implies [`Self::premint`]. This is the strict verification mode:
    /// any style the crawl missed panics in the browser instead of
    /// silently falling back — run your app's interactions under this
    /// before shipping a po bundle.
    pub premint_only: bool,
    /// Log every engine fall-through (`--premint-report`). Implies
    /// [`Self::premint`].
    pub premint_report: bool,
    /// Whether each rebuild runs `wasm-split` (`--no-split` clears it).
    /// Passed through to [`build_web::BuildOptions::wasm_split`]
    /// verbatim: `true` is the default, and clearing it trades a larger
    /// served wasm for a shorter packaging pass.
    pub wasm_split: bool,
    /// Debug-info level for each rebuild's wasm (`--debuginfo`). Passed
    /// through to [`build_web::BuildOptions::debuginfo`]; the default
    /// trims DWARF that every post-cargo pass — and the browser, on every
    /// reload — would otherwise re-process.
    pub debuginfo: build_web::DebugInfo,
    /// Optimization posture for each rebuild (`--dev-opt`). Passed
    /// through to [`build_web::BuildOptions::dev_opt`]; the default
    /// favours the case the dev loop actually spends its time in — a
    /// framework-crate edit invalidating every workspace member
    /// downstream — over the leaf-only edit.
    pub dev_opt: build_web::DevOpt,
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
            bundle_out_dir: None,
            premint: false,
            premint_only: false,
            premint_report: false,
            wasm_split: true,
            debuginfo: build_web::DebugInfo::default(),
            dev_opt: build_web::DevOpt::default(),
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

/// Every LOCAL source root in `manifest_path`'s cargo dependency
/// closure: `src/` plus `Cargo.toml` for the crate itself and for each
/// path / workspace-member crate it actually pulls in.
///
/// This deliberately follows the RESOLVED GRAPH rather than the project
/// directory. The watch set used to be `[<dir>/src, <dir>/Cargo.toml]`
/// and nothing else, which is correct only for a single-crate app. The
/// moment shared UI moves into a sibling crate — the normal shape once
/// an app grows, and the universal shape in a multi-app workspace —
/// edits to that crate changed the built wasm but triggered no rebuild.
/// The failure mode is silent: no error, no log line, just a browser
/// serving an hours-old bundle while every save appears to succeed.
///
/// Registry and git dependencies are skipped: cargo reports them with a
/// non-null `source`, and they cannot change under a running dev
/// session. Only `source: null` packages — path deps and workspace
/// members — are watched. A `[patch]` pointing the framework at a local
/// checkout makes it local by that rule, so framework hacking rebuilds
/// the app too; the printed watch list is what makes that visible.
///
/// Best-effort: if cargo can't be run, or its output doesn't parse,
/// fall back to the old single-crate set. A dev session that watches
/// too little beats one that refuses to start.
pub fn watch_roots(manifest_path: &Path) -> Vec<PathBuf> {
    let dir = manifest_path.parent().unwrap_or(Path::new("."));
    let dirs = match package_dirs(manifest_path) {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => {
            eprintln!(
                "[dev-reload] cargo metadata listed no local packages for {}; \
                 watching the project crate only",
                manifest_path.display(),
            );
            vec![dir.to_path_buf()]
        }
        Err(e) => {
            eprintln!(
                "[dev-reload] could not resolve the dependency closure ({e:#}); \
                 watching the project crate only — edits to sibling crates will \
                 NOT rebuild",
            );
            vec![dir.to_path_buf()]
        }
    };

    let mut roots = Vec::new();
    for d in dirs {
        // A crate with no `src/` (build-script-only, or mid-scaffold)
        // is not an error — watch what exists and move on.
        let src = d.join("src");
        if src.is_dir() {
            roots.push(src);
        }
        let toml = d.join("Cargo.toml");
        if toml.is_file() {
            roots.push(toml);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Run `cargo metadata` for `manifest_path` and hand its document to
/// [`local_package_dirs`].
fn package_dirs(manifest_path: &Path) -> Result<Vec<PathBuf>> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .context("run `cargo metadata`")?;
    anyhow::ensure!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr).trim(),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parse `cargo metadata` output")?;
    Ok(local_package_dirs(&meta))
}

/// The pure core of [`watch_roots`]: given a `cargo metadata` document,
/// return the manifest directory of every local package reachable from
/// the root package. Split out so the graph walk is unit-testable
/// against a synthetic document, without invoking cargo or touching
/// the filesystem.
fn local_package_dirs(meta: &serde_json::Value) -> Vec<PathBuf> {
    // id -> manifest dir, local packages only.
    let mut local: BTreeMap<&str, PathBuf> = BTreeMap::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
        // Absent or null `source` == path dependency or workspace
        // member. Everything else came from a registry or a git rev.
        if !pkg.get("source").map_or(true, |s| s.is_null()) {
            continue;
        }
        let Some(id) = pkg["id"].as_str() else { continue };
        let Some(dir) = pkg["manifest_path"]
            .as_str()
            .and_then(|m| Path::new(m).parent())
        else {
            continue;
        };
        local.insert(id, dir.to_path_buf());
    }

    // Restrict to the root package's closure. Without this, a two-app
    // workspace would rebuild app A on every save in app B — they are
    // both workspace members, but neither depends on the other.
    let keep: BTreeSet<&str> = match meta.pointer("/resolve/root").and_then(|r| r.as_str()) {
        Some(root) => {
            let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for node in meta
                .pointer("/resolve/nodes")
                .and_then(|n| n.as_array())
                .into_iter()
                .flatten()
            {
                let Some(id) = node["id"].as_str() else { continue };
                edges.insert(
                    id,
                    node["dependencies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d.as_str())
                        .collect(),
                );
            }
            let mut seen = BTreeSet::new();
            let mut stack = vec![root];
            while let Some(id) = stack.pop() {
                if !seen.insert(id) {
                    continue;
                }
                for dep in edges.get(id).into_iter().flatten() {
                    stack.push(dep);
                }
            }
            seen
        }
        // A virtual workspace manifest has no single root package, so
        // there is no closure to walk — watch every local package.
        None => local.keys().copied().collect(),
    };

    local
        .into_iter()
        .filter(|(id, _)| keep.contains(id))
        .map(|(_, dir)| dir)
        .collect()
}

/// Watch every local source root in `dir`'s dependency closure (see
/// [`watch_roots`]) — its own `src/` + `Cargo.toml`, plus those of
/// each path / workspace-member crate it pulls in. Each debounced
/// event batch triggers one `wasm-pack` build; the build is
/// synchronous on this thread so events arriving while a build is in
/// flight queue up naturally on the channel and we collapse them by
/// draining before the next build.
fn watch_loop(dir: PathBuf, signal: Arc<ReloadSignal>, opts: BuildOptions) {
    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(Duration::from_millis(DEBOUNCE_MS), tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[dev-reload] could not start file watcher: {e}");
            return;
        }
    };

    let mut watch_paths = watch_roots(&dir.join("Cargo.toml"));
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
        describe(&watch_paths),
    );

    while let Ok(events) = rx.recv() {
        drain(&rx);
        if events.is_err() {
            continue;
        }
        // Absorb the rest of the burst before starting the build —
        // otherwise a multi-file edit queues one full rebuild per file.
        let folded = settle(&rx);
        if folded > 0 {
            eprintln!("[dev-reload] change detected (+{folded} more), rebuilding…");
        } else {
            eprintln!("[dev-reload] change detected, rebuilding…");
        }
        match build_wasm(&dir, &opts) {
            Ok(()) => {
                let new_gen = signal.bump();
                eprintln!("[dev-reload] rebuilt — gen={new_gen}");
            }
            Err(e) => eprintln!("[dev-reload] rebuild failed: {e}"),
        }

        // The save may have edited a `Cargo.toml` and ADDED a path
        // dependency — and a dependency nobody watches is precisely the
        // silent staleness this watcher exists to prevent, so it must
        // not be reintroduced by a mid-session edit. Re-resolving costs
        // one `cargo metadata` against a build we just spent orders of
        // magnitude longer on, which is cheap enough to do
        // unconditionally rather than sniff event paths for manifests.
        let fresh = watch_roots(&dir.join("Cargo.toml"));
        if fresh != watch_paths {
            for path in &watch_paths {
                let _ = debouncer.watcher().unwatch(path);
            }
            for path in &fresh {
                if let Err(e) = debouncer
                    .watcher()
                    .watch(path, RecursiveMode::Recursive)
                {
                    eprintln!("[dev-reload] cannot watch {}: {e}", path.display());
                }
            }
            watch_paths = fresh;
            eprintln!(
                "[dev-reload] dependencies changed — now watching {}",
                describe(&watch_paths),
            );
        }

        // Coalesce anything queued during the build — wasm-pack
        // writes to `pkg/` (not watched) and cargo touches
        // `target/` (not watched), but defensively draining keeps
        // editor save-bursts from triggering N consecutive builds.
        drain(&rx);
    }
}

/// The watch set as one log-friendly line. Printed at startup and
/// whenever it changes: a watcher that follows a dependency graph is
/// only trustworthy if you can see what it decided to follow.
fn describe(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn drain<T>(rx: &mpsc::Receiver<T>) {
    while rx.try_recv().is_ok() {}
}

/// Wait for the filesystem to go quiet, absorbing every event batch that
/// arrives meanwhile, and report how many extra batches were folded in.
///
/// Returns once either no batch has arrived for [`QUIET_WINDOW_MS`] or
/// [`MAX_COALESCE_MS`] has elapsed since the first one. See
/// [`QUIET_WINDOW_MS`] for why a fixed 150ms debounce isn't enough.
///
/// Split out from the watcher loops so the policy is unit-testable
/// against a plain channel — the loops themselves are infinite and own a
/// real filesystem watcher.
fn settle<T>(rx: &mpsc::Receiver<T>) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_millis(MAX_COALESCE_MS);
    let mut folded = 0usize;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return folded;
        }
        // Never wait past the cap, even if the quiet window is longer
        // than the time left on it.
        let wait = Duration::from_millis(QUIET_WINDOW_MS).min(deadline - now);
        match rx.recv_timeout(wait) {
            Ok(_) => folded += 1,
            // Quiet for a full window, or the watcher hung up — either
            // way there is nothing more to fold in.
            Err(_) => return folded,
        }
    }
}

/// Spawn a watcher on `paths` that runs `on_change` for each
/// debounced event batch, and bumps `signal` after each successful
/// call so connected browsers reload. Used by paths that aren't the
/// full wasm rebuild — currently the icon-source watcher in
/// `cmd::dev`, which re-runs `icon_gen::sync_web_icons` when the
/// project's SVG/PNG changes.
///
/// `label` appears in `[dev-reload <label>]` log lines so the user
/// can tell which watcher fired. Failed callbacks log to stderr but
/// don't bump the signal (or kill the thread) — the next change
/// re-tries.
///
/// A callback returning [`Rebuilt::Unchanged`] also leaves the signal
/// alone: it ran fine, it just produced the same artifact, so waking
/// consumers would be pure churn (see [`Rebuilt`]).
pub fn start_watch<F>(
    paths: Vec<PathBuf>,
    signal: Arc<ReloadSignal>,
    label: &'static str,
    mut on_change: F,
) -> Result<JoinHandle<()>>
where
    F: FnMut() -> Result<Rebuilt> + Send + 'static,
{
    thread::Builder::new()
        .name(format!("idealyst-watch-{label}"))
        .spawn(move || {
            let (tx, rx) = mpsc::channel();
            let mut debouncer = match new_debouncer(Duration::from_millis(DEBOUNCE_MS), tx) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[dev-reload {label}] watcher init failed: {e}");
                    return;
                }
            };
            // Watch each path non-recursively first — the typical
            // case is a handful of asset files, not directories.
            // The watcher silently tolerates a missing path (the
            // user can add an icon mid-session and the next regen
            // hits a different code path).
            for path in &paths {
                let mode = if path.is_dir() {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                if let Err(e) = debouncer.watcher().watch(path, mode) {
                    eprintln!(
                        "[dev-reload {label}] cannot watch {}: {e}",
                        path.display()
                    );
                }
            }
            eprintln!(
                "[dev-reload {label}] watching {}",
                paths
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
                let folded = settle(&rx);
                if folded > 0 {
                    eprintln!("[dev-reload {label}] change detected (+{folded} more)");
                } else {
                    eprintln!("[dev-reload {label}] change detected");
                }
                match on_change() {
                    Ok(Rebuilt::Changed) => {
                        let new_gen = signal.bump();
                        eprintln!("[dev-reload {label}] regen complete — gen={new_gen}");
                    }
                    Ok(Rebuilt::Unchanged) => eprintln!(
                        "[dev-reload {label}] rebuilt, artifact unchanged — nothing to do"
                    ),
                    Err(e) => eprintln!("[dev-reload {label}] regen failed: {e}"),
                }
                drain(&rx);
            }
        })
        .context("spawn watch thread")
}

fn build_wasm(dir: &Path, opts: &BuildOptions) -> Result<()> {
    // Delegate to `build_web::build` — it generates the wrapper,
    // runs wasm-pack against it, and copies `pkg/` into `dir`.
    // Same path `idealyst build web` uses; the dev loop is just
    // "do that, but on debounced file changes".
    build_web::build(dir, to_build_web_options(opts)).map(|_| ())
}

/// Map the dev-loop options onto a full `build_web::BuildOptions`.
/// Split out of [`build_wasm`] so the premint/hydrate interaction is
/// unit-testable without running a build.
fn to_build_web_options(opts: &BuildOptions) -> build_web::BuildOptions {
    let premint = opts.premint || opts.premint_only || opts.premint_report;
    build_web::BuildOptions {
        wasm_split: opts.wasm_split,
        debuginfo: opts.debuginfo,
        // Dev reload always builds the full vocabulary: the flag is a
        // release-bundle lever, and a dev rebuild that dropped a
        // primitive would panic at mount mid-session.
        primitives: None,
        premint_only: opts.premint_only,
        premint_report: opts.premint_report,
        release: false,
        source: opts.source.clone(),
        user_features: opts.features.clone(),
        // Full-stack standalone-server dev passes a `dist/web` here
        // so the bundle the server serves is restaged each rebuild;
        // the plain dev-http / in-crate paths leave it `None` and
        // get the default pkg-into-project copy.
        bundle_out_dir: opts.bundle_out_dir.clone(),
        dev_opt: opts.dev_opt,
        gzip: false,
        // Dev rebuilds skip the q11 encode; `.br` siblings are a
        // deploy-artifact concern (`idealyst build --web --release`).
        brotli: false,
        // Dev keeps panic messages — stripping them is a
        // production-only `idealyst build --web --strip-panics` thing.
        strip_panics: false,
        // Dev-loop builds support `dev --ssr` hand-offs — EXCEPT under
        // premint, which cannot combine with SSR adoption (the SSR HTML
        // carries live-minted classes, the hydrating client stamps
        // preminted ones; `build_web` refuses the pair). A premint dev
        // session trades the SSR hand-off for exercising the real
        // premint attach paths.
        hydrate: !premint,
        // Dev-loop builds skip data pruning — iteration speed
        // beats bundle size, and the heuristic adds a pass per
        // rebuild.
        prune_dead_data_min: None,
        premint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Regression guard for the save-storm that kept the dev bundle
    /// perpetually mid-build: a multi-file edit arrives as a SEQUENCE of
    /// debounced batches, and the old loop started a full rebuild for
    /// each one. Since a rebuild outlasts the burst that triggered it,
    /// the queue never drained. `settle` folds the burst into one build.
    #[test]
    fn settle_folds_a_burst_into_one_rebuild() {
        let (tx, rx) = mpsc::channel::<u8>();
        let writer = thread::spawn(move || {
            // Five "files" written well inside the quiet window — what a
            // formatter sweep or an agent's multi-file edit looks like.
            for i in 0..5 {
                thread::sleep(Duration::from_millis(40));
                let _ = tx.send(i);
            }
        });
        let folded = settle(&rx);
        writer.join().unwrap();
        assert_eq!(
            folded, 5,
            "every batch in the burst must be absorbed into the pending build",
        );
    }

    /// A quiet channel must not make the watcher wait out the cap — the
    /// single-file save (a human hitting ⌘S) has to start building after
    /// one quiet window, not three seconds later.
    #[test]
    fn settle_returns_promptly_when_nothing_follows() {
        let (_tx, rx) = mpsc::channel::<u8>();
        let start = Instant::now();
        let folded = settle(&rx);
        let elapsed = start.elapsed();
        assert_eq!(folded, 0);
        assert!(
            elapsed >= Duration::from_millis(QUIET_WINDOW_MS - 50),
            "must actually wait for the window: {elapsed:?}",
        );
        assert!(
            elapsed < Duration::from_millis(MAX_COALESCE_MS),
            "a lone save must not pay the coalescing cap: {elapsed:?}",
        );
    }

    /// A CONTINUOUS trickle of writes must not push the quiet window out
    /// forever — that starves the rebuild and reproduces the very symptom
    /// ("my change never shows up") from the other direction.
    #[test]
    fn settle_caps_a_continuous_trickle() {
        let (tx, rx) = mpsc::channel::<u8>();
        let stop = Arc::new(AtomicU64::new(0));
        let stop_w = stop.clone();
        let writer = thread::spawn(move || {
            // Write faster than the quiet window, for longer than the cap.
            while stop_w.load(Ordering::Acquire) == 0 {
                if tx.send(1).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let start = Instant::now();
        let _ = settle(&rx);
        let elapsed = start.elapsed();
        stop.store(1, Ordering::Release);
        let _ = writer.join();
        assert!(
            elapsed >= Duration::from_millis(MAX_COALESCE_MS - 100),
            "should have coalesced up to the cap: {elapsed:?}",
        );
        assert!(
            elapsed < Duration::from_millis(MAX_COALESCE_MS + 800),
            "must not run past the cap while writes keep arriving: {elapsed:?}",
        );
    }

    /// `Rebuilt::Unchanged` exists so a successful-but-no-op rebuild does
    /// not wake consumers. The full-stack loop turns a generation bump
    /// into a server restart, so bumping on an unchanged binary would
    /// bounce the port for nothing.
    #[test]
    fn unchanged_rebuild_is_distinguishable_from_a_changed_one() {
        assert_ne!(Rebuilt::Changed, Rebuilt::Unchanged);
    }

    fn opts(premint: bool, only: bool, report: bool) -> BuildOptions {
        BuildOptions {
            source: FrameworkSource::Workspace { root: std::path::PathBuf::from("/x") },
            features: Vec::new(),
            bundle_out_dir: None,
            premint,
            premint_only: only,
            premint_report: report,
            wasm_split: true,
            debuginfo: build_web::DebugInfo::default(),
            dev_opt: build_web::DevOpt::default(),
        }
    }

    /// The dev loop passes the split choice straight through — a
    /// `--no-split` session must not silently start splitting again on
    /// the second rebuild.
    #[test]
    fn wasm_split_choice_reaches_the_web_build() {
        let mut o = opts(false, false, false);
        assert!(to_build_web_options(&o).wasm_split);
        o.wasm_split = false;
        assert!(!to_build_web_options(&o).wasm_split);
    }

    /// Premint dev builds must turn hydration OFF: `build_web` refuses
    /// the premint+hydrate pair, so leaving the dev loop's default
    /// `hydrate: true` in place would make every `dev --premint`
    /// rebuild fail. Each of the three flags implies premint (and so
    /// must flip hydrate), matching `idealyst build`'s semantics.
    #[test]
    fn premint_flags_imply_premint_and_disable_hydrate() {
        for (p, o, r) in [(true, false, false), (false, true, false), (false, false, true)] {
            let mapped = to_build_web_options(&opts(p, o, r));
            assert!(mapped.premint, "({p},{o},{r}) implies premint");
            assert!(!mapped.hydrate, "({p},{o},{r}) must disable hydrate");
        }
        let plain = to_build_web_options(&opts(false, false, false));
        assert!(!plain.premint);
        assert!(plain.hydrate, "non-premint dev builds keep the SSR hand-off");
    }

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

    /// A `cargo metadata` document shaped like a real multi-crate app
    /// workspace: the app crate depends on a shared UI crate which
    /// depends on an api crate; a SECOND app crate is a workspace
    /// member but nothing depends on it; and the framework arrives by
    /// git while serde arrives from the registry.
    fn workspace_metadata() -> serde_json::Value {
        serde_json::json!({
            "packages": [
                {"id": "app-main", "manifest_path": "/w/crates/app-main/Cargo.toml", "source": null},
                {"id": "ui-shared", "manifest_path": "/w/crates/ui-shared/Cargo.toml", "source": null},
                {"id": "api", "manifest_path": "/w/crates/api/Cargo.toml", "source": null},
                {"id": "app-checkin", "manifest_path": "/w/crates/app-checkin/Cargo.toml", "source": null},
                {"id": "idea-ui", "manifest_path": "/home/u/.cargo/git/checkouts/x/idea-ui/Cargo.toml",
                 "source": "git+https://github.com/IdealystIO/idealyst-native.git?tag=1.3.16"},
                {"id": "serde", "manifest_path": "/home/u/.cargo/registry/src/serde/Cargo.toml",
                 "source": "registry+https://github.com/rust-lang/crates.io-index"}
            ],
            "resolve": {
                "root": "app-main",
                "nodes": [
                    {"id": "app-main", "dependencies": ["ui-shared", "idea-ui", "serde"]},
                    {"id": "ui-shared", "dependencies": ["api", "idea-ui"]},
                    {"id": "api", "dependencies": ["serde"]},
                    {"id": "app-checkin", "dependencies": ["ui-shared"]},
                    {"id": "idea-ui", "dependencies": []},
                    {"id": "serde", "dependencies": []}
                ]
            }
        })
    }

    /// The regression this whole function exists for: a save in a
    /// SIBLING crate must be watched. Before the closure walk the set
    /// was `[<app>/src, <app>/Cargo.toml]`, so editing `ui-shared`
    /// changed the bundle's contents but never triggered a rebuild —
    /// silently, which is what made it so expensive to diagnose.
    #[test]
    fn path_dependencies_are_watched() {
        let dirs = local_package_dirs(&workspace_metadata());
        assert!(dirs.contains(&PathBuf::from("/w/crates/ui-shared")));
        assert!(dirs.contains(&PathBuf::from("/w/crates/api")));
        assert!(dirs.contains(&PathBuf::from("/w/crates/app-main")));
    }

    /// Registry and git deps can't change under a running session, and
    /// watching a whole `~/.cargo` tree would be a lot of inotify
    /// handles for nothing.
    #[test]
    fn registry_and_git_dependencies_are_not_watched() {
        let dirs = local_package_dirs(&workspace_metadata());
        assert!(
            dirs.iter().all(|d| d.starts_with("/w/")),
            "only local packages should be watched, got {dirs:?}",
        );
    }

    /// The other app in a two-app workspace is a local package but NOT
    /// in this app's closure. Watching it would rebuild the admin
    /// bundle on every kiosk save.
    #[test]
    fn unrelated_workspace_members_are_not_watched() {
        let dirs = local_package_dirs(&workspace_metadata());
        assert!(
            !dirs.contains(&PathBuf::from("/w/crates/app-checkin")),
            "a sibling app nothing depends on should stay out of the watch set",
        );
    }

    /// A virtual workspace manifest resolves no root package, so there
    /// is no closure to walk — fall back to every local member rather
    /// than watching nothing.
    #[test]
    fn virtual_workspace_watches_every_local_member() {
        let mut meta = workspace_metadata();
        meta["resolve"]["root"] = serde_json::Value::Null;
        let dirs = local_package_dirs(&meta);
        assert!(dirs.contains(&PathBuf::from("/w/crates/app-checkin")));
        assert!(dirs.contains(&PathBuf::from("/w/crates/app-main")));
        assert!(dirs.iter().all(|d| d.starts_with("/w/")));
    }

    /// A package with no `source` KEY at all (rather than an explicit
    /// null) is still a local one — don't drop it on a shape cargo is
    /// free to emit either way.
    #[test]
    fn missing_source_key_counts_as_local() {
        let meta = serde_json::json!({
            "packages": [{"id": "solo", "manifest_path": "/w/solo/Cargo.toml"}],
            "resolve": {"root": "solo", "nodes": [{"id": "solo", "dependencies": []}]}
        });
        assert_eq!(local_package_dirs(&meta), vec![PathBuf::from("/w/solo")]);
    }
}
