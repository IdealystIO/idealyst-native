//! The panel's state, as a pure fold over the session's events.
//!
//! Nothing here touches the framework: [`Model::apply`] takes one
//! [`Envelope`] and updates plain data, so the state machine is tested as
//! data (`tests` below) and the view (`crate::view`) is a function of it.
//!
//! Each target is a row with a state machine:
//!
//! ```text
//! starting ─► building (initial) ─► ready
//!                                       │ change detected
//!                                       ▼
//!   watching ◄── unchanged ◄── changed (deciding)
//!                                 │ decided
//!             ┌───────────────────┼──────────────────────┐
//!             ▼                   ▼                      ▼
//!     applying overlay     building patch ──failed──► rebuilding
//!             │                   │                      │
//!             ▼                   ▼                      ▼
//!          applied             patched            reloaded │ error
//! ```
//!
//! A full-stack session's server (declared by `session_started.server`)
//! is a row from the first frame, with its own machine:
//!
//! ```text
//! queued ─► building (initial) ─► starting ─► running on <url>
//!                                                 │ its own sources saved
//!                                                 ▼
//!        running ◄── starting ◄── rebuilding ◄── changed
//!        (restarted)   (restarting)    │
//!                                      ├─ unchanged ─► running (untouched)
//!                                      └─ failed ─► error (old server still up)
//! ```
//!
//! A save only the web bundle sees leaves it as it was.
//!
//! Every save is also a line in the history ring: the files, the tier it
//! took, how long it took, what it did, and whether the page acked it.

use std::collections::VecDeque;

use dev_events::{
    BuildCause, BuildOutcome, Decision, DevEvent, Diagnostic, Envelope, HotTier, PageAck,
    ServerKind, SidecarUpdate,
};

/// Saves kept in the history ring.
pub const HISTORY_CAP: usize = 20;
/// Lines kept for the log pane.
pub const LOG_CAP: usize = 2_000;

/// What a target is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing reported yet.
    Starting,
    /// Idle, watching for saves.
    Watching,
    /// The first build finished.
    Ready { ms: u64 },
    /// Files changed; the decision is coming.
    Changed { files: Vec<String> },
    /// Overlay patches are on their way to the page.
    ApplyingOverlay { sites: usize },
    /// A hot patch is compiling.
    BuildingPatch { crates: Vec<String> },
    /// A build is running.
    Building {
        /// `building` for the first build, `rebuilding` after.
        label: &'static str,
        stage: Option<String>,
        compiled: u32,
        total: Option<u32>,
        current: Option<String>,
    },
    /// An overlay patch was pushed.
    Applied { sites: usize, ms: u64 },
    /// A hot patch was pushed (or, in runtime-server mode, applied).
    Patched { redirected: Option<usize>, ms: u64 },
    /// A rebuild finished; the page reloads.
    Reloaded { ms: u64 },
    /// A rebuild produced the same bundle.
    Unchanged { ms: u64 },
    /// A build failed.
    Failed { summary: String },
    /// A target with no typed events yet (a native launcher): its latest
    /// `[dev <target>]` line.
    Note { line: String },
    /// A declared server whose first build has not started — waiting its
    /// turn behind the web bundle, or about to start beside it.
    Queued,
    /// A server that built and is being (re)started: `starting` or
    /// `restarting`, until it accepts connections. `note` is what its
    /// build did, carried into [`State::Running`].
    Launching { label: &'static str, note: String },
    /// A server accepting connections at `url`. `note` says what its last
    /// build did; `fresh` when that was a save's rebuild (a restart, or a
    /// rebuild that changed nothing) rather than the session's start.
    Running { url: String, note: String, fresh: bool },
}

impl State {
    /// Whether the row is working (drives the spinner and elapsed time).
    pub fn busy(&self) -> bool {
        matches!(
            self,
            State::Changed { .. }
                | State::ApplyingOverlay { .. }
                | State::BuildingPatch { .. }
                | State::Building { .. }
                | State::Launching { .. }
        )
    }
}

/// One target's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The `target` its events carry.
    pub name: String,
    /// What the row is called: the name, or a declared server's own
    /// (`crewforge-server` for events filed under `server`).
    pub title: String,
    pub state: State,
    /// Session time the current state began (elapsed time is measured
    /// from here).
    pub since_ms: u64,
    /// Error diagnostics of the build in flight.
    pending: Vec<Diagnostic>,
}

/// One save, as the history shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    pub at_ms: u64,
    pub target: String,
    /// File names (not paths) the save touched.
    pub files: Vec<String>,
    /// `overlay`, `hot patch`, `rebuild`, `no change`, … — or `…` while
    /// undecided.
    pub tier: String,
    /// Save to applied, once known.
    pub ms: Option<u64>,
    /// What it did: `3 fn redirected`, `1 site`, `patch failed`, …
    pub detail: String,
    /// The page (or the sidecar) reported applying it.
    pub acked: bool,
    pub failed: bool,
}

