//! The dev loop's lifecycle, as data.
//!
//! `idealyst dev` used to report what it was doing with `eprintln!` from
//! six crates. That is fine for a human reading a scrolling terminal and
//! useless for anything else: an editor cannot tell "a patch landed"
//! from "a build failed" without scraping text, and a status panel
//! cannot show "building, 212/480 crates" at all, because nothing ever
//! said so.
//!
//! So every fact the dev loop reports is a [`DevEvent`], emitted through
//! a [`Reporter`], and fanned out to [`Sink`]s:
//!
//! - [`PlainLines`] renders each event as the line the CLI printed
//!   before events existed (see [`plain`]), so a terminal, a CI log or
//!   the MCP dev-runner's log file sees exactly what it always did;
//! - [`JsonLines`] writes one JSON object per event — the hook for
//!   editors and tools (`idealyst dev --events-file <path>`);
//! - [`Queue`] buffers events for a consumer that drains them on its own
//!   schedule — the interactive panel.
//!
//! Every event is wrapped in an [`Envelope`] carrying a session-relative
//! sequence number and a monotonic timestamp, so a consumer can order
//! events from several threads and measure intervals without trusting
//! the wall clock.
//!
//! The migration is incremental by design: [`DevEvent::Log`] and
//! [`DevEvent::Output`] carry any line nobody has typed yet, tagged by
//! its source, so nothing is dropped while more of the loop learns to
//! speak in typed events.
//!
//! # Hooking in
//!
//! The stream is an open hook, not a closed set of outputs. Four ways in,
//! all carrying the same versioned objects ([`Envelope`], schema from
//! [`json_schema`] / `idealyst dev --events-schema`):
//!
//! 1. **In process**: implement [`Sink`] and [`Reporter::subscribe`] it —
//!    under `idealyst dev`, the session's reporter is [`global`]. Every
//!    built-in output (the plain terminal lines, `--events-file`, the
//!    panel, the page overlay, the HTTP stream) is an ordinary sink, so
//!    the trait is provably enough.
//! 2. **Over HTTP**: `GET /__idealyst/events` on any dev server, or on the
//!    session's own events port (its URL is in `.idealyst/events.url`):
//!    Server-Sent Events, a snapshot of the session first
//!    ([`snapshot`]), then every event live ([`broadcast`]).
//! 3. **From a file or a pipe**: `idealyst dev --events-file <path>` or
//!    `--events json` (stdout), one object per line.
//! 4. **As a command**: `[hooks]` in `dev.toml` maps an event kind to a
//!    shell command that receives the event's JSON on stdin.
//!
//! A sink that counts failed builds:
//!
//! ```
//! use std::sync::{Arc, Mutex};
//! use dev_events::{BuildOutcome, DevEvent, Envelope, Reporter, Sink};
//!
//! #[derive(Default)]
//! struct Failures(Mutex<Vec<String>>);
//!
//! impl Sink for Failures {
//!     fn emit(&self, envelope: &Envelope) {
//!         if let DevEvent::BuildFinished { outcome: BuildOutcome::Failed { error }, .. } =
//!             &envelope.event
//!         {
//!             self.0.lock().unwrap().push(error.clone());
//!         }
//!     }
//! }
//!
//! // Under `idealyst dev` this is `dev_events::global()`.
//! let session = Reporter::new();
//! let failures = Arc::new(Failures::default());
//! session.add_sink(failures.clone()); // or `subscribe(Box::new(..))`
//!
//! session.emit(DevEvent::BuildFinished {
//!     target: "web".into(),
//!     outcome: BuildOutcome::Failed { error: "cargo exited with 101".into() },
//!     ms: 3100,
//! });
//! assert_eq!(*failures.0.lock().unwrap(), vec!["cargo exited with 101".to_string()]);
//! ```
//!
//! A sink runs on the emitting thread with the reporter's fan-out lock
//! held (that is what keeps every sink in sequence order), so it must be
//! quick and must not emit itself — hand slow work to a thread, as the
//! `[hooks]` sink does.

