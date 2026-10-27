//! `wait_for_build`: block until a dev session's build or save has
//! landed, then say how it ended.
//!
//! The wait reads the session's own event stream (`events_url` in its
//! registry entry, `/__idealyst/events`), never its log text. The stream
//! opens with a snapshot of the session's present (each target's latest
//! episode, with the original sequence numbers) and continues live, so a
//! build that already finished is answered from the snapshot at once.
//!
//! A wait can never outlive its deadline. The connect, every read and the
//! quiet windows are each bounded by it, a stream that closes (the session
//! exited) ends the wait, and a build still running at the deadline is
//! reported as `building` with its progress, for the caller to wait again.
//!
//! # Identifying a build
//!
//! Every event carries the session's gap-free sequence number. A build is
//! identified by the `seq` of the event that opened it (`id`) and the one
//! that ended it (`closed_seq`), and every answer carries a `cursor`, the
//! highest `seq` seen. `after: <cursor>` waits for an outcome that arrives
//! after that point. That covers a save made after the cursor, a build
//! that was already running at it, and a `--no-watch` queue that
//! `rebuild_dev` applies, and it answers at once when that outcome is
//! already in. `gen`, where an outcome has one, is the page generation a
//! rebuild reloaded to; patches do not move it, so it cannot identify them.
//!
//! # Waiting on a rebuild request
//!
//! `rebuild_dev` answers with the session's number for the request
//! (`request`). Each watcher that took it reports the work that applies
//! it (`rebuild_taken`), so `request: n` waits for exactly those builds,
//! one per target the request went to (`rebuild_requested`), and ignores
//! the ones after: a second agent's request made while the first's build
//! runs is applied by the next build, which the first agent does not wait
//! for. A watcher takes requests in order, so a target that has applied a
//! later request has applied this one.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use dev_events::{BuildOutcome, Decision, DevEvent, Envelope, SidecarUpdate};
use serde_json::{json, Value};

/// Default and longest wait. A caller whose build outlives it gets
/// `building` and waits again, which also keeps one call under an MCP
/// client's own request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// With nothing building and nothing after the cursor (or no cursor), how
/// long to wait for a save the caller just made to be noticed: the watcher's debounce is 50 ms, and
/// an editor's write-then-rename can take a few hundred more.
const SETTLE: Duration = Duration::from_millis(1500);

/// After the last outcome, how long nothing new may start before the wait
/// answers. A full-stack save builds the web bundle and the server on two
/// watchers, and the second can start just after the first ends.
const QUIET: Duration = Duration::from_millis(400);

/// How long one socket read may block before the deadline is checked.
const READ_SLICE: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Rustc errors kept per build.
const MAX_ERRORS: usize = 20;

/// Per target, the latest outcomes that applied rebuild requests, kept
/// past the target's later saves.
const MAX_ANSWERED: usize = 8;

#[derive(Debug, Clone, Default)]
struct Open {
    id: u64,
    /// A build or patch started: past the point where the save could still
    /// be waiting in a `--no-watch` queue.
    working: bool,
    superseded: u32,
    decision: Option<Decision>,
    patch_failed: Option<String>,
    stage: Option<String>,
    compiled: Option<u32>,
    total: Option<u32>,
    current: Option<String>,
    errors: Vec<Value>,
    /// Rebuild requests this work applies (`rebuild_taken`).
    requests: BTreeSet<u64>,
}

#[derive(Debug, Clone)]
struct Done {
    id: u64,
    closed_seq: u64,
    outcome: &'static str,
    failed: bool,
    error: Option<String>,
    gen: Option<u64>,
    ms: Option<u64>,
    decision: Option<Decision>,
    patch_failed: Option<String>,
    superseded: u32,
    errors: Vec<Value>,
    requests: BTreeSet<u64>,
}

#[derive(Debug, Default)]
struct Target {
    open: Option<Open>,
    last: Option<Done>,
    /// The latest [`MAX_ANSWERED`] outcomes that applied requests, oldest
    /// first.
    answered: Vec<Done>,
    /// Its watcher reports the requests it takes. A runtime-server host
    /// built before numbering never does; see [`Tracker::applied`].
    numbers_requests: bool,
    /// Its watcher announced `watching` and not yet `watcher_ready`: a save
    /// made now waits, unreported, until the watcher is ready.
    preparing: bool,
}

/// The episodes of every target, folded from the stream in order.
#[derive(Debug, Default)]
pub struct Tracker {
    targets: BTreeMap<String, Target>,
    cursor: u64,
    /// The seq of the latest event that opened or continued work (a save or
    /// a build start) — what the quiet window watches.
    last_start: u64,
    /// Each rebuild request seen: the seq announcing it, and the targets
    /// it went to.
    requests: BTreeMap<u64, (u64, Vec<String>)>,
}

