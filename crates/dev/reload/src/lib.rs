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

/// The overlay's build-time half, re-exported.
///
/// It lives in `dev-overlay` because the runtime-server host needs the
/// same decision and has no business pulling this crate's bundler in to
/// get it. Re-exported here so the watcher's callers keep one import.
pub use dev_overlay::archive as overlay;
pub use dev_overlay::decide as overlay_decide;

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
/// How many decided patches the signal keeps for listeners that
/// reconnect. A page that fell further behind than this has missed
/// enough that reloading is the honest answer.
const MAX_BUFFERED_PATCHES: usize = 64;

/// type is intentionally lock-light on the read side (atomic load),
/// with the mutex/condvar pair carrying only the wake notification.
#[derive(Default)]
pub struct ReloadSignal {
    gen: AtomicU64,
    notify: (Mutex<()>, Condvar),
    /// Overlay patches decided since the process started, newest last,
    /// each with the sequence number a listener resumes from.
    ///
    /// A separate counter from `gen` because the two mean opposite
    /// things to a page: a generation bump says "the bundle moved,
    /// reload", a patch says "the bundle is fine, apply this". Folding
    /// them into one number would make every patch a reload, which is
    /// the cost the patch exists to avoid.
    ///
    /// Bounded: a long session must not grow a list nobody will read,
    /// and a listener that fell far enough behind to miss entries is a
    /// listener that should reload anyway.
    patches: Mutex<Vec<PushedPatch>>,
    patch_seq: AtomicU64,
}

/// What a page should do with a pushed patch.
///
/// The distinction is the page's, not this crate's: an overlay patch is
/// new DATA for a `ui!` site and the page applies it to what is already
/// mounted; a hot patch is new CODE and the page has to load a module and
/// rebuild the tree against it. They share one ordered channel because a
/// save can produce both and the order they were decided in is the order
/// they must arrive in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    Overlay,
    Hot,
}

impl PatchKind {
    /// The SSE event name the page listens on, so it can route a payload
    /// without parsing it first.
    pub fn sse_event(self) -> &'static str {
        match self {
            PatchKind::Overlay => "patch",
            PatchKind::Hot => "hot-patch",
        }
    }
}

/// One pushed patch, with the sequence number a listener catches up by.
#[derive(Debug, Clone)]
pub struct PushedPatch {
    pub seq: u64,
    pub kind: PatchKind,
    pub json: String,
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

    /// The patch sequence a listener has caught up to.
    pub fn patch_seq(&self) -> u64 {
        self.patch_seq.load(Ordering::Acquire)
    }

    /// Record a decided overlay patch and wake listeners.
    ///
    /// `json` is whatever the delivery side agreed on; this type does
    /// not parse it. Keeping the payload opaque is what lets the
    /// protocol between the watcher and the page change without this
    /// crate's public surface moving.
    pub fn push_patch(&self, json: String) -> u64 {
        self.push(PatchKind::Overlay, json)
    }

    /// Record a decided HOT patch — new code rather than new data — and
    /// wake listeners.
    ///
    /// A separate kind rather than a separate channel, because the two
    /// are ordered against each other: a save that changed both a
    /// literal and a body must reach the page in the order the dev loop
    /// decided them, and two channels cannot promise that. They travel
    /// as differently-named SSE events so the page routes each to the
    /// right applier without parsing the payload first.
    pub fn push_hot_patch(&self, json: String) -> u64 {
        self.push(PatchKind::Hot, json)
    }

    fn push(&self, kind: PatchKind, json: String) -> u64 {
        let seq = self.patch_seq.fetch_add(1, Ordering::AcqRel) + 1;
        {
            let mut patches = self.patches.lock().unwrap();
            patches.push(PushedPatch { seq, kind, json });
            let len = patches.len();
            if len > MAX_BUFFERED_PATCHES {
                patches.drain(..len - MAX_BUFFERED_PATCHES);
            }
        }
        self.patch_seq.store(seq, Ordering::Release);
        let _g = self.notify.0.lock().unwrap();
        self.notify.1.notify_all();
        seq
    }