pub mod broadcast;
pub mod cargo;
pub mod child;
pub mod plain;
pub mod page;
pub mod process;
pub mod snapshot;

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Where the app's code runs during the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// `--local`: each target runs the app itself.
    Local,
    /// The default: the app runs in a host-side sidecar and targets are
    /// thin wire clients.
    RuntimeServer,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Local => "local",
            Mode::RuntimeServer => "runtime-server",
        }
    }
}

/// Whether a body edit can be applied as a wasm hot patch this session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HotTier {
    Armed,
    Off { reason: String },
}

/// What kind of server a [`DevEvent::ServerReady`] announces. Each has
/// its own legacy line, and a consumer may care which (the livereload
/// server is the one a browser should open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// `dev --web --local`: static files plus the livereload stream.
    Livereload,
    /// `dev --web` in runtime-server mode: static files plus the
    /// sidecar's URL.
    RuntimeServerBridged,
    /// A full-stack project's own server (bundle and API).
    FullStack,
    /// The reload/overlay stream on its own port, beside a full-stack
    /// server.
    ReloadStream,
    /// The session's event stream (`/__idealyst/events`) on its own port.
    Events,
}

/// What the watcher decided a save needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "tier", rename_all = "snake_case")]
pub enum Decision {
    /// Literal/data edits to `ui!` sites: pushed as overlay patches.
    Overlay { sites: usize },
    /// Edits inside function bodies: a wasm hot patch re-emitting these
    /// crates.
    HotPatch { crates: Vec<String>, files: Vec<String> },
    /// Anything else. `reason` is `None` when the save named no file the
    /// archives describe (a deleted file, a path under no crate) — the
    /// rebuild is the only honest answer and there is nothing more to say.
    Rebuild { reason: Option<String> },
    /// The save changed nothing that reaches the program.
    Unchanged,
}

impl Decision {
    /// Short tier name for display.
    pub fn tier(&self) -> &'static str {
        match self {
            Decision::Overlay { .. } => "overlay",
            Decision::HotPatch { .. } => "hot patch",
            Decision::Rebuild { .. } => "rebuild",
            Decision::Unchanged => "unchanged",
        }
    }
}

/// Why a build started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum BuildCause {
    /// The session's first build.
    Initial,
    /// A save the watcher could not patch. `folded` counts the extra
    /// event batches the quiet window absorbed into this one build.
    Save { folded: usize },
    /// Someone asked (the panel's `r`).
    Forced,
    /// A build with no watcher behind it: runtime-server mode's thin web
    /// client, a full-stack wire-mode bundle.
    OneShot,
}

/// How a build ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BuildOutcome {
    /// The initial build finished; browsers get generation `gen`.
    Ready { gen: u64 },
    /// A rebuild produced a new bundle; pages reload to generation `gen`.
    Reloaded { gen: u64 },
    /// Cargo produced an identical module; nothing to reload.
    Unchanged,
    /// The module did not move but the premint stylesheet did.
    PremintRefreshed { gen: u64 },
    /// The build failed. The page keeps running the last good bundle.
    Failed { error: String },
}

/// How the runtime-server host applied a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SidecarUpdate {
    /// Re-emitted the user crate and rebound the sidecar's jump table,
    /// in place: clients stay attached.
    HotPatch,
    /// Rebuilt and restarted the sidecar; sessions are replayed onto it.
    Respawn,
}

/// One timed step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Timing {
    pub name: String,
    pub ms: u64,
}

impl Timing {
    pub fn new(name: impl Into<String>, d: std::time::Duration) -> Self {
        Self { name: name.into(), ms: d.as_millis() as u64 }
    }
}

/// One crate a hot patch re-emitted, and how long its replay took —
/// `None` when its objects were reused from an earlier replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CrateTiming {
    pub name: String,
    pub ms: Option<u64>,
}

/// A rustc diagnostic, from cargo's JSON message stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Diagnostic {
    /// `error`, `warning`, `note`, `help`, `failure-note`, …
    pub level: String,
    /// The one-line message.
    pub message: String,
    /// The lint or error code (`E0308`, `unused_variables`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The primary span's file, as rustc printed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The package the diagnostic came from (cargo's `package_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The full human rendering, without colour codes.
    pub rendered: String,
    /// The same rendering with cargo's colour codes, when cargo was asked
    /// for colour. The plain sink prints this, so a terminal session
    /// looks exactly as it did when cargo wrote to it directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansi: Option<String>,
}