impl Tracker {
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn feed(&mut self, e: &Envelope) {
        self.cursor = self.cursor.max(e.seq);
        let seq = e.seq;
        match &e.event {
            DevEvent::Watching { target, rewatch: false, .. } => {
                self.targets.entry(target.clone()).or_default().preparing = true;
            }
            DevEvent::WatcherReady { target } => {
                self.targets.entry(target.clone()).or_default().preparing = false;
            }
            DevEvent::RebuildRequested { request, targets } => {
                self.requests.insert(*request, (seq, targets.clone()));
            }
            DevEvent::RebuildTaken { target, request } => {
                self.last_start = seq;
                self.targets.entry(target.clone()).or_default().numbers_requests = true;
                self.open_or_new(target, seq).requests.insert(*request);
            }
            DevEvent::ChangeDetected { target, .. } => {
                self.last_start = seq;
                let t = self.targets.entry(target.clone()).or_default();
                // A save while one is open is the same work continuing: the
                // restart after a supersession, or a `--no-watch` queue
                // growing. It keeps the id the work started with.
                if t.open.is_none() {
                    t.open = Some(Open { id: seq, ..Open::default() });
                }
            }
            DevEvent::Superseded { target, .. } => {
                // Not an ending: the save goes on in the restart that follows.
                if let Some(o) = self.open_mut(target) {
                    o.superseded += 1;
                    o.working = false;
                }
            }
            DevEvent::Decided { target, decision } => {
                let o = self.open_or_new(target, seq);
                o.decision = Some(decision.clone());
                if matches!(decision, Decision::Unchanged) {
                    self.close(target, seq, "no_change", None, None, None);
                }
            }
            DevEvent::BuildStarted { target, .. } => {
                self.last_start = seq;
                self.open_or_new(target, seq).working = true;
            }
            DevEvent::StageStarted { target, stage } => {
                if let Some(o) = self.open_mut(target) {
                    o.stage = Some(stage.clone());
                }
            }
            DevEvent::CargoProgress { target, compiled, total, current } => {
                if let Some(o) = self.open_mut(target) {
                    o.compiled = Some(*compiled);
                    o.total = *total;
                    o.current = current.clone();
                }
            }
            DevEvent::Diagnostic { target, diagnostic } if diagnostic.is_error() => {
                if let Some(o) = self.open_mut(target) {
                    if o.errors.len() < MAX_ERRORS {
                        o.errors.push(json!({
                            "message": diagnostic.message,
                            "code": diagnostic.code,
                            "location": diagnostic.location(),
                            "rendered": diagnostic.rendered,
                        }));
                    }
                }
            }
            DevEvent::PatchFailed { target, reason, .. } => {
                // The save rebuilds next; the rebuild's outcome is the answer.
                self.open_or_new(target, seq).patch_failed = Some(reason.clone());
            }
            DevEvent::OverlayPushed { target, ms, .. } => {
                self.close(target, seq, "overlay_patch", None, None, Some(*ms));
            }
            DevEvent::PatchBuilt { target, ms, .. } => {
                self.close(target, seq, "hot_patch", None, None, Some(*ms));
            }
            DevEvent::BuildFinished { target, outcome, ms } => {
                let (name, gen, error) = match outcome {
                    BuildOutcome::Ready { gen } => ("ready", Some(*gen), None),
                    BuildOutcome::Reloaded { gen } => ("reloaded", Some(*gen), None),
                    BuildOutcome::Unchanged => ("unchanged", None, None),
                    BuildOutcome::PremintRefreshed { gen } => ("premint_refreshed", Some(*gen), None),
                    BuildOutcome::Failed { error } => ("failed", None, Some(error.clone())),
                };
                self.close(target, seq, name, gen, error, Some(*ms));
            }
            DevEvent::SidecarApplied { target, how, ms, .. } => {
                // Only as the end of an open episode: after a respawn's own
                // `build_finished` it is the restart, not a second outcome.
                if self.targets.get(target).is_some_and(|t| t.open.is_some()) {
                    let name = match how {
                        SidecarUpdate::HotPatch => "sidecar_hot_patch",
                        SidecarUpdate::Respawn => "sidecar_respawn",
                    };
                    self.close(target, seq, name, None, None, Some(*ms));
                }
            }
            _ => {}
        }
    }

    fn open_mut(&mut self, target: &str) -> Option<&mut Open> {
        self.targets.get_mut(target).and_then(|t| t.open.as_mut())
    }

    fn open_or_new(&mut self, target: &str, seq: u64) -> &mut Open {
        let t = self.targets.entry(target.to_string()).or_default();
        t.open.get_or_insert_with(|| Open { id: seq, ..Open::default() })
    }

    fn close(
        &mut self,
        target: &str,
        seq: u64,
        outcome: &'static str,
        gen: Option<u64>,
        error: Option<String>,
        ms: Option<u64>,
    ) {
        let t = self.targets.entry(target.to_string()).or_default();
        let o = t.open.take().unwrap_or(Open { id: seq, ..Open::default() });
        let done = Done {
            id: o.id,
            closed_seq: seq,
            outcome,
            failed: error.is_some(),
            error,
            gen,
            ms,
            decision: o.decision,
            patch_failed: o.patch_failed,
            superseded: o.superseded,
            errors: o.errors,
            requests: o.requests,
        };
        if !done.requests.is_empty() {
            t.answered.push(done.clone());
            if t.answered.len() > MAX_ANSWERED {
                t.answered.remove(0);
            }
        }
        t.last = Some(done);
    }

    /// The outcome on `target` that applied request `n`: the one that took
    /// it, or, its record gone, the earliest that took a later request
    /// (a watcher takes them in order). For a watcher that does not number
    /// requests (an older runtime-server host), its first outcome to open
    /// after the request was announced.
    fn applied(&self, target: &str, n: u64) -> Option<&Done> {
        let t = self.targets.get(target)?;
        if t.numbers_requests {
            return t
                .answered
                .iter()
                .filter(|d| d.requests.range(n..).next().is_some())
                .min_by_key(|d| d.requests.range(n..).next().copied());
        }
        let (announced, _) = self.requests.get(&n)?;
        t.last.as_ref().filter(|d| d.id > *announced)
    }

    /// The targets request `n` went to. Without its announcement (an
    /// older session's, or one past the snapshot's window), those that
    /// reported taking it or a later one.
    fn request_targets(&self, n: u64) -> Option<Vec<String>> {
        if let Some((_, targets)) = self.requests.get(&n) {
            return Some(targets.clone());
        }
        let took: Vec<String> = self
            .targets
            .iter()
            .filter(|(_, t)| {
                t.answered.iter().any(|d| d.requests.range(n..).next().is_some())
                    || t.open.as_ref().is_some_and(|o| o.requests.range(n..).next().is_some())
            })
            .map(|(name, _)| name.clone())
            .collect();
        (!took.is_empty()).then_some(took)
    }

    /// Open episodes that are building, as opposed to saves waiting in a
    /// `--no-watch` queue for `rebuild_dev`.
    fn progressing(&self, no_watch: bool) -> impl Iterator<Item = (&String, &Open)> {
        self.targets
            .iter()
            .filter_map(|(n, t)| t.open.as_ref().map(|o| (n, o)))
            .filter(move |(_, o)| !(no_watch && is_queued(o)))
    }

    fn queued(&self, no_watch: bool) -> Vec<&String> {
        if !no_watch {
            return Vec::new();
        }
        self.targets.iter().filter(|(_, t)| t.open.as_ref().is_some_and(is_queued)).map(|(n, _)| n).collect()
    }

    fn preparing(&self) -> Vec<&String> {
        self.targets.iter().filter(|(_, t)| t.preparing).map(|(n, _)| n).collect()
    }

    fn done_after(&self, after: u64) -> Vec<(&String, &Done)> {
        self.targets
            .iter()
            .filter_map(|(n, t)| t.last.as_ref().map(|d| (n, d)))
            .filter(|(_, d)| d.closed_seq > after)
            .collect()
    }

    fn latest(&self) -> Vec<(&String, &Done)> {
        self.targets.iter().filter_map(|(n, t)| t.last.as_ref().map(|d| (n, d))).collect()
    }
}