    /// Patches newer than `seen`, oldest first.
    ///
    /// A listener whose `seen` predates the buffer gets what is left,
    /// not an error: the page it belongs to is about to be told to
    /// reload anyway, and refusing to send the recent ones would make a
    /// reconnect worse than useless.
    pub fn patches_since(&self, seen: u64) -> Vec<PushedPatch> {
        self.patches
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.seq > seen)
            .cloned()
            .collect()
    }

    /// Block until `current() > seen`, or until `timeout` elapses.
    /// Returns the current generation (which equals `seen` on timeout
    /// with no intervening bump). The mutex protects the condvar
    /// only — the actual state is the atomic counter.
    pub fn wait_past(&self, seen: u64, timeout: Duration) -> u64 {
        self.wait_past_either(seen, u64::MAX, timeout).0
    }

    /// Block until either the generation passes `seen_gen` or the patch
    /// sequence passes `seen_patch`. Returns both current values.
    ///
    /// One wait for two events, because the SSE writer has one thread
    /// per listener and waiting on them separately would need two.
    pub fn wait_past_either(
        &self,
        seen_gen: u64,
        seen_patch: u64,
        timeout: Duration,
    ) -> (u64, u64) {
        let mut g = self.notify.0.lock().unwrap();
        loop {
            let (cur, patch) = (
                self.gen.load(Ordering::Acquire),
                self.patch_seq.load(Ordering::Acquire),
            );
            if cur > seen_gen || patch > seen_patch {
                return (cur, patch);
            }
            let (gn, res) = self.notify.1.wait_timeout(g, timeout).unwrap();
            g = gn;
            if res.timed_out() {
                return (
                    self.gen.load(Ordering::Acquire),
                    self.patch_seq.load(Ordering::Acquire),
                );
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
    /// Robot relay URL to advertise from the STAGED `index.html`
    /// ([`build_web::BuildOptions::robot_relay_url`]). Only the
    /// full-stack path sets it: its server hands out the staged file
    /// directly, so a rebuild that restages `index.html` would drop a
    /// serve-time injection. `None` everywhere else.
    pub robot_relay_url: Option<String>,
    /// A `<script>` to splice into the staged `index.html` head —
    /// the full-stack shape's livereload + overlay `EventSource`. See
    /// [`build_web::BuildOptions::head_script`].
    pub head_script: Option<String>,
    /// [`build_web::BuildOptions::runtime_server_url`]. `Some` only in
    /// wire mode on a full-stack project, where the bundle is built
    /// once and the browser is a thin client.
    pub runtime_server_url: Option<String>,
    /// [`build_web::BuildOptions::hot_patch`] — build a base module a
    /// subsecond patch can link against. Dev-only; set by the `--local`
    /// web loop when the hot-patch tier is armed.
    pub hot_patch: bool,
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
    // A one-shot build has no browser to spare a reload; whether the
    // passes ran is the dev loop's concern.
    build_wasm(dir, opts).map(|_| ())
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
            robot_relay_url: None,
            head_script: None,
            runtime_server_url: None,
            hot_patch: false,
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
    // The initial build's result is irrelevant: gen 1 is the browsers'
    // first bundle whether the passes ran or were skipped.
    let initial = build_wasm(dir, &opts).context("initial web build failed")?;
    signal.set(1);

    let dir_owned = dir.to_path_buf();
    thread::Builder::new()
        .name("idealyst-watch".into())
        .spawn(move || watch_loop(dir_owned, signal, opts, initial))
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

/// The hot-patch tier's state across one dev session.
///
/// A [`build_web::hotpatch_build::WasmPatchBuilder`] is valid for
/// exactly one base build: it holds an index of the served module's
/// function table, and every rebuild renumbers that table. So the
/// builder is dropped on each rebuild and made again on the next body
/// edit — lazily, because indexing a debug-profile wasm module with
/// every function in its table is the expensive part and a session that
/// never edits a body should never pay for it.
struct HotPatchBase {
    /// Where the page fetches from: the staged bundle for a full-stack
    /// project, the project directory for a static one.
    serve_root: PathBuf,
    crate_name: String,
    armed: bool,
    artifact: build_web::BuildArtifact,
    builder: Option<build_web::hotpatch_build::WasmPatchBuilder>,
    /// Set after a failure that will recur — a base built without the
    /// capture wrapper, say. One message, then the tier stays quiet and
    /// every body edit rebuilds.
    retired: Option<String>,
}

impl HotPatchBase {
    fn new(dir: &Path, opts: &BuildOptions, artifact: build_web::BuildArtifact) -> Self {
        Self {
            // A full-stack project's own server hands out the staged
            // bundle, so a patch written into the project directory
            // would 404. A static project is served from the project
            // directory itself.
            serve_root: opts.bundle_out_dir.clone().unwrap_or_else(|| dir.to_path_buf()),
            crate_name: dir_package_name(dir),
            armed: opts.hot_patch,
            artifact,
            builder: None,
            retired: None,
        }
    }

    fn rebuilt(&mut self, artifact: build_web::BuildArtifact) {
        self.artifact = artifact;
        // The table the old builder indexed no longer describes the
        // module the page is running.
        self.builder = None;
    }

    /// Build a patch for this save, or say why it cannot.
    fn patch(&mut self) -> std::result::Result<build_web::hotpatch_build::BuiltPatch, String> {
        if !self.armed {
            return Err(
                "the hot-patch tier is off for this session (`idealyst dev --web --local` \
                 arms it)"
                    .to_string(),
            );
        }
        if let Some(why) = &self.retired {
            return Err(why.clone());
        }
        if self.builder.is_none() {
            let Some(captures) = self.artifact.captures_dir.clone() else {
                let why = "this base was built without the rustc capture wrapper, so there is \
                           no invocation to replay"
                    .to_string();
                self.retired = Some(why.clone());
                return Err(why);
            };
            match build_web::hotpatch_build::WasmPatchBuilder::new(
                &self.artifact.served_wasm,
                self.artifact.symbol_aliases.as_deref(),
                captures,
                self.crate_name.clone(),
                build_web::patches_dir(&self.serve_root),
            ) {
                Ok(builder) => {
                    eprintln!(
                        "[hotpatch] base indexed: {} functions reachable through the table \
                         ({} of them a second name for one of the others)",
                        builder.base_table_size(),
                        builder.alias_count(),
                    );
                    self.builder = Some(builder);
                }
                // Not retired: a base that could not be indexed once may
                // index fine after the next rebuild, and retiring here
                // would disarm the tier for the whole session over one
                // bad build.
                Err(e) => return Err(format!("{e:#}")),
            }
        }
        self.builder
            .as_ref()
            .expect("just built")
            .build()
            .map_err(|e| format!("{e:#}"))
    }

    /// The SSE payload for a built patch: where the page fetches the
    /// module from, and the table to apply.
    fn event_json(&self, patch: &build_web::hotpatch_build::BuiltPatch) -> Result<String> {
        let file = patch
            .path
            .file_name()
            .and_then(|f| f.to_str())
            .context("the patch has no file name")?;
        Ok(serde_json::to_string(&serde_json::json!({
            "url": build_web::patch_url_path(file),
            "table": patch.jump_table.to_subsecond(),
        }))?)
    }
}

/// Watch every local source root in `dir`'s dependency closure (see
/// [`watch_roots`]) — its own `src/` + `Cargo.toml`, plus those of
/// each path / workspace-member crate it pulls in. Each debounced
/// event batch triggers one `wasm-pack` build; the build is
/// synchronous on this thread so events arriving while a build is in
/// flight queue up naturally on the channel and we collapse them by
/// draining before the next build.
fn watch_loop(
    dir: PathBuf,
    signal: Arc<ReloadSignal>,
    opts: BuildOptions,
    initial: build_web::BuildArtifact,
) {
    // The hot-patch tier's builder, valid for the life of ONE base
    // build: it holds an index of the served module's function table,
    // which every rebuild invalidates. Rebuilt lazily on the first body
    // edit after each rebuild, so a session that never makes one never
    // pays to parse the module.
    let mut base = HotPatchBase::new(&dir, &opts, initial);
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

    // The descriptor set describing the build now running. Written
    // HERE, before the first save, rather than only after a rebuild:
    // the initial build already happened by the time this loop starts,
    // and without an archive from it the very first save of every
    // session would have nothing to diff against and would rebuild.
    //
    // Kept across saves and advanced after each decided patch, so the
    // NEXT save diffs against what is actually running rather than
    // against the source the last compiler saw.
    let package = dir_package_name(&dir);
    if let Err(e) = overlay::write_for(&dir, &dir) {
        eprintln!("[dev-reload] no descriptor set for this build: {e}");
    }
    let mut archive = overlay_decide::load_archive(&dir, &package);

    while let Ok(events) = rx.recv() {
        let changed_paths: Vec<PathBuf> = match &events {
            Ok(evs) => evs.iter().map(|e| e.path.clone()).collect(),
            Err(_) => Vec::new(),
        };
        drain(&rx);
        if events.is_err() {
            continue;
        }
        // Absorb the rest of the burst before starting the build —
        // otherwise a multi-file edit queues one full rebuild per file.
        let folded = settle(&rx);

        // Can this save skip the compiler? Decided BEFORE anything
        // expensive starts, from the archive plus the new source. See
        // `overlay_decide` for why the answer is conservative.
        if let Some(set) = archive.as_ref() {
            let started = std::time::Instant::now();
            let changed = read_changed(&dir, &changed_paths);
            // A premint session baked its class names from every
            // `stylesheet!` at start; a sheet edit there must rebuild even
            // when the shape says body-only.
            let premint = opts.premint || opts.premint_only;
            match overlay_decide::decide_with(Some(set), &changed, premint) {
                overlay_decide::Decision::Patch(patches) => {
                    let count = patches.len();
                    for patch in &patches {
                        match serde_json::to_string(&overlay_decide::wire_payload(patch)) {
                            Ok(json) => {
                                signal.push_patch(json);
                            }
                            Err(e) => eprintln!("[dev-reload] cannot encode patch: {e}"),
                        }
                    }
                    if let Some(set) = archive.as_mut() {
                        overlay_decide::advance_archive(set, &changed);
                    }
                    eprintln!(
                        "[dev] patched {count} site(s) in {} ms, no rebuild",
                        started.elapsed().as_millis()
                    );
                    drain(&rx);
                    continue;
                }
                overlay_decide::Decision::Unchanged if !changed.is_empty() => {
                    eprintln!("[dev] no UI or code change in this save, no rebuild");
                    drain(&rx);
                    continue;
                }
                overlay_decide::Decision::Unchanged => {}
                overlay_decide::Decision::HotPatch(files) => {
                    // A body edit: new CODE, which the overlay cannot
                    // carry. Build a wasm patch and send it, so the page
                    // swaps the function bodies and rebuilds its tree
                    // without losing what is on screen.
                    //
                    // Any failure falls through to a rebuild, which is
                    // always correct. A patch that half-applies would
                    // leave the page running code the source no longer
                    // describes, with nothing to say so.
                    match base.patch() {
                        Ok(patch) => match base.event_json(&patch) {
                            Ok(json) => {
                                signal.push_hot_patch(json);
                                // A FULL rescan, not `advance_archive`
                                // over the saved files: a hot patch
                                // re-emits every file of the crate, so a
                                // file outside this save can come back
                                // with new site keys too, and the next
                                // save must diff against what is running.
                                archive = rescan_archive(&dir, &package);
                                eprintln!(
                                    "[hotpatch] {} · {} function(s) redirected · {}",
                                    files.join(", "),
                                    patch.jump_table.map.len(),
                                    patch.timing_line(),
                                );
                                drain(&rx);
                                continue;
                            }
                            Err(e) => eprintln!(
                                "[hotpatch] cannot encode the patch event ({e:#}); rebuilding"
                            ),
                        },
                        Err(why) => eprintln!(
                            "[hotpatch] {} changed inside function bodies, but no patch: \
                             {why}; rebuilding",
                            files.join(", "),
                        ),
                    }
                }
                overlay_decide::Decision::Rebuild(why) => {
                    eprintln!("[dev] rebuilding: {why}");
                }
            }
        }

        if folded > 0 {
            eprintln!("[dev-reload] change detected (+{folded} more), rebuilding…");
        } else {
            eprintln!("[dev-reload] change detected, rebuilding…");
        }
        match build_wasm(&dir, &opts).map(|a| {
            let changed = a.wasm_changed;
            base.rebuilt(a);
            changed
        }) {
            Ok(true) => {
                let new_gen = signal.bump();
                eprintln!("[dev-reload] rebuilt — gen={new_gen}");
            }
            // Cargo produced nothing new and the packaging passes were
            // skipped, so the served bundle is the one the browser
            // already has. A premint session is the exception: its
            // `pkg/premint.css` is regenerated from a native dump on
            // every rebuild and can move without the wasm moving.
            Ok(false) if !(opts.premint || opts.premint_only || opts.premint_report) => {
                eprintln!("[dev-reload] wasm unchanged — packaging skipped, nothing to reload")
            }
            Ok(false) => {
                let new_gen = signal.bump();
                eprintln!("[dev-reload] wasm unchanged, premint refreshed — gen={new_gen}");
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

        // A rebuild regenerates the descriptor set from source, which
        // is also what drops every staged patch: the new binary already
        // has the edits compiled in, so re-sending them would be
        // applying the same change twice.
        archive = rescan_archive(&dir, &package);

        // Coalesce anything queued during the build — wasm-pack
        // writes to `pkg/` (not watched) and cargo touches
        // `target/` (not watched), but defensively draining keeps
        // editor save-bursts from triggering N consecutive builds.
        drain(&rx);
    }
}

/// The crate's package name, for locating its descriptor archive.
fn dir_package_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("Cargo.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
        .and_then(|m| {
            m.get("package")?.get("name")?.as_str().map(str::to_string)
        })
        .unwrap_or_default()
}

/// Read the changed files the decision needs, as package-relative
/// paths.
///
/// A path outside the crate — a watched path dependency — is skipped
/// here, which means the decision never sees it and the save falls to a
/// rebuild. That is the right answer: the archive describes THIS crate,
/// and a dependency's sites are compiled into a different artifact.
fn read_changed(dir: &Path, paths: &[PathBuf]) -> Vec<overlay_decide::ChangedFile> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(dir) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !seen.insert(relative.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        out.push(overlay_decide::ChangedFile { path: relative, text });
    }
    out
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

/// Run one bundle build.
///
/// Returns the whole artifact rather than just "did the wasm move":
/// the hot-patch tier needs the served module's path and the captures
/// directory, and both are decided inside the build. A caller that only
/// wants the reload decision reads `wasm_changed`.
fn build_wasm(dir: &Path, opts: &BuildOptions) -> Result<build_web::BuildArtifact> {
    // Delegate to `build_web::build` — it generates the wrapper,
    // runs wasm-pack against it, and copies `pkg/` into `dir`.
    // Same path `idealyst build web` uses; the dev loop is just
    // "do that, but on debounced file changes".
    build_web::build(dir, to_build_web_options(opts))
}

/// Map the dev-loop options onto a full `build_web::BuildOptions`.
/// Split out of [`build_wasm`] so the premint/hydrate interaction is
/// unit-testable without running a build.
fn to_build_web_options(opts: &BuildOptions) -> build_web::BuildOptions {
    let premint = opts.premint || opts.premint_only || opts.premint_report;
    build_web::BuildOptions {
        wasm_split: opts.wasm_split,
        hot_patch: opts.hot_patch,
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
        // Re-injected on every rebuild because `stage_bundle` copies
        // the project's `index.html` fresh each time.
        robot_relay_url: opts.robot_relay_url.clone(),
        head_script: opts.head_script.clone(),
        runtime_server_url: opts.runtime_server_url.clone(),
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

/// Re-derive the `ui!` descriptor archive from the crate's source as it
/// is on disk now.
///
/// Used after anything that puts the WHOLE crate's current source into
/// the running program: a rebuild, and a hot patch, which re-emits every
/// file of the crate rather than only the ones in the save.
fn rescan_archive(dir: &Path, package: &str) -> Option<overlay::DescriptorSet> {
    match overlay::write_for(dir, dir) {
        Ok(_) => overlay_decide::load_archive(dir, package),
        Err(e) => {
            eprintln!("[dev-reload] no descriptor set for this build: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Regression: after a hot patch the archive was advanced over the
    /// SAVED files only, but a patch re-emits every file of the crate.
    /// A file edited outside the save (by a formatter, a second editor,
    /// a branch switch the watcher folded away) came back running new
    /// site keys the archive never saw, and the next literal save diffed
    /// against a stale picture.
    #[test]
    fn regression_the_archive_after_a_hot_patch_covers_files_outside_the_save() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"rescan-probe\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "mod a;\nmod b;\n").unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() { ui! { text { \"a\" } } }\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn b() { ui! { text { \"b\" } } }\n").unwrap();

        let before = rescan_archive(dir, "rescan-probe").expect("an archive");
        let b_before = before.files.get("src/b.rs").expect("b.rs scanned").content.clone();

        // `b.rs` changes, but the save that produced the patch was a.rs.
        std::fs::write(
            dir.join("src/b.rs"),
            "fn b() { ui! { text { \"b\" } } }\nfn c() { ui! { text { \"c\" } } }\n",
        )
        .unwrap();
        let after = rescan_archive(dir, "rescan-probe").expect("an archive");
        assert_ne!(
            after.files.get("src/b.rs").expect("b.rs scanned").content,
            b_before,
            "the archive must describe b.rs as it is now, not as it was at the last build"
        );
        assert!(
            after.sites.len() > before.sites.len(),
            "b.rs's new site is missing: {} -> {}",
            before.sites.len(),
            after.sites.len()
        );
    }

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
            robot_relay_url: None,
            head_script: None,
            runtime_server_url: None,
            hot_patch: false,
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
    /// Both tiers share ONE ordered channel. A save that changed a
    /// literal and a body produces an overlay patch and a hot patch, and
    /// the page must receive them in the order the dev loop decided
    /// them — two channels could not promise that, and applying the
    /// overlay's data to a tree the hot patch has not yet rebuilt shows
    /// the old body with the new label.
    #[test]
    fn the_two_patch_tiers_keep_their_decided_order() {
        let signal = ReloadSignal::new();
        signal.push_patch("{\"a\":1}".into());
        signal.push_hot_patch("{\"b\":2}".into());
        signal.push_patch("{\"c\":3}".into());

        let kinds: Vec<_> = signal.patches_since(0).iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![PatchKind::Overlay, PatchKind::Hot, PatchKind::Overlay],
        );
        let seqs: Vec<_> = signal.patches_since(0).iter().map(|p| p.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3], "one sequence, not one per tier");
    }

    /// A listener catches up by sequence regardless of kind, so a page
    /// that missed a hot patch is not handed it again after it reloads.
    #[test]
    fn catching_up_skips_what_a_listener_already_saw() {
        let signal = ReloadSignal::new();
        signal.push_patch("{}".into());
        signal.push_hot_patch("{}".into());
        assert_eq!(signal.patches_since(1).len(), 1);
        assert_eq!(signal.patches_since(2).len(), 0);
    }

}