/// The build error the panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInfo {
    pub target: String,
    /// `file:line:column` of the first error, when rustc said.
    pub location: Option<String>,
    /// Its one-line message.
    pub message: String,
    /// Its rendered text (rustc's own), or the build's error.
    pub rendered: String,
    /// Error diagnostics in the failed build.
    pub count: usize,
}

/// One line of the log pane, and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub text: String,
    /// The full-stack server process's own output (`output{source:
    /// server}`), as opposed to the dev loop's lines. The pane shows the
    /// loop's by default; the server's only when asked (`l`).
    pub server: bool,
}

impl LogLine {
    pub fn dev(text: impl Into<String>) -> Self {
        Self { text: text.into(), server: false }
    }
}

/// Server lines the error pane quotes when the server crashed.
const CRASH_TAIL: usize = 20;

/// See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    pub app: String,
    pub mode: String,
    /// Where the app is served.
    pub urls: Vec<String>,
    /// `/__idealyst/events`, for anything else that wants the stream.
    pub events_url: Option<String>,
    pub hot_tier: Option<HotTier>,
    pub log_file: Option<String>,
    pub targets: Vec<Target>,
    /// Newest first.
    pub history: VecDeque<Save>,
    /// Oldest first.
    pub log: VecDeque<LogLine>,
    pub error: Option<ErrorInfo>,
    /// The latest session time seen.
    pub now_ms: u64,
    /// The declared full-stack server's target, and where it last
    /// answered.
    pub server: Option<String>,
    server_url: Option<String>,
    /// Where the server process's output is written (`server.log`).
    pub server_log: Option<String>,
}

impl Model {
    pub fn new(targets: &[String]) -> Self {
        let mut m = Self::default();
        for t in targets {
            m.target(t);
        }
        m
    }

    fn target(&mut self, name: &str) -> &mut Target {
        if let Some(i) = self.targets.iter().position(|t| t.name == name) {
            return &mut self.targets[i];
        }
        self.targets.push(Target {
            name: name.to_string(),
            title: name.to_string(),
            state: State::Starting,
            since_ms: self.now_ms,
            pending: Vec::new(),
        });
        self.targets.last_mut().expect("just pushed")
    }

    fn set(&mut self, target: &str, state: State) {
        let now = self.now_ms;
        let t = self.target(target);
        t.state = state;
        t.since_ms = now;
    }

    /// The newest save of `target`, if it is still in flight or just done.
    fn save(&mut self, target: &str) -> Option<&mut Save> {
        self.history.iter_mut().find(|s| s.target == target)
    }

    fn push_save(&mut self, save: Save) {
        self.history.push_front(save);
        self.history.truncate(HISTORY_CAP);
    }

    /// Record the end of `target`'s newest save.
    fn finish_save(&mut self, target: &str, tier: &str, detail: String, failed: bool) {
        let now = self.now_ms;
        if let Some(s) = self.save(target) {
            if s.ms.is_none() {
                s.tier = tier.to_string();
                s.ms = Some(now.saturating_sub(s.at_ms));
                s.detail = detail;
                s.failed = failed;
            }
        }
    }

    fn log_line(&mut self, line: String) {
        self.push_log(line, false);
    }