/// A `--no-watch` save that nothing has started building yet.
fn is_queued(o: &Open) -> bool {
    !o.working && o.decision.is_none() && o.superseded == 0 && o.requests.is_empty()
}

fn done_json(target: &str, d: &Done) -> Value {
    let mut v = json!({
        "target": target,
        "id": d.id,
        "closed_seq": d.closed_seq,
        "outcome": d.outcome,
        "failed": d.failed,
    });
    let o = v.as_object_mut().expect("an object");
    if let Some(tier) = d.decision.as_ref().map(Decision::tier) {
        o.insert("tier".into(), json!(tier));
    }
    if let Some(Decision::Rebuild { reason: Some(why) }) = &d.decision {
        o.insert("reason".into(), json!(why));
    }
    for (k, val) in [("gen", d.gen.map(Value::from)), ("ms", d.ms.map(Value::from))] {
        if let Some(val) = val {
            o.insert(k.into(), val);
        }
    }
    if let Some(e) = &d.error {
        o.insert("error".into(), json!(e));
    }
    if let Some(p) = &d.patch_failed {
        o.insert("hot_patch_failed".into(), json!(p));
    }
    if d.superseded > 0 {
        o.insert("superseded".into(), json!(d.superseded));
    }
    if !d.errors.is_empty() {
        o.insert("errors".into(), json!(d.errors));
    }
    if !d.requests.is_empty() {
        o.insert("requests".into(), json!(d.requests));
    }
    v
}

fn open_json(target: &str, o: &Open) -> Value {
    json!({
        "target": target,
        "id": o.id,
        "tier": o.decision.as_ref().map(Decision::tier),
        "stage": o.stage,
        "compiled": o.compiled,
        "total": o.total,
        "current": o.current,
        "errors_so_far": o.errors.len(),
    })
}

/// How far a waited-on rebuild request has got.
#[derive(Debug, PartialEq)]
enum RequestState {
    /// Every target it went to has an outcome for it.
    Applied,
    /// The session never announced it.
    Unknown,
    Pending,
}

/// What a wait decides, given the events fed so far.
#[derive(Debug, PartialEq)]
enum Step {
    /// Keep reading; answer at the deadline with what is known.
    Wait,
    /// Keep reading until `until`; answer then unless something starts.
    QuietUntil(Instant),
    Answer,
}

/// The wait's rules, separate from the socket so tests drive them.
pub struct Wait {
    pub tracker: Tracker,
    no_watch: bool,
    /// `None` until the snapshot has been read, when a missing `after`
    /// is resolved (see [`Wait::snapshot_done`]).
    after: Option<u64>,
    asked_after: Option<u64>,
    /// No cursor and nothing building at the snapshot: waiting
    /// [`SETTLE`] for a just-made save to be noticed.
    settling_until: Option<Instant>,
    quiet: Option<(u64, Instant)>,
    /// The session's `dev.log`, named in a failed answer: the full output
    /// behind the errors (`read_dev_log` reads it).
    log_file: Option<String>,
    /// The session's watchers send `watcher_ready`
    /// (`SessionEntry::reports_watcher_ready`). Only then does a watcher
    /// that has not sent it count as preparing: an older session's never
    /// will.
    honour_ready: bool,
    /// The snapshot already held the outcome asked for (see
    /// [`Wait::snapshot_done`]).
    already_in: bool,
    /// Waiting on this rebuild request instead of a cursor (see the
    /// module docs).
    request: Option<u64>,
    snapshot_read: bool,
}

impl Wait {
    pub fn new(after: Option<u64>, no_watch: bool) -> Self {
        Self { tracker: Tracker::default(), no_watch, after, asked_after: after, settling_until: None, quiet: None, log_file: None, honour_ready: false, already_in: false, request: None, snapshot_read: false }
    }

    /// Wait on rebuild request `n`.
    pub fn for_request(n: u64, no_watch: bool) -> Self {
        Self { request: Some(n), ..Self::new(None, no_watch) }
    }

    pub fn honouring_ready(mut self, on: bool) -> Self {
        self.honour_ready = on;
        self
    }

    fn preparing(&self) -> Vec<&String> {
        if self.honour_ready {
            self.tracker.preparing()
        } else {
            Vec::new()
        }
    }

    pub fn with_log(mut self, log_file: Option<String>) -> Self {
        self.log_file = log_file;
        self
    }

    /// The snapshot is in. Without a cursor: wait for whatever is building
    /// now, or, with nothing building, give a just-made save [`SETTLE`] to
    /// show up.
    pub fn snapshot_done(&mut self, now: Instant) {
        self.snapshot_read = true;
        if self.request.is_some() {
            return;
        }
        if let Some(after) = self.after {
            // The outcome asked for is in the snapshot and nothing builds:
            // it is the answer, with no window to wait out — the snapshot is
            // the session's present, so nothing is about to follow it.
            if !self.tracker.done_after(after).is_empty()
                && self.tracker.progressing(self.no_watch).next().is_none()
                && self.preparing().is_empty()
            {
                self.already_in = true;
                return;
            }
            // Nothing building and nothing landed after the cursor: give a
            // save [`SETTLE`] to show up rather than wait the whole timeout
            // for one that may never come (a save that changed no bytes, a
            // cursor taken after the save it meant to cover).
            if self.tracker.progressing(self.no_watch).next().is_none() && self.tracker.done_after(after).is_empty() {
                self.settling_until = Some(now + SETTLE);
            }
            return;
        }
        self.after = Some(self.tracker.cursor());
        if self.tracker.progressing(self.no_watch).next().is_none() {
            self.settling_until = Some(now + SETTLE);
        } else {
            // Something is building: its outcome is after the snapshot.
            self.after = Some(self.tracker.progressing(self.no_watch).map(|(_, o)| o.id).min().unwrap_or(1) - 1);
        }
    }