impl Diagnostic {
    /// `file:line:column`, or as much of it as is known.
    pub fn location(&self) -> Option<String> {
        let file = self.file.as_deref()?;
        Some(match (self.line, self.column) {
            (Some(l), Some(c)) => format!("{file}:{l}:{c}"),
            (Some(l), None) => format!("{file}:{l}"),
            _ => file.to_string(),
        })
    }

    pub fn is_error(&self) -> bool {
        self.level == "error"
    }
}

/// What a page reported back after the dev loop pushed something to it.
///
/// These are the facts the browser used to print only to its own
/// console (`[idealyst] hot patch: 3 function(s) redirected`), carried
/// back over the reload stream's ack endpoint so the terminal knows the
/// save actually landed — not just that it was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAck {
    /// A page connected to the reload stream, running generation `gen`.
    Connected { gen: u64 },
    /// A page is reloading onto generation `gen`.
    Reloading { gen: u64 },
    /// An overlay patch was applied: `applied` nodes updated in place,
    /// `refused` waiting for their site's next render. Both are absent
    /// when the bundle predates the counts.
    Overlay {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        applied: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<u64>,
    },
    /// A hot patch was applied and the tree rebuilt against it.
    HotPatch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redirected: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        carried: Option<u64>,
    },
    /// Something the page was sent did not apply. `what` is `overlay` or
    /// `hot_patch`; the page reloads after a failed hot patch.
    Failed { what: String, error: String },
}

/// Everything the dev loop reports.
///
/// `target` names the row a status view files the event under: `web`
/// for the browser bundle, `server` for a full-stack server's watcher,
/// the platform name for native targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DevEvent {
    /// The session is configured and about to start its targets.
    SessionStarted {
        app: String,
        targets: Vec<String>,
        mode: Mode,
        hot_tier: HotTier,
        /// The verbose session log every event is also written to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        log_file: Option<String>,
    },
    /// A server is accepting connections at `url`.
    ServerReady { target: String, kind: ServerKind, url: String },
    /// The watcher is watching `roots`. `rewatch` is true when a save
    /// changed the dependency graph and the set was re-resolved.
    Watching { target: String, roots: Vec<String>, rewatch: bool },
    /// Files changed. Emitted once per coalesced save, before the
    /// decision.
    ChangeDetected {
        target: String,
        paths: Vec<String>,
        /// The packages those paths belong to, where known.
        crates: Vec<String>,
        /// Extra event batches folded into this save.
        folded: usize,
    },
    /// What the save needs.
    Decided { target: String, decision: Decision },
    /// Overlay patches were pushed to connected pages.
    OverlayPushed { target: String, sites: usize, ms: u64 },
    /// A hot patch was built and pushed.
    PatchBuilt {
        target: String,
        files: Vec<String>,
        crates: Vec<CrateTiming>,
        redirected: usize,
        steps: Vec<Timing>,
        /// Crates of the plan the base never compiled.
        skipped: Vec<String>,
        /// Size of the served patch module.
        bytes: u64,
        ms: u64,
    },
    /// A body edit could not be patched; the watcher rebuilds instead.
    PatchFailed { target: String, files: Vec<String>, reason: String },
    /// A build started. The cause is inlined: `"cause": "save",
    /// "folded": 0`.
    BuildStarted {
        target: String,
        #[serde(flatten)]
        cause: BuildCause,
    },
    /// A build stage started (`cargo`, `wasm-bindgen`, `wasm-split`, …).
    StageStarted { target: String, stage: String },
    /// A build stage finished.
    StageFinished { target: String, stage: String, ms: u64 },
    /// Cargo finished compiling another unit. `compiled` counts distinct
    /// packages seen so far (fresh ones included); `total` is the size of
    /// the build's dependency closure from `cargo metadata`, when it has
    /// been resolved — see [`cargo`] for why not cargo's own unit count.
    CargoProgress {
        target: String,
        compiled: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<String>,
    },
    /// rustc said something.
    Diagnostic { target: String, diagnostic: Diagnostic },
    /// The bundler's per-stage summary for one build.
    BuildTimed { target: String, stages: Vec<Timing>, total_ms: u64 },
    /// A build ended. The outcome is inlined: `"outcome": "failed",
    /// "error": "…"`.
    BuildFinished {
        target: String,
        #[serde(flatten)]
        outcome: BuildOutcome,
        ms: u64,
    },
    /// A page reported back — or, in runtime-server mode, the sidecar,
    /// which applies overlay and hot patches to its own mounted tree and
    /// reports the same facts a page would.
    PageAck { target: String, ack: PageAck },
    /// Runtime-server mode: the host applied a save to the sidecar.
    SidecarApplied {
        target: String,
        how: SidecarUpdate,
        ms: u64,
        /// Why a respawn was needed (`rebuild`, `force_respawn`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Something went wrong that the loop survives.
    Warning { source: String, message: String },
    /// Something failed.
    Error { source: String, message: String },
    /// A dev-loop line with no typed event yet, printed as
    /// `[source] line`.
    Log { source: String, line: String },
    /// A line of subprocess output (cargo, wasm-bindgen, a sidecar),
    /// printed verbatim.
    Output { source: String, line: String },
}