    fn push_log(&mut self, line: String, server: bool) {
        for l in dev_events::plain::strip_ansi(&line).lines() {
            self.log.push_back(LogLine { text: l.to_string(), server });
        }
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    /// `c`: forget the history, the log and the error. Rows keep their
    /// state — they describe the present, not the past.
    pub fn clear(&mut self) {
        self.history.clear();
        self.log.clear();
        self.error = None;
    }

    /// Fold one event in.
    pub fn apply(&mut self, e: &Envelope) {
        self.now_ms = self.now_ms.max(e.at_ms);
        // The log pane is the raw stream: every line the plain terminal
        // would have shown, each marked with whose it is.
        if let Some(line) = dev_events::plain::render(&e.event) {
            let server = matches!(&e.event, DevEvent::Output { source, .. }
                if source == dev_events::SERVER_OUTPUT_SOURCE);
            self.push_log(line, server);
        }
        match &e.event {
            DevEvent::SessionStarted { app, targets, mode, hot_tier, log_file, server } => {
                self.app = app.clone();
                self.mode = mode.as_str().to_string();
                self.hot_tier = Some(hot_tier.clone());
                self.log_file = log_file.clone();
                for t in targets {
                    self.target(t);
                }
                // The server's row exists before its build says anything:
                // a session that is compiling a server shows one.
                if let Some(server) = server {
                    let t = self.target(&server.target);
                    t.title = server.name.clone();
                    if t.state == State::Starting {
                        t.state = State::Queued;
                    }
                    self.server = Some(server.target.clone());
                    self.server_log = server.log_file.clone();
                }
            }
            DevEvent::ServerReady { kind, url, .. } => match kind {
                ServerKind::Events => self.events_url = Some(url.clone()),
                // The stream beside a full-stack server is plumbing, not
                // an address anyone opens.
                ServerKind::ReloadStream => {}
                _ => {
                    if !self.urls.contains(url) {
                        self.urls.push(url.clone());
                    }
                    if *kind == ServerKind::FullStack {
                        self.server_url = Some(url.clone());
                        if let Some(server) = self.server.clone() {
                            let (note, fresh) = match &self.target(&server).state {
                                State::Launching { label, note } => {
                                    (note.clone(), *label == "restarting")
                                }
                                // Started with no build of ours (`--no-build`).
                                _ => (String::new(), false),
                            };
                            self.set(&server, State::Running { url: url.clone(), note, fresh });
                            // A server that answers again has left its
                            // crash (or failed build) behind.
                            self.clear_error(&server);
                        }
                    }
                }
            },
            DevEvent::Watching { target, .. } => {
                if self.target(target).state == State::Starting {
                    self.set(target, State::Watching);
                }
            }
            DevEvent::ChangeDetected { target, paths, .. } => {
                let files: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
                self.set(target, State::Changed { files: files.clone() });
                let at_ms = self.now_ms;
                self.push_save(Save {
                    at_ms,
                    target: target.clone(),
                    files,
                    tier: "…".into(),
                    ms: None,
                    detail: String::new(),
                    acked: false,
                    failed: false,
                });
            }
            DevEvent::Decided { target, decision } => match decision {
                Decision::Overlay { sites } => {
                    self.set(target, State::ApplyingOverlay { sites: *sites })
                }
                Decision::HotPatch { crates, .. } => {
                    self.set(target, State::BuildingPatch { crates: crates.clone() })
                }
                Decision::Rebuild { reason } => {
                    if let Some(s) = self.save(target) {
                        if s.ms.is_none() {
                            s.tier = "rebuild".into();
                            s.detail = reason.clone().unwrap_or_default();
                        }
                    }
                }
                Decision::Unchanged => {
                    self.set(target, State::Watching);
                    self.finish_save(target, "no change", String::new(), false);
                    // The source is back to what runs: an old build error
                    // no longer describes it.
                    self.clear_error(target);
                }
            },
            DevEvent::OverlayPushed { target, sites, ms } => {
                self.set(target, State::Applied { sites: *sites, ms: *ms });
                self.finish_save(target, "overlay", plural(*sites, "site"), false);
            }
            DevEvent::PatchBuilt { target, redirected, ms, .. } => {
                self.set(target, State::Patched { redirected: Some(*redirected), ms: *ms });
                self.finish_save(target, "hot patch", format!("{redirected} fn redirected"), false);
                self.clear_error(target);
            }
            DevEvent::PatchFailed { target, .. } => {
                if let Some(s) = self.save(target) {
                    if s.ms.is_none() {
                        s.detail = "patch failed, rebuilding".into();
                    }
                }
            }
            DevEvent::BuildStarted { target, cause } => {
                let label = match cause {
                    BuildCause::Initial | BuildCause::OneShot => "building",
                    BuildCause::Save { .. } | BuildCause::Forced => "rebuilding",
                };
                if matches!(cause, BuildCause::Forced) {
                    let at_ms = self.now_ms;
                    self.push_save(Save {
                        at_ms,
                        target: target.clone(),
                        files: Vec::new(),
                        tier: "rebuild".into(),
                        ms: None,
                        detail: "requested".into(),
                        acked: false,
                        failed: false,
                    });
                }
                if let Some(s) = self.save(target) {
                    if s.ms.is_none() {
                        s.tier = "rebuild".into();
                    }
                }
                self.set(
                    target,
                    State::Building { label, stage: None, compiled: 0, total: None, current: None },
                );
                self.target(target).pending.clear();
            }
            DevEvent::StageStarted { target, stage } => {
                if let State::Building { stage: s, .. } = &mut self.target(target).state {
                    *s = Some(stage.clone());
                }
            }
            DevEvent::CargoProgress { target, compiled, total, current } => {
                if let State::Building { compiled: c, total: t, current: cur, .. } =
                    &mut self.target(target).state
                {
                    *c = *compiled;
                    *t = *total;
                    if current.is_some() {
                        *cur = current.clone();
                    }
                }
            }
            DevEvent::Diagnostic { target, diagnostic } => {
                if diagnostic.is_error() {
                    self.target(target).pending.push(diagnostic.clone());
                }
            }
            DevEvent::BuildFinished { target, outcome, ms } if self.is_server(target) => {
                let built = secs_text(*ms);
                match outcome {
                    BuildOutcome::Ready { .. } => {
                        self.set(
                            target,
                            State::Launching { label: "starting", note: format!("built in {built}") },
                        );
                        self.clear_error(target);
                    }
                    BuildOutcome::Reloaded { .. } | BuildOutcome::PremintRefreshed { .. } => {
                        self.set(
                            target,
                            State::Launching {
                                label: "restarting",
                                note: format!("restarted · rebuilt in {built}"),
                            },
                        );
                        self.finish_save(target, "rebuild", "restarted".into(), false);
                        self.clear_error(target);
                    }
                    BuildOutcome::Unchanged => {
                        self.server_untouched(target, format!("rebuilt, nothing changed · {built}"));
                        self.finish_save(target, "rebuild", "no change".into(), false);
                        self.clear_error(target);
                    }
                    BuildOutcome::Failed { error } => self.failed(target, error),
                }
            }
            DevEvent::BuildFinished { target, outcome, ms } => match outcome {
                BuildOutcome::Ready { .. } => {
                    self.set(target, State::Ready { ms: *ms });
                    self.clear_error(target);
                }
                BuildOutcome::Reloaded { .. } | BuildOutcome::PremintRefreshed { .. } => {
                    self.set(target, State::Reloaded { ms: *ms });
                    self.finish_save(target, "rebuild", "reloaded".into(), false);
                    self.clear_error(target);
                }
                BuildOutcome::Unchanged => {
                    self.set(target, State::Unchanged { ms: *ms });
                    self.finish_save(target, "rebuild", "no change".into(), false);
                    self.clear_error(target);
                }
                BuildOutcome::Failed { error } => self.failed(target, error),
            },
            DevEvent::SidecarApplied { target, how, ms, .. } => {
                match how {
                    SidecarUpdate::HotPatch => {
                        self.set(target, State::Patched { redirected: None, ms: *ms });
                        self.finish_save(target, "hot patch", String::new(), false);
                    }
                    SidecarUpdate::Respawn => {
                        self.set(target, State::Reloaded { ms: *ms });
                        self.finish_save(target, "respawn", String::new(), false);
                    }
                }
                self.clear_error(target);
            }
            DevEvent::PageAck { target, ack } => match ack {
                PageAck::Overlay { .. } | PageAck::HotPatch { .. } | PageAck::Reloading { .. } => {
                    if let Some(s) = self.save(target) {
                        s.acked = true;
                    }
                }
                PageAck::Failed { what, error } => {
                    self.log_line(format!("[{target} page] {what} failed: {error}"));
                }
                PageAck::Connected { .. } => {}
            },
            // The native launchers speak in `[dev ios] …` lines, not typed
            // events yet: their latest line is their row's status until
            // something typed arrives.
            DevEvent::Log { source, line } | DevEvent::Warning { source, message: line } => {
                if let Some(name) = source.strip_prefix("dev ") {
                    let quiet = self
                        .targets
                        .iter()
                        .find(|t| t.name == name)
                        .is_some_and(|t| matches!(t.state, State::Starting | State::Note { .. }));
                    if quiet {
                        self.set(name, State::Note { line: line.clone() });
                    }
                }
            }
            // The server process crashed or panicked (the CLI reads its
            // output): its row says so, and the error pane quotes the end
            // of its output — the rest is in server.log.
            DevEvent::Error { source, message } if self.is_server(source) => {
                let target = source.clone();
                let tail: Vec<String> = self
                    .log
                    .iter()
                    .filter(|l| l.server)
                    .rev()
                    .take(CRASH_TAIL)
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let mut rendered = message.clone();
                if !tail.is_empty() {
                    rendered.push_str("\n\n");
                    rendered.push_str(&tail.join("\n"));
                }
                self.set(&target, State::Failed { summary: message.clone() });
                self.error = Some(ErrorInfo {
                    target,
                    location: None,
                    message: message.clone(),
                    rendered,
                    count: 0,
                });
            }
            DevEvent::Error { .. }
            | DevEvent::StreamRoute { .. }
            | DevEvent::Output { .. }
            | DevEvent::StageFinished { .. }
            | DevEvent::BuildTimed { .. } => {}
        }
    }

    /// A build of `target` failed: the row says where, the error pane
    /// shows the first rustc error (or the build's own error).
    fn failed(&mut self, target: &str, error: &str) {
        let pending = std::mem::take(&mut self.target(target).pending);
        let info = match pending.first() {
            Some(d) => ErrorInfo {
                target: target.to_string(),
                location: d.location(),
                message: d.message.clone(),
                rendered: d.rendered.trim_end().to_string(),
                count: pending.len(),
            },
            None => ErrorInfo {
                target: target.to_string(),
                location: None,
                message: error.to_string(),
                rendered: error.to_string(),
                count: 0,
            },
        };
        let summary = match &info.location {
            Some(loc) => format!("{loc} {}", info.message),
            None => info.message.clone(),
        };
        self.set(target, State::Failed { summary });
        self.finish_save(target, "rebuild", "build failed".into(), true);
        self.error = Some(info);
    }

    fn is_server(&self, target: &str) -> bool {
        self.server.as_deref() == Some(target)
    }

    /// Back to running on the address it last answered on, after a
    /// rebuild that did not restart it. Without one yet (its first build
    /// failed, so it never started) it waits for the next build.
    fn server_untouched(&mut self, target: &str, note: String) {
        let state = match self.server_url.clone() {
            Some(url) => State::Running { url, note, fresh: true },
            None => State::Queued,
        };
        self.set(target, state);
    }

    fn clear_error(&mut self, target: &str) {
        if self.error.as_ref().is_some_and(|e| e.target == target) {
            self.error = None;
        }
    }
}

/// `41.2s`, as the rows print build times.
fn secs_text(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dev_events::{Mode, SCHEMA_VERSION};

    fn web() -> String {
        "web".into()
    }

    /// Feed events at the given session times.
    fn run(model: &mut Model, events: Vec<(u64, DevEvent)>) {
        for (i, (at_ms, event)) in events.into_iter().enumerate() {
            model.apply(&Envelope { v: SCHEMA_VERSION, seq: i as u64 + 1, at_ms, event });
        }
    }

    fn started() -> Model {
        let mut m = Model::new(&[]);
        run(
            &mut m,
            vec![
                (
                    0,
                    DevEvent::SessionStarted {
                        app: "Lab".into(),
                        targets: vec![web()],
                        mode: Mode::Local,
                        hot_tier: HotTier::Armed,
                        log_file: None,
                        server: None,
                    },
                ),
                (5, DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial }),
                (6, DevEvent::StageStarted { target: web(), stage: "cargo".into() }),
                (
                    9,
                    DevEvent::CargoProgress {
                        target: web(),
                        compiled: 12,
                        total: Some(40),
                        current: Some("idea-ui".into()),
                    },
                ),
            ],
        );
        m
    }