    fn step(&mut self, now: Instant) -> Step {
        if let Some(n) = self.request {
            return match self.request_state(n) {
                RequestState::Applied | RequestState::Unknown => Step::Answer,
                RequestState::Pending => Step::Wait,
            };
        }
        let after = self.after.unwrap_or(0);
        if self.already_in {
            return Step::Answer;
        }
        let building = self.tracker.progressing(self.no_watch).next().is_some();
        if let Some(until) = self.settling_until {
            // Work showed up: wait for it as for any other.
            if building || self.tracker.last_start > after || !self.tracker.done_after(after).is_empty() {
                self.settling_until = None;
            } else if !self.preparing().is_empty() {
                // A watcher still preparing holds any save made meanwhile
                // without a word: the settle starts once it is ready.
                self.settling_until = Some(now + SETTLE);
                return Step::Wait;
            } else if now >= until {
                return Step::Answer;
            } else {
                return Step::QuietUntil(until);
            }
        }
        if building {
            self.quiet = None;
            return Step::Wait;
        }
        if !self.tracker.queued(self.no_watch).is_empty() {
            return Step::Answer;
        }
        if self.tracker.done_after(after).is_empty() {
            return Step::Wait;
        }
        // An outcome is in and nothing builds: answer once nothing new has
        // started for a quiet window.
        let mark = self.tracker.last_start;
        match self.quiet {
            Some((m, until)) if m == mark => {
                if now >= until {
                    Step::Answer
                } else {
                    Step::QuietUntil(until)
                }
            }
            _ => {
                let until = now + QUIET;
                self.quiet = Some((mark, until));
                Step::QuietUntil(until)
            }
        }
    }

    fn request_state(&self, n: u64) -> RequestState {
        match self.tracker.request_targets(n) {
            // The control endpoint announces a request before it answers,
            // so a wait begun after `rebuild_dev` finds it in the snapshot.
            None if self.snapshot_read => RequestState::Unknown,
            None => RequestState::Pending,
            Some(targets) if targets.iter().all(|t| self.tracker.applied(t, n).is_some()) => RequestState::Applied,
            Some(_) => RequestState::Pending,
        }
    }

    fn answer_request(&self, n: u64, timed_out: bool, ended: Option<&str>) -> Value {
        let t = &self.tracker;
        let targets = t.request_targets(n).unwrap_or_default();
        let applied: Vec<(&String, &Done)> =
            targets.iter().filter_map(|name| t.applied(name, n).map(|d| (name, d))).collect();
        let builds: Vec<Value> = applied.iter().map(|(name, d)| done_json(name, d)).collect();
        let pending: Vec<&String> = targets.iter().filter(|name| t.applied(name, n).is_none()).collect();
        let building: Vec<Value> = pending
            .iter()
            .filter_map(|name| t.targets.get(*name).and_then(|x| x.open.as_ref()).map(|o| open_json(name, o)))
            .collect();
        let failed = applied.iter().any(|(_, d)| d.failed);
        let (status, next) = if let Some(why) = ended {
            ("session_ended", format!("the session's event stream closed ({why}); list_dev_sessions shows whether it is still running"))
        } else if targets.is_empty() {
            (
                "unknown_request",
                format!(
                    "this session has no rebuild request {n}: rebuild_dev returns the number to wait on (a session \
                     restarted since numbers its requests from 1 again). wait_for_build with after: {} waits for \
                     whatever builds next",
                    t.cursor()
                ),
            )
        } else if !pending.is_empty() {
            ("building", format!("not applied by the deadline; call wait_for_build again with request: {n}"))
        } else if failed {
            (
                "failed",
                format!(
                    "fix the errors and save; then wait_for_build with after: {}. The full output is in `log`: \
                     read_dev_log with filter \"error\" shows it",
                    t.cursor()
                ),
            )
        } else {
            ("finished", format!("request {n} is applied; after your next save, wait_for_build with after: {}", t.cursor()))
        };
        let mut out = json!({ "status": status, "request": n, "cursor": t.cursor(), "builds": builds, "next": next });
        let o = out.as_object_mut().expect("an object");
        if !building.is_empty() {
            o.insert("building".into(), json!(building));
        }
        if status == "building" {
            // Each target still owing the request, building it or not yet
            // started on it (busy with work before it).
            o.insert("waiting_for".into(), json!(pending));
        }
        if matches!(status, "failed" | "session_ended") {
            if let Some(log) = &self.log_file {
                o.insert("log".into(), json!(log));
            }
        }
        if timed_out && status != "building" {
            o.insert("timed_out".into(), json!(true));
        }
        out
    }

    /// The answer, as the tool returns it. `ended` is why the stream
    /// stopped early (the session exited), if it did.
    pub fn answer(&self, timed_out: bool, ended: Option<&str>) -> Value {
        if let Some(n) = self.request {
            return self.answer_request(n, timed_out, ended);
        }
        let after = self.after.unwrap_or(0);
        let t = &self.tracker;
        let building: Vec<Value> = t.progressing(self.no_watch).map(|(n, o)| open_json(n, o)).collect();
        let queued = t.queued(self.no_watch);
        let done = t.done_after(after);
        let builds: Vec<Value> = done.iter().map(|(n, d)| done_json(n, d)).collect();
        let failed = done.iter().any(|(_, d)| d.failed);
        let (status, next) = if let Some(why) = ended {
            ("session_ended", format!("the session's event stream closed ({why}); list_dev_sessions shows whether it is still running"))
        } else if !building.is_empty() {
            ("building", format!("still building at the deadline; call wait_for_build again with after: {after}"))
        } else if done.is_empty() && !self.preparing().is_empty() {
            (
                "preparing",
                format!(
                    "the session's watcher is still starting up (scanning the workspace); a save made now is \
                     applied once it is ready. Call wait_for_build again with after: {after}"
                ),
            )
        } else if !queued.is_empty() {
            ("queued", format!("saves are queued (--no-watch); call rebuild_dev, then wait_for_build with after: {}", t.cursor()))
        } else if !builds.is_empty() {
            if failed {
                (
                    "failed",
                    format!(
                        "fix the errors and save; then wait_for_build with after: {}. The full output is in \
                         `log`: read_dev_log with filter \"error\" shows it",
                        t.cursor()
                    ),
                )
            } else {
                ("finished", format!("after your next save, wait_for_build with after: {}", t.cursor()))
            }
        } else {
            ("idle", format!("nothing is building; after your next save, wait_for_build with after: {}", t.cursor()))
        };
        let mut out = json!({
            "status": status,
            "cursor": t.cursor(),
            "builds": builds,
            "next": next,
        });
        let o = out.as_object_mut().expect("an object");
        if !building.is_empty() {
            o.insert("building".into(), json!(building));
        }
        if !queued.is_empty() {
            o.insert("queued".into(), json!(queued));
        }
        let preparing = self.preparing();
        if !preparing.is_empty() {
            o.insert("preparing".into(), json!(preparing));
        }
        if status == "idle" {
            // What the session last did, so an idle answer still says
            // whether the last build was good.
            let last: Vec<Value> = t.latest().iter().map(|(n, d)| done_json(n, d)).collect();
            o.insert("last".into(), json!(last));
        }
        if matches!(status, "failed" | "session_ended") {
            if let Some(log) = &self.log_file {
                o.insert("log".into(), json!(log));
            }
        }
        if timed_out && status != "building" {
            o.insert("timed_out".into(), json!(true));
        }
        if let Some(a) = self.asked_after {
            o.insert("after".into(), json!(a));
        }
        out
    }
}