/// The event schema's major version, carried in every envelope as `v`.
///
/// The schema is a contract: within one major version changes are
/// ADDITIVE only — a new event type, a new optional field, a new enum
/// value a consumer can ignore. Renaming or removing a field, changing a
/// type, or giving an existing field a new meaning bumps this number.
/// Consumers should ignore event types and fields they do not know.
/// `idealyst dev --events-schema` prints the JSON Schema ([`json_schema`]).
pub const SCHEMA_VERSION: u32 = 1;

/// An event with its place in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Envelope {
    /// The schema's major version ([`SCHEMA_VERSION`]).
    pub v: u32,
    /// Session-relative sequence number, from 1, gap-free per reporter.
    pub seq: u64,
    /// Milliseconds since the reporter was created, from a monotonic
    /// clock.
    pub at_ms: u64,
    #[serde(flatten)]
    pub event: DevEvent,
}

/// Something that consumes events.
///
/// Called on whichever thread emitted the event, with the reporter's
/// fan-out lock held — so a sink sees events in sequence order, and must
/// not block for long.
pub trait Sink: Send + Sync {
    fn emit(&self, envelope: &Envelope);
}

struct Hub {
    origin: Instant,
    seq: AtomicU64,
    /// Held across assign-and-fan-out, so every sink receives events in
    /// `seq` order even when several threads emit at once.
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
}

/// A cheap, cloneable handle every producer on the dev path emits
/// through. Clones share one sequence and one set of sinks.
#[derive(Clone)]
pub struct Reporter {
    hub: Arc<Hub>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sinks = self.hub.sinks.lock().map(|s| s.len()).unwrap_or(0);
        write!(f, "Reporter({sinks} sink(s))")
    }
}

impl Default for Reporter {
    /// Plain lines on stderr: exactly what a producer printed before it
    /// took a reporter. A caller that never wires one up keeps the old
    /// behaviour.
    fn default() -> Self {
        Self::plain_stderr()
    }
}

impl Reporter {
    /// A reporter with no sinks. Events are numbered and dropped until a
    /// sink is added.
    pub fn new() -> Self {
        Self {
            hub: Arc::new(Hub {
                origin: Instant::now(),
                seq: AtomicU64::new(0),
                sinks: Mutex::new(Vec::new()),
            }),
        }
    }

    /// A reporter printing [`PlainLines`] to stderr.
    pub fn plain_stderr() -> Self {
        let r = Self::new();
        r.add_sink(Arc::new(PlainLines::stderr()));
        r
    }

    /// Add a sink. Events emitted from now on reach it.
    pub fn add_sink(&self, sink: Arc<dyn Sink>) {
        if let Ok(mut sinks) = self.hub.sinks.lock() {
            sinks.push(sink);
        }
    }

    /// Subscribe `sink` to every event emitted from now on — the hook
    /// for anything in-process that wants the session's events. The
    /// built-in sinks are ordinary subscribers too.
    pub fn subscribe(&self, sink: Box<dyn Sink>) {
        self.add_sink(Arc::from(sink));
    }