    #[test]
    fn the_initial_build_shows_its_stage_and_progress_then_ready() {
        let mut m = started();
        assert_eq!(
            m.targets[0].state,
            State::Building {
                label: "building",
                stage: Some("cargo".into()),
                compiled: 12,
                total: Some(40),
                current: Some("idea-ui".into()),
            }
        );
        assert_eq!(m.targets[0].since_ms, 5);
        run(
            &mut m,
            vec![(900, DevEvent::BuildFinished {
                target: web(),
                outcome: BuildOutcome::Ready { gen: 1 },
                ms: 895,
            })],
        );
        assert_eq!(m.targets[0].state, State::Ready { ms: 895 });
        assert!(m.history.is_empty(), "the initial build is not a save");
    }

    #[test]
    fn an_overlay_save_goes_changed_then_applying_then_applied_and_is_acked() {
        let mut m = started();
        run(
            &mut m,
            vec![
                (1000, DevEvent::ChangeDetected {
                    target: web(),
                    paths: vec!["/lab/src/app.rs".into()],
                    crates: vec![],
                    folded: 0,
                }),
            ],
        );
        assert_eq!(m.targets[0].state, State::Changed { files: vec!["app.rs".into()] });
        run(&mut m, vec![(1010, DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 1 } })]);
        assert_eq!(m.targets[0].state, State::ApplyingOverlay { sites: 1 });
        run(
            &mut m,
            vec![
                (1012, DevEvent::OverlayPushed { target: web(), sites: 1, ms: 12 }),
                (1030, DevEvent::PageAck {
                    target: web(),
                    ack: PageAck::Overlay { applied: Some(1), refused: Some(0) },
                }),
            ],
        );
        assert_eq!(m.targets[0].state, State::Applied { sites: 1, ms: 12 });
        let save = &m.history[0];
        assert_eq!(
            (save.tier.as_str(), save.ms, save.detail.as_str(), save.acked),
            ("overlay", Some(12), "1 site", true)
        );
    }

    #[test]
    fn a_failed_patch_falls_to_a_rebuild_whose_failure_carries_the_first_error() {
        let mut m = started();
        let diag = |msg: &str, line: u32| DevEvent::Diagnostic {
            target: web(),
            diagnostic: Diagnostic {
                level: "error".into(),
                message: msg.into(),
                code: Some("E0308".into()),
                file: Some("src/app.rs".into()),
                line: Some(line),
                column: Some(9),
                package: None,
                rendered: format!("error[E0308]: {msg}\n --> src/app.rs:{line}:9\n"),
                ansi: None,
            },
        };
        run(
            &mut m,
            vec![
                (100, DevEvent::ChangeDetected { target: web(), paths: vec!["src/app.rs".into()], crates: vec![], folded: 0 }),
                (101, DevEvent::Decided {
                    target: web(),
                    decision: Decision::HotPatch { crates: vec!["app".into()], files: vec!["src/app.rs".into()] },
                }),
            ],
        );
        assert_eq!(m.targets[0].state, State::BuildingPatch { crates: vec!["app".into()] });
        run(
            &mut m,
            vec![
                (500, DevEvent::PatchFailed { target: web(), files: vec![], reason: "rustc".into() }),
                (501, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } }),
                (700, diag("mismatched types", 12)),
                (701, diag("second", 20)),
                (900, DevEvent::BuildFinished {
                    target: web(),
                    outcome: BuildOutcome::Failed { error: "cargo exited".into() },
                    ms: 399,
                }),
            ],
        );
        assert_eq!(
            m.targets[0].state,
            State::Failed { summary: "src/app.rs:12:9 mismatched types".into() }
        );
        let err = m.error.as_ref().unwrap();
        assert_eq!(err.count, 2);
        assert!(err.rendered.starts_with("error[E0308]: mismatched types"));
        assert!(m.history[0].failed);
        assert_eq!(m.history[0].tier, "rebuild");

        // The next patch that lands clears it.
        run(
            &mut m,
            vec![
                (1000, DevEvent::ChangeDetected { target: web(), paths: vec!["src/app.rs".into()], crates: vec![], folded: 0 }),
                (1001, DevEvent::Decided {
                    target: web(),
                    decision: Decision::HotPatch { crates: vec!["app".into()], files: vec!["src/app.rs".into()] },
                }),
                (1400, DevEvent::PatchBuilt {
                    target: web(),
                    files: vec!["src/app.rs".into()],
                    crates: vec![],
                    redirected: 3,
                    steps: vec![],
                    skipped: vec![],
                    bytes: 1,
                    ms: 399,
                }),
            ],
        );
        assert!(m.error.is_none());
        assert_eq!(m.targets[0].state, State::Patched { redirected: Some(3), ms: 399 });
        assert_eq!(m.history[0].detail, "3 fn redirected");
    }

    #[test]
    fn the_history_ring_keeps_the_newest_saves() {
        let mut m = started();
        for i in 0..(HISTORY_CAP as u64 + 5) {
            run(
                &mut m,
                vec![
                    (1000 + i * 10, DevEvent::ChangeDetected {
                        target: web(),
                        paths: vec![format!("src/f{i}.rs")],
                        crates: vec![],
                        folded: 0,
                    }),
                    (1001 + i * 10, DevEvent::Decided { target: web(), decision: Decision::Unchanged }),
                ],
            );
        }
        assert_eq!(m.history.len(), HISTORY_CAP);
        assert_eq!(m.history[0].files, vec![format!("f{}.rs", HISTORY_CAP + 4)]);
        assert_eq!(m.history[0].tier, "no change");
        m.clear();
        assert!(m.history.is_empty() && m.log.is_empty());
        assert_eq!(m.targets.len(), 1, "clearing keeps the rows");
    }

    #[test]
    fn a_target_first_seen_in_an_event_gets_a_row() {
        let mut m = started();
        run(
            &mut m,
            vec![(10, DevEvent::BuildStarted { target: "server".into(), cause: BuildCause::Save { folded: 0 } })],
        );
        assert_eq!(m.targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["web", "server"]);
    }

    #[test]
    fn a_native_target_shows_its_latest_launcher_line_until_it_has_typed_events() {
        let mut m = Model::new(&["ios".to_string()]);
        run(
            &mut m,
            vec![
                (1, DevEvent::Log { source: "dev ios".into(), line: "building + launching simulator…".into() }),
                (2, DevEvent::Log { source: "dev web".into(), line: "not a row".into() }),
            ],
        );
        assert_eq!(m.targets[0].state, State::Note { line: "building + launching simulator…".into() });
        assert_eq!(m.targets.len(), 1, "a launcher line for another target makes no row");
        run(&mut m, vec![(3, DevEvent::BuildStarted { target: "ios".into(), cause: BuildCause::Initial })]);
        run(&mut m, vec![(4, DevEvent::Log { source: "dev ios".into(), line: "later".into() })]);
        assert!(matches!(m.targets[0].state, State::Building { .. }), "a typed state is not overwritten");
    }

    fn server() -> String {
        dev_events::SERVER_TARGET.into()
    }

    /// A full-stack session: web and a declared server.
    fn full_stack() -> Model {
        let mut m = Model::new(&[web()]);
        run(
            &mut m,
            vec![(0, DevEvent::SessionStarted {
                app: "CrewForge".into(),
                targets: vec![web()],
                mode: Mode::Local,
                hot_tier: HotTier::Armed,
                log_file: None,
                server: Some(
                    dev_events::SessionServer::named("crewforge-server")
                        .with_log_file("/cf/target/idealyst/crewforge-main/server.log"),
                ),
            })],
        );
        m
    }

    fn row<'a>(m: &'a Model, name: &str) -> &'a Target {
        m.targets.iter().find(|t| t.name == name).unwrap()
    }

    /// The bug: the server's build "just sits there with no indication
    /// that a server even exists". A declared server is a row from the
    /// session's first event, before its build says anything.
    #[test]
    fn regression_the_server_is_a_row_from_the_first_frame() {
        let m = full_stack();
        let rows: Vec<(&str, &str)> =
            m.targets.iter().map(|t| (t.name.as_str(), t.title.as_str())).collect();
        assert_eq!(rows, vec![("web", "web"), ("server", "crewforge-server")]);
        assert_eq!(row(&m, "server").state, State::Queued);
        assert_eq!(m.server.as_deref(), Some("server"));
        assert_eq!(m.server_log.as_deref(), Some("/cf/target/idealyst/crewforge-main/server.log"));
    }

    #[test]
    fn the_server_builds_starts_and_runs_on_its_url() {
        let mut m = full_stack();
        run(
            &mut m,
            vec![
                (10, DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial }),
                (12, DevEvent::BuildStarted { target: server(), cause: BuildCause::Initial }),
                (900, DevEvent::CargoProgress {
                    target: server(),
                    compiled: 120,
                    total: Some(480),
                    current: Some("sqlx".into()),
                }),
            ],
        );
        // Both building at once, each with its own progress.
        assert!(matches!(row(&m, "web").state, State::Building { .. }));
        assert_eq!(
            row(&m, "server").state,
            State::Building {
                label: "building",
                stage: None,
                compiled: 120,
                total: Some(480),
                current: Some("sqlx".into()),
            }
        );
        run(&mut m, vec![(41_012, DevEvent::BuildFinished {
            target: server(),
            outcome: BuildOutcome::Ready { gen: 1 },
            ms: 41_000,
        })]);
        assert_eq!(
            row(&m, "server").state,
            State::Launching { label: "starting", note: "built in 41.0s".into() }
        );
        assert!(row(&m, "server").state.busy(), "starting spins until the port answers");
        run(&mut m, vec![(41_900, DevEvent::ServerReady {
            target: web(),
            kind: ServerKind::FullStack,
            url: "http://127.0.0.1:3100".into(),
        })]);
        assert_eq!(
            row(&m, "server").state,
            State::Running {
                url: "http://127.0.0.1:3100".into(),
                note: "built in 41.0s".into(),
                fresh: false,
            }
        );
        assert!(m.history.is_empty(), "starting up is not a save");
    }

    fn running() -> Model {
        let mut m = full_stack();
        run(
            &mut m,
            vec![
                (1, DevEvent::BuildStarted { target: server(), cause: BuildCause::Initial }),
                (2, DevEvent::BuildFinished { target: server(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 1 }),
                (3, DevEvent::ServerReady {
                    target: web(),
                    kind: ServerKind::FullStack,
                    url: "http://127.0.0.1:3100".into(),
                }),
                (4, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 4 }),
            ],
        );
        m
    }

    /// A save only the web bundle sees leaves the server's row exactly as
    /// it was.
    #[test]
    fn a_web_only_save_leaves_the_server_untouched() {
        let mut m = running();
        let before = row(&m, "server").clone();
        run(
            &mut m,
            vec![
                (100, DevEvent::ChangeDetected { target: web(), paths: vec!["/cf/app-main/src/a.rs".into()], crates: vec![], folded: 0 }),
                (101, DevEvent::Decided {
                    target: web(),
                    decision: Decision::HotPatch { crates: vec!["app".into()], files: vec![] },
                }),
                (500, DevEvent::PatchBuilt {
                    target: web(),
                    files: vec![],
                    crates: vec![],
                    redirected: 2,
                    steps: vec![],
                    skipped: vec![],
                    bytes: 1,
                    ms: 400,
                }),
            ],
        );
        assert_eq!(*row(&m, "server"), before);
        assert_eq!(row(&m, "web").state, State::Patched { redirected: Some(2), ms: 400 });
    }

    #[test]
    fn a_server_save_rebuilds_restarts_and_runs_again() {
        let mut m = running();
        run(
            &mut m,
            vec![
                (1000, DevEvent::ChangeDetected {
                    target: server(),
                    paths: vec!["/cf/crates/api-server/src/routes.rs".into()],
                    crates: vec![],
                    folded: 0,
                }),
                (1450, DevEvent::BuildStarted { target: server(), cause: BuildCause::Save { folded: 0 } }),
            ],
        );
        assert!(matches!(row(&m, "server").state, State::Building { label: "rebuilding", .. }));
        assert_eq!(m.history[0].target, "server");
        assert_eq!(m.history[0].files, vec!["routes.rs".to_string()]);
        run(&mut m, vec![(13_450, DevEvent::BuildFinished {
            target: server(),
            outcome: BuildOutcome::Reloaded { gen: 2 },
            ms: 12_000,
        })]);
        assert_eq!(
            row(&m, "server").state,
            State::Launching { label: "restarting", note: "restarted · rebuilt in 12.0s".into() }
        );
        run(&mut m, vec![(14_000, DevEvent::ServerReady {
            target: web(),
            kind: ServerKind::FullStack,
            url: "http://127.0.0.1:3100".into(),
        })]);
        assert_eq!(
            row(&m, "server").state,
            State::Running {
                url: "http://127.0.0.1:3100".into(),
                note: "restarted · rebuilt in 12.0s".into(),
                fresh: true,
            }
        );
        let save = &m.history[0];
        assert_eq!((save.tier.as_str(), save.ms, save.detail.as_str()), ("rebuild", Some(12_450), "restarted"));
    }

    #[test]
    fn a_server_rebuild_that_changed_nothing_keeps_it_running() {
        let mut m = running();
        run(
            &mut m,
            vec![
                (10, DevEvent::BuildStarted { target: server(), cause: BuildCause::Save { folded: 0 } }),
                (3_110, DevEvent::BuildFinished { target: server(), outcome: BuildOutcome::Unchanged, ms: 3_100 }),
            ],
        );
        assert_eq!(
            row(&m, "server").state,
            State::Running {
                url: "http://127.0.0.1:3100".into(),
                note: "rebuilt, nothing changed · 3.1s".into(),
                fresh: true,
            }
        );
    }

    /// The server process's own output is marked, so the log pane can
    /// leave it out; a crash is an error on the server's row, and the
    /// error pane quotes the end of its output.
    #[test]
    fn server_output_is_marked_and_a_crash_lands_on_its_row() {
        let mut m = running();
        let out = |line: &str| DevEvent::Output {
            source: dev_events::SERVER_OUTPUT_SOURCE.into(),
            line: line.into(),
            target: Some(server()),
        };
        run(
            &mut m,
            vec![
                (20, out("GET / 200")),
                (21, DevEvent::Output { source: "cargo".into(), line: "   Compiling app".into(), target: Some(web()) }),
                (22, out("thread 'main' panicked at src/main.rs:9:5:")),
                (23, out("boom")),
                (24, DevEvent::Error {
                    source: server(),
                    message: "exited with exit status: 101 — its output is in /cf/server.log".into(),
                }),
            ],
        );
        let server_lines: Vec<&str> =
            m.log.iter().filter(|l| l.server).map(|l| l.text.as_str()).collect();
        assert_eq!(
            server_lines,
            vec!["[server] GET / 200", "[server] thread 'main' panicked at src/main.rs:9:5:", "[server] boom"]
        );
        assert!(m.log.iter().any(|l| !l.server && l.text == "   Compiling app"));
        assert_eq!(
            row(&m, "server").state,
            State::Failed { summary: "exited with exit status: 101 — its output is in /cf/server.log".into() }
        );
        let err = m.error.as_ref().unwrap();
        assert_eq!(err.target, "server");
        assert!(err.rendered.ends_with("[server] thread 'main' panicked at src/main.rs:9:5:\n[server] boom"), "{}", err.rendered);
        // A restart clears the row.
        run(&mut m, vec![(30, DevEvent::ServerReady {
            target: web(),
            kind: ServerKind::FullStack,
            url: "http://127.0.0.1:3100".into(),
        })]);
        assert!(matches!(row(&m, "server").state, State::Running { .. }));
    }

    #[test]
    fn the_log_pane_gets_the_plain_lines_without_colour() {
        let mut m = started();
        run(
            &mut m,
            vec![(10, DevEvent::Output {
                source: "cargo".into(),
                line: "\u{1b}[1m\u{1b}[92m   Compiling\u{1b}[0m app v0.1.0".into(),
                target: Some(web()),
            })],
        );
        assert_eq!(m.log.back().unwrap().text, "   Compiling app v0.1.0");
    }
}