/// What a wait needs to know about its session, from the registry entry.
pub struct Session {
    pub events_url: String,
    pub log_file: Option<String>,
    pub no_watch: bool,
    pub reports_watcher_ready: bool,
}

/// What a wait waits for.
pub enum On {
    /// An outcome after this cursor, or, `None`, whatever builds now.
    After(Option<u64>),
    /// The builds that apply this rebuild request.
    Request(u64),
}

/// Wait on `session`. Returns the
/// answer, or an error when the stream cannot be opened.
pub fn wait(session: Session, on: On, timeout: Duration) -> Result<Value, String> {
    let deadline = Instant::now() + timeout.min(MAX_TIMEOUT);
    let mut reader = connect(&session.events_url, deadline)?;
    let w = match on {
        On::After(after) => Wait::new(after, session.no_watch),
        On::Request(n) => Wait::for_request(n, session.no_watch),
    };
    let mut w = w
        .with_log(session.log_file)
        .honouring_ready(session.reports_watcher_ready);
    let mut buf: Vec<u8> = Vec::new();
    let mut in_snapshot = true;
    // The snapshot is sent in one burst before any live event: it is over
    // when a read finds nothing waiting.
    loop {
        let now = Instant::now();
        if !in_snapshot {
            match w.step(now) {
                Step::Answer => return Ok(w.answer(false, None)),
                Step::Wait | Step::QuietUntil(_) if now >= deadline => return Ok(w.answer(true, None)),
                _ => {}
            }
        } else if now >= deadline {
            return Ok(w.answer(true, None));
        }
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => {
                if in_snapshot {
                    w.snapshot_done(Instant::now());
                }
                return Ok(w.answer(false, Some("the session exited")));
            }
            Ok(_) => {
                if buf.ends_with(b"\n") {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    buf.clear();
                    if let Some(json) = line.strip_prefix("data:") {
                        if let Ok(env) = serde_json::from_str::<Envelope>(json.trim()) {
                            w.tracker.feed(&env);
                        }
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if in_snapshot {
                    in_snapshot = false;
                    w.snapshot_done(Instant::now());
                }
            }
            Err(e) => return Ok(w.answer(false, Some(&e.to_string()))),
        }
    }
}

/// Open the stream and read past the response head. Every step is bounded
/// by `deadline`.
fn connect(url: &str, deadline: Instant) -> Result<BufReader<std::net::TcpStream>, String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("not an http events url: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = std::net::ToSocketAddrs::to_socket_addrs(authority)
        .map_err(|e| format!("cannot resolve {authority}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {authority}"))?;
    let budget = deadline.saturating_duration_since(Instant::now());
    let mut stream = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT.min(budget).max(Duration::from_millis(1)))
        .map_err(|e| format!("the session's event stream at {url} did not answer: {e}"))?;
    stream.set_read_timeout(Some(READ_SLICE)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\n\r\n")
        .map_err(|e| format!("cannot request the event stream: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let mut status_checked = false;
    loop {
        if Instant::now() >= deadline {
            return Err("the session's event stream sent no response before the deadline".into());
        }
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Err("the session's event stream closed before responding".into()),
            Ok(_) if line.ends_with(b"\n") => {
                let text = String::from_utf8_lossy(&line).trim_end().to_string();
                line.clear();
                if !status_checked {
                    if text.split_whitespace().nth(1) != Some("200") {
                        return Err(format!("the session's event stream answered `{text}`"));
                    }
                    status_checked = true;
                } else if text.is_empty() {
                    return Ok(reader);
                }
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("reading the event stream: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dev_events::{BuildCause, Diagnostic, SupersededWork, SCHEMA_VERSION};

    fn env(seq: u64, event: DevEvent) -> Envelope {
        Envelope { v: SCHEMA_VERSION, seq, at_ms: seq, event }
    }
    fn web() -> String {
        "web".into()
    }
    fn change(seq: u64) -> Envelope {
        env(seq, DevEvent::ChangeDetected { target: web(), paths: vec!["/p/src/app.rs".into()], crates: vec![], folded: 0 })
    }
    fn rebuild(seq: u64) -> Envelope {
        env(seq, DevEvent::Decided { target: web(), decision: Decision::Rebuild { reason: Some("a signature changed".into()) } })
    }
    fn started(seq: u64) -> Envelope {
        env(seq, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } })
    }
    fn reloaded(seq: u64, gen: u64) -> Envelope {
        env(seq, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Reloaded { gen }, ms: 2900 })
    }

    /// Drive `events` through a wait: `snapshot` first, then the live
    /// ones, stepping the clock past every quiet window. Returns the step
    /// after the last event and the answer.
    fn run(after: Option<u64>, no_watch: bool, snapshot: &[Envelope], live: &[Envelope]) -> (Step, Value) {
        let mut w = Wait::new(after, no_watch);
        for e in snapshot {
            w.tracker.feed(e);
        }
        let t0 = Instant::now();
        w.snapshot_done(t0);
        for e in live {
            w.tracker.feed(e);
            let _ = w.step(t0);
        }
        // Long after every window.
        let later = t0 + Duration::from_secs(60);
        let first = w.step(later);
        let step = if matches!(first, Step::QuietUntil(_)) { w.step(later + QUIET) } else { first };
        let answer = w.answer(false, None);
        (step, answer)
    }

    /// The build already finished before the wait began: the snapshot has
    /// its outcome after the cursor, and the wait answers without waiting
    /// for anything new.
    #[test]
    fn a_build_that_already_finished_is_answered_from_the_snapshot() {
        let snapshot = [change(10), rebuild(11), started(12), reloaded(20, 4)];
        let mut w = Wait::new(Some(9), false);
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert_eq!(w.step(t0), Step::Answer, "at once, with no quiet window");
        let (step, a) = run(Some(9), false, &snapshot, &[]);
        assert_eq!(step, Step::Answer);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["id"], 10);
        assert_eq!(a["builds"][0]["closed_seq"], 20);
        assert_eq!(a["builds"][0]["gen"], 4);
        assert_eq!(a["builds"][0]["tier"], "rebuild");
        assert_eq!(a["cursor"], 20);
    }

    /// A cursor taken after that outcome waits for the next one, and
    /// answers when it lands.
    #[test]
    fn a_cursor_waits_for_the_build_after_it() {
        let snapshot = [change(10), rebuild(11), started(12), reloaded(20, 4)];
        let mut w = Wait::new(Some(20), false);
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert!(matches!(w.step(t0), Step::QuietUntil(_)), "settling: nothing after the cursor yet");
        for e in [change(30), env(31, DevEvent::Decided { target: web(), decision: Decision::HotPatch { crates: vec!["app".into()], files: vec![] } })] {
            w.tracker.feed(&e);
        }
        assert_eq!(w.step(t0), Step::Wait, "the patch is building");
        w.tracker.feed(&env(
            40,
            DevEvent::PatchBuilt { target: web(), files: vec![], crates: vec![], redirected: 3, steps: vec![], skipped: vec![], bytes: 1, ms: 600 },
        ));
        assert!(matches!(w.step(t0), Step::QuietUntil(_)));
        assert_eq!(w.step(t0 + QUIET), Step::Answer);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["outcome"], "hot_patch");
        assert_eq!(a["builds"][0]["id"], 30);
        assert!(a["builds"][0].get("gen").is_none(), "a patch moves no generation");
    }

    /// Regression: a cursor with nothing after it (a save that changed no
    /// bytes, or a cursor taken after the save it meant to cover) waited
    /// the whole timeout — 120 s in the live run — for a build that never
    /// came. It answers `idle` once the settle passes with nothing started.
    #[test]
    fn regression_a_cursor_with_nothing_after_it_answers_idle_after_the_settle() {
        let snapshot = [change(10), rebuild(11), started(12), reloaded(20, 4), env(25, DevEvent::PageAck { target: web(), ack: dev_events::PageAck::Connected { gen: 4 } })];
        let mut w = Wait::new(Some(25), false);
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert!(matches!(w.step(t0), Step::QuietUntil(_)));
        assert_eq!(w.step(t0 + SETTLE), Step::Answer, "pre-fix: Step::Wait until the deadline");
        let a = w.answer(false, None);
        assert_eq!(a["status"], "idle");
        assert_eq!(a["last"][0]["closed_seq"], 20);
    }

    /// Regression (live, welcome): a save made while the watcher scanned
    /// the workspace (9 s after its first build) went unreported, and the
    /// wait's 1.5 s settle answered `idle` for a save that was still to
    /// come. A watcher that has not said `watcher_ready` holds the settle
    /// until it does, and the held save is then waited for.
    #[test]
    fn regression_a_save_held_by_a_preparing_watcher_is_waited_for() {
        let watching = env(30, DevEvent::Watching { target: web(), roots: vec![], rewatch: false });
        let snapshot = [change(10), rebuild(11), started(12), reloaded(20, 1), watching];
        let mut w = Wait::new(Some(30), false).honouring_ready(true);
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert_eq!(w.step(t0 + SETTLE * 4), Step::Wait, "pre-fix: Answer (idle) after the settle");
        assert_eq!(w.answer(true, None)["status"], "preparing");
        let t1 = t0 + SETTLE * 6;
        w.tracker.feed(&env(31, DevEvent::WatcherReady { target: web() }));
        w.tracker.feed(&change(32));
        w.tracker.feed(&env(33, DevEvent::Decided { target: web(), decision: Decision::Unchanged }));
        assert!(matches!(w.step(t1), Step::QuietUntil(_)));
        assert_eq!(w.step(t1 + QUIET), Step::Answer);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["id"], 32);
    }

    /// A session from before `watcher_ready` never sends it: its watcher is
    /// not treated as preparing, or every wait would run to the timeout.
    #[test]
    fn an_older_session_without_watcher_ready_settles_as_before() {
        let watching = env(30, DevEvent::Watching { target: web(), roots: vec![], rewatch: false });
        let mut w = Wait::new(Some(30), false).honouring_ready(false);
        w.tracker.feed(&watching);
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert_eq!(w.step(t0 + SETTLE), Step::Answer);
        assert_eq!(w.answer(false, None)["status"], "idle");
    }

    /// A superseded save is not an ending: the wait follows the restart and
    /// answers with its outcome, under the id the work started with.
    #[test]
    fn a_superseded_save_is_followed_to_its_restart() {
        let live = [
            change(30),
            rebuild(31),
            started(32),
            env(33, DevEvent::Superseded { target: web(), work: SupersededWork::Rebuild, ms: 900 }),
            change(34),
            rebuild(35),
            started(36),
            reloaded(50, 5),
        ];
        let mut w = Wait::new(Some(20), false);
        let t0 = Instant::now();
        w.snapshot_done(t0);
        for e in &live[..4] {
            w.tracker.feed(e);
        }
        assert_eq!(w.step(t0), Step::Wait, "superseded is not done");
        let (_, a) = run(Some(20), false, &[], &live);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["id"], 30);
        assert_eq!(a["builds"][0]["superseded"], 1);
        assert_eq!(a["builds"][0]["gen"], 5);
    }

    /// A failed build answers `failed` with rustc's errors, located.
    #[test]
    fn a_failed_build_reports_its_errors() {
        let diag = Diagnostic {
            level: "error".into(),
            message: "mismatched types".into(),
            code: Some("E0308".into()),
            file: Some("src/app.rs".into()),
            line: Some(12),
            column: Some(9),
            package: None,
            rendered: "error[E0308]: mismatched types".into(),
            ansi: None,
        };
        let live = [
            change(30),
            rebuild(31),
            started(32),
            env(33, DevEvent::Diagnostic { target: web(), diagnostic: diag }),
            env(34, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Failed { error: "cargo exited with 101".into() }, ms: 4000 }),
        ];
        let mut w = Wait::new(Some(20), false).with_log(Some("/p/target/idealyst/app/dev.log".into()));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        live.iter().for_each(|e| w.tracker.feed(e));
        assert!(matches!(w.step(t0), Step::QuietUntil(_)));
        assert_eq!(w.step(t0 + QUIET), Step::Answer);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "failed");
        assert_eq!(a["log"], "/p/target/idealyst/app/dev.log", "the whole log is referenced");
        assert!(a["next"].as_str().unwrap().contains("read_dev_log"));
        assert_eq!(a["builds"][0]["errors"][0]["location"], "src/app.rs:12:9");
        assert_eq!(a["builds"][0]["error"], "cargo exited with 101");
    }

    /// With no cursor and nothing building, a wait answers `idle` after a
    /// short settle — with the last outcome — instead of waiting for
    /// anything to happen.
    #[test]
    fn no_cursor_and_nothing_building_answers_idle_with_the_last_build() {
        let snapshot = [change(10), rebuild(11), started(12), reloaded(20, 4)];
        let mut w = Wait::new(None, false);
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        assert!(matches!(w.step(t0), Step::QuietUntil(_)), "settling for a just-made save");
        assert_eq!(w.step(t0 + SETTLE), Step::Answer);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "idle");
        assert_eq!(a["last"][0]["gen"], 4);
        assert_eq!(a["cursor"], 20);
    }

    /// A save made just before the call, noticed during the settle, is
    /// waited for.
    #[test]
    fn no_cursor_catches_a_save_noticed_while_settling() {
        let mut w = Wait::new(None, false);
        let t0 = Instant::now();
        w.snapshot_done(t0);
        w.tracker.feed(&change(5));
        w.tracker.feed(&rebuild(6));
        assert_eq!(w.step(t0), Step::Wait);
        w.tracker.feed(&started(7));
        w.tracker.feed(&reloaded(9, 2));
        assert!(matches!(w.step(t0), Step::QuietUntil(_)));
        assert_eq!(w.step(t0 + QUIET), Step::Answer);
        assert_eq!(w.answer(false, None)["builds"][0]["id"], 5);
    }

    /// No cursor while a build runs: the wait is for that build.
    #[test]
    fn no_cursor_waits_for_the_build_in_flight() {
        let snapshot = [change(10), rebuild(11), started(12)];
        let (step, a) = run(None, false, &snapshot, &[reloaded(20, 3)]);
        assert_eq!(step, Step::Answer);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["id"], 10);
    }

    fn requested(seq: u64, n: u64, targets: &[&str]) -> Envelope {
        env(seq, DevEvent::RebuildRequested { request: n, targets: targets.iter().map(|t| t.to_string()).collect() })
    }
    fn taken(seq: u64, target: &str, n: u64) -> Envelope {
        env(seq, DevEvent::RebuildTaken { target: target.into(), request: n })
    }
    fn forced(seq: u64, target: &str) -> Envelope {
        env(seq, DevEvent::BuildStarted { target: target.into(), cause: BuildCause::Forced })
    }
    fn finished(seq: u64, target: &str, outcome: BuildOutcome) -> Envelope {
        env(seq, DevEvent::BuildFinished { target: target.into(), outcome, ms: 1200 })
    }

    /// Feed `snapshot`, then each live event, returning the step after
    /// each live event.
    fn drive(w: &mut Wait, snapshot: &[Envelope], live: &[Envelope]) -> Vec<Step> {
        snapshot.iter().for_each(|e| w.tracker.feed(e));
        let t0 = Instant::now();
        w.snapshot_done(t0);
        live.iter()
            .map(|e| {
                w.tracker.feed(e);
                w.step(t0)
            })
            .collect()
    }

    /// The case two agents hit: the first requests a rebuild, the second
    /// requests another while the first's build runs. The first agent's
    /// wait ends when ITS build does, not after the second build.
    #[test]
    fn regression_a_requests_wait_ends_with_its_build_not_the_next_agents() {
        let mut w = Wait::for_request(1, false);
        let steps = drive(
            &mut w,
            &[requested(10, 1, &["web"]), taken(11, "web", 1), forced(12, "web")],
            &[
                requested(13, 2, &["web"]),
                finished(20, "web", BuildOutcome::Reloaded { gen: 3 }),
                taken(21, "web", 2),
                forced(22, "web"),
            ],
        );
        assert_eq!(steps[..2], [Step::Wait, Step::Answer], "answered as request 1's build finished");
        let a = w.answer(false, None);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["request"], 1);
        assert_eq!(a["builds"][0]["requests"], json!([1]));
        assert_eq!(a["builds"][0]["gen"], 3);
        // And the second agent's wait is not answered by the first build.
        let mut w = Wait::for_request(2, false);
        let steps = drive(
            &mut w,
            &[requested(10, 1, &["web"]), taken(11, "web", 1), forced(12, "web"), requested(13, 2, &["web"])],
            &[finished(20, "web", BuildOutcome::Reloaded { gen: 3 }), taken(21, "web", 2), forced(22, "web"), finished(30, "web", BuildOutcome::Reloaded { gen: 4 })],
        );
        assert_eq!(steps, [Step::Wait, Step::Wait, Step::Wait, Step::Answer]);
        assert_eq!(w.answer(false, None)["builds"][0]["gen"], 4);
    }

    /// A save that supersedes the request's build restarts it with the
    /// request still in it: the restart's outcome answers, not the
    /// supersession.
    #[test]
    fn a_superseded_requests_restart_answers_it() {
        let mut w = Wait::for_request(1, false);
        let superseded = env(13, DevEvent::Superseded { target: web(), work: SupersededWork::Rebuild, ms: 4 });
        let steps = drive(
            &mut w,
            &[requested(10, 1, &["web"])],
            &[taken(11, "web", 1), forced(12, "web"), superseded, taken(14, "web", 1), forced(15, "web"), finished(20, "web", BuildOutcome::Reloaded { gen: 2 })],
        );
        assert_eq!(steps.last(), Some(&Step::Answer));
        assert!(steps[..steps.len() - 1].iter().all(|s| *s == Step::Wait), "{steps:?}");
        assert_eq!(w.answer(false, None)["builds"][0]["superseded"], 1);
    }

    /// A request sent to two watchers is answered when both have applied
    /// it; one failing makes the answer `failed`, with the log.
    #[test]
    fn a_request_to_two_targets_waits_for_both() {
        let mut w = Wait::for_request(5, false).with_log(Some("/p/dev.log".into()));
        let steps = drive(
            &mut w,
            &[requested(10, 5, &["web", "server"]), taken(11, "web", 5), forced(12, "web")],
            &[
                finished(13, "web", BuildOutcome::Reloaded { gen: 2 }),
                taken(14, "server", 5),
                forced(15, "server"),
                finished(16, "server", BuildOutcome::Failed { error: "cargo exited with 101".into() }),
            ],
        );
        assert_eq!(steps, [Step::Wait, Step::Wait, Step::Wait, Step::Answer]);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "failed");
        assert_eq!(a["builds"].as_array().unwrap().len(), 2);
        assert_eq!(a["log"], "/p/dev.log");
    }

    /// A wait begun after the request was applied, and after later saves
    /// replaced it as the latest build, still answers from the snapshot
    /// (which keeps the last episode that applied a request) at once.
    #[test]
    fn a_request_applied_before_the_wait_is_answered_at_once() {
        let mut w = Wait::for_request(1, false);
        let snapshot = [
            requested(10, 1, &["web"]),
            taken(11, "web", 1),
            forced(12, "web"),
            finished(13, "web", BuildOutcome::Reloaded { gen: 2 }),
            change(20),
            env(21, DevEvent::OverlayPushed { target: web(), sites: 1, ms: 30 }),
        ];
        drive(&mut w, &snapshot, &[]);
        assert_eq!(w.step(Instant::now()), Step::Answer);
        let a = w.answer(false, None);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["closed_seq"], 13, "the build that applied the request, not the later patch");
    }

    /// A request the session never made is said so at once, not waited
    /// out.
    #[test]
    fn an_unknown_request_is_answered_at_once() {
        let mut w = Wait::for_request(9, false);
        drive(&mut w, &[requested(10, 1, &["web"]), taken(11, "web", 1)], &[]);
        assert_eq!(w.step(Instant::now()), Step::Answer);
        assert_eq!(w.answer(false, None)["status"], "unknown_request");
    }

    /// A runtime-server host built before numbering takes the request
    /// without saying so: its first outcome to open after the request was
    /// announced answers it.
    #[test]
    fn a_host_that_does_not_number_requests_is_answered_by_its_next_build() {
        let rs = "runtime-server";
        let mut w = Wait::for_request(1, false);
        let steps = drive(
            &mut w,
            &[env(5, DevEvent::BuildStarted { target: rs.into(), cause: BuildCause::Initial }), requested(10, 1, &[rs])],
            &[
                finished(11, rs, BuildOutcome::Ready { gen: 1 }),
                env(12, DevEvent::BuildStarted { target: rs.into(), cause: BuildCause::Forced }),
                finished(13, rs, BuildOutcome::Reloaded { gen: 2 }),
            ],
        );
        assert_eq!(steps, [Step::Wait, Step::Wait, Step::Answer], "not the build already running at the request");
    }

    /// At the deadline an unapplied request is `building`, naming what it
    /// waits for, and the next call names the request again.
    #[test]
    fn an_unapplied_request_at_the_deadline_says_what_it_waits_for() {
        let mut w = Wait::for_request(1, false);
        drive(&mut w, &[requested(10, 1, &["web", "server"]), taken(11, "web", 1), forced(12, "web")], &[]);
        let a = w.answer(true, None);
        assert_eq!(a["status"], "building");
        assert_eq!(a["waiting_for"], json!(["web", "server"]));
        assert_eq!(a["building"][0]["target"], "web");
        assert!(a["next"].as_str().unwrap().contains("request: 1"));
    }

    /// Under `--no-watch` a save waits in a queue until `rebuild_dev`, so
    /// waiting on it would wait for nothing: answer `queued` at once.
    #[test]
    fn a_no_watch_queue_answers_queued_instead_of_waiting() {
        let (step, a) = run(Some(0), true, &[change(10)], &[]);
        assert_eq!(step, Step::Answer);
        assert_eq!(a["status"], "queued");
        assert!(a["next"].as_str().unwrap().contains("rebuild_dev"));
        // The rebuild then continues the queued episode, and a wait with the
        // cursor from before it answers.
        let (_, a) = run(Some(10), true, &[change(10)], &[rebuild(11), started(12), reloaded(20, 2)]);
        assert_eq!(a["status"], "finished");
        assert_eq!(a["builds"][0]["id"], 10);
    }

    /// A full-stack save ends on one watcher and starts on another just
    /// after: the quiet window waits for the second.
    #[test]
    fn a_second_target_starting_in_the_quiet_window_is_waited_for() {
        let mut w = Wait::new(Some(0), false);
        let t0 = Instant::now();
        w.snapshot_done(t0);
        for e in [change(1), rebuild(2), started(3), reloaded(4, 2)] {
            w.tracker.feed(&e);
        }
        assert!(matches!(w.step(t0), Step::QuietUntil(_)));
        w.tracker.feed(&env(5, DevEvent::ChangeDetected { target: "server".into(), paths: vec![], crates: vec![], folded: 0 }));
        w.tracker.feed(&env(6, DevEvent::BuildStarted { target: "server".into(), cause: BuildCause::Save { folded: 0 } }));
        assert_eq!(w.step(t0 + QUIET), Step::Wait, "the server is building");
        w.tracker.feed(&env(7, DevEvent::BuildFinished { target: "server".into(), outcome: BuildOutcome::Unchanged, ms: 900 }));
        assert!(matches!(w.step(t0 + QUIET), Step::QuietUntil(_)));
        assert_eq!(w.step(t0 + QUIET * 3), Step::Answer);
        assert_eq!(w.answer(false, None)["builds"].as_array().unwrap().len(), 2);
    }

    /// The socket half: a stream that stays open and silent (keepalives
    /// only) still answers at the deadline, and one that closes answers
    /// `session_ended` — neither hangs.
    #[test]
    fn a_silent_stream_answers_at_the_deadline_and_a_closed_one_at_once() {
        use std::io::Read;
        let serve = |close_after: bool| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/__idealyst/events", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut req = [0u8; 512];
                let _ = s.read(&mut req);
                let building = [change(10), rebuild(11), started(12)];
                let mut body = String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nretry: 1000\n\n");
                for e in building {
                    body.push_str(&format!("data: {}\n\n", serde_json::to_string(&e).unwrap()));
                }
                s.write_all(body.as_bytes()).unwrap();
                if close_after {
                    return;
                }
                for _ in 0..40 {
                    std::thread::sleep(Duration::from_millis(100));
                    if s.write_all(b": ping\n\n").is_err() {
                        return;
                    }
                }
            });
            url
        };
        let sess = |events_url: String| Session { events_url, log_file: None, no_watch: false, reports_watcher_ready: true };
        let started_at = Instant::now();
        let a = wait(sess(serve(false)), On::After(None), Duration::from_millis(1200)).unwrap();
        let took = started_at.elapsed();
        assert_eq!(a["status"], "building");
        assert_eq!(a["building"][0]["id"], 10);
        assert!(took < Duration::from_millis(2500), "answered at the deadline, took {took:?}");

        let a = wait(sess(serve(true)), On::After(None), Duration::from_secs(30)).unwrap();
        assert_eq!(a["status"], "session_ended");

        let err = wait(sess("http://127.0.0.1:1/__idealyst/events".into()), On::After(None), Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("did not answer"), "{err}");
    }
}