    /// Replace every sink. The CLI uses this when the interactive panel
    /// takes over the terminal: the plain stderr sink comes out, the
    /// panel's queue goes in, and every clone already handed out follows.
    pub fn set_sinks(&self, new: Vec<Arc<dyn Sink>>) {
        if let Ok(mut sinks) = self.hub.sinks.lock() {
            *sinks = new;
        }
    }

    /// Emit one event to every sink.
    pub fn emit(&self, event: DevEvent) {
        // A poisoned lock means a sink panicked mid-write. Keep
        // reporting through the others rather than going silent.
        let sinks = match self.hub.sinks.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        let envelope = Envelope {
            v: SCHEMA_VERSION,
            seq: self.hub.seq.fetch_add(1, Ordering::SeqCst) + 1,
            at_ms: self.hub.origin.elapsed().as_millis() as u64,
            event,
        };
        for sink in sinks.iter() {
            sink.emit(&envelope);
        }
    }

    /// `[source] line`, for a line that has no typed event yet.
    pub fn log(&self, source: impl Into<String>, line: impl Into<String>) {
        self.emit(DevEvent::Log { source: source.into(), line: line.into() });
    }

    /// A verbatim line of subprocess output.
    pub fn output(&self, source: impl Into<String>, line: impl Into<String>) {
        self.emit(DevEvent::Output { source: source.into(), line: line.into() });
    }

    pub fn warn(&self, source: impl Into<String>, message: impl Into<String>) {
        self.emit(DevEvent::Warning { source: source.into(), message: message.into() });
    }

    pub fn error(&self, source: impl Into<String>, message: impl Into<String>) {
        self.emit(DevEvent::Error { source: source.into(), message: message.into() });
    }
}

/// The JSON Schema every emitted object ([`Envelope`]) validates
/// against, with the schema version in `$id` and `x-schema-version`.
#[cfg(feature = "schema")]
pub fn json_schema() -> serde_json::Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(Envelope))
        .expect("a derived schema serializes");
    if let Some(obj) = schema.as_object_mut() {
        obj.insert(
            "$id".into(),
            format!("https://idealyst.dev/schemas/dev-events/v{SCHEMA_VERSION}.json").into(),
        );
        obj.insert("title".into(), "idealyst dev event".into());
        obj.insert("x-schema-version".into(), SCHEMA_VERSION.into());
    }
    // `v` is not any integer: it is THIS major version. A consumer that
    // validates rejects a stream from an incompatible `idealyst dev`.
    if let Some(v) = schema.pointer_mut("/properties/v") {
        *v = serde_json::json!({
            "description": "The schema's major version.",
            "const": SCHEMA_VERSION,
            "type": "integer",
        });
    }
    schema
}

/// Whether a file name is an editor's or a tool's scratch file rather
/// than a file someone saved: a `.name.swp`, an `.!12345!name.rs` from
/// `sed -i`, a `name.rs~` backup, an emacs `#name#`. Watchers see those
/// beside every save; a [`DevEvent::ChangeDetected`] names the saved files
/// without them.
pub fn is_scratch_file(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('#') || name.ends_with('~')
}

static GLOBAL: OnceLock<Reporter> = OnceLock::new();

/// Install the process-wide reporter. The first call wins; later calls
/// return `false` and change nothing. The CLI installs the session's
/// reporter here so code with no handle threaded to it (a log shim, a
/// leaf helper) still reaches the session's sinks.
pub fn install_global(reporter: Reporter) -> bool {
    GLOBAL.set(reporter).is_ok()
}

/// The process-wide reporter: the installed one, or plain stderr.
pub fn global() -> Reporter {
    GLOBAL.get_or_init(Reporter::plain_stderr).clone()
}

/// How much a [`PlainLines`] sink prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Only events that correspond to a line the CLI printed before
    /// events existed, rendered as that line. The terminal default.
    Legacy,
    /// Also events that had no line (page acks, change detection, stage
    /// boundaries, cargo progress), each prefixed with its session time.
    /// The session log file.
    Verbose,
}

/// Renders each event as a plain text line. See [`plain::render`].
pub struct PlainLines<W: Write + Send> {
    out: Mutex<W>,
    verbosity: Verbosity,
}

impl PlainLines<std::io::Stderr> {
    pub fn stderr() -> Self {
        Self::new(std::io::stderr(), Verbosity::Legacy)
    }
}

impl<W: Write + Send> PlainLines<W> {
    pub fn new(out: W, verbosity: Verbosity) -> Self {
        Self { out: Mutex::new(out), verbosity }
    }

    /// Take the writer back (tests).
    pub fn into_inner(self) -> W {
        match self.out.into_inner() {
            Ok(w) => w,
            Err(p) => p.into_inner(),
        }
    }
}

impl<W: Write + Send> Sink for PlainLines<W> {
    fn emit(&self, envelope: &Envelope) {
        let text = match self.verbosity {
            Verbosity::Legacy => plain::render(&envelope.event),
            Verbosity::Verbose => plain::render_verbose(envelope),
        };
        let Some(text) = text else { return };
        if let Ok(mut out) = self.out.lock() {
            // Write errors are dropped: a closed stderr must not take the
            // dev loop down with it.
            let _ = writeln!(out, "{text}");
            let _ = out.flush();
        }
    }
}

/// One JSON object per event, one per line.
pub struct JsonLines<W: Write + Send> {
    out: Mutex<W>,
}

impl<W: Write + Send> JsonLines<W> {
    pub fn new(out: W) -> Self {
        Self { out: Mutex::new(out) }
    }
}

impl JsonLines<std::io::Stdout> {
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }
}

impl JsonLines<std::io::BufWriter<std::fs::File>> {
    /// Create (truncate) `path`, making its parent directory.
    pub fn file(path: &std::path::Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self::new(std::io::BufWriter::new(std::fs::File::create(path)?)))
    }
}

impl<W: Write + Send> Sink for JsonLines<W> {
    fn emit(&self, envelope: &Envelope) {
        let Ok(line) = serde_json::to_string(envelope) else { return };
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{line}");
            // Flushed per event: a tool tailing the file must see an
            // event when it happens, not when a buffer fills.
            let _ = out.flush();
        }
    }
}

/// Buffers events for a consumer that drains them itself — the panel,
/// which runs on its own frame clock, and tests.
#[derive(Clone, Default)]
pub struct Queue {
    inner: Arc<Mutex<VecDeque<Envelope>>>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything emitted since the last drain, oldest first.
    pub fn drain(&self) -> Vec<Envelope> {
        match self.inner.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Sink for Queue {
    fn emit(&self, envelope: &Envelope) {
        if let Ok(mut q) = self.inner.lock() {
            q.push_back(envelope.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_sequence_and_one_set_of_sinks() {
        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        let r2 = r.clone();
        r.log("dev", "a");
        r2.log("dev", "b");
        let seqs: Vec<u64> = q.drain().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn set_sinks_redirects_every_clone() {
        let r = Reporter::new();
        let first = Queue::new();
        r.add_sink(Arc::new(first.clone()));
        let handed_out = r.clone();
        let second = Queue::new();
        r.set_sinks(vec![Arc::new(second.clone())]);
        handed_out.log("dev", "after");
        assert!(first.is_empty());
        assert_eq!(second.len(), 1);
    }

    #[test]
    fn concurrent_emitters_deliver_in_sequence_order() {
        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let r = r.clone();
                std::thread::spawn(move || {
                    for i in 0..200 {
                        r.log("t", format!("{t}/{i}"));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let seqs: Vec<u64> = q.drain().iter().map(|e| e.seq).collect();
        assert_eq!(seqs.len(), 1600);
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "a sink saw events out of order");
    }

    #[test]
    fn scratch_files_are_told_from_saved_ones() {
        for scratch in [".app.rs.swp", ".!21378!app.rs", "app.rs~", "#app.rs#"] {
            assert!(is_scratch_file(scratch), "{scratch}");
        }
        for saved in ["app.rs", "Cargo.toml", "lib.rs"] {
            assert!(!is_scratch_file(saved), "{saved}");
        }
    }

    #[test]
    fn timestamps_never_go_backwards() {
        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        for _ in 0..50 {
            r.log("dev", "x");
        }
        let at: Vec<u64> = q.drain().iter().map(|e| e.at_ms).collect();
        assert!(at.windows(2).all(|w| w[1] >= w[0]));
    }
}
