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
        )
    }
}

/// One target's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
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
    pub log: VecDeque<String>,
    pub error: Option<ErrorInfo>,
    /// The latest session time seen.
    pub now_ms: u64,
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
        for l in dev_events::plain::strip_ansi(&line).lines() {
            self.log.push_back(l.to_string());
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
        // would have shown.
        if let Some(line) = dev_events::plain::render(&e.event) {
            self.log_line(line);
        }
        match &e.event {
            DevEvent::SessionStarted { app, targets, mode, hot_tier, log_file } => {
                self.app = app.clone();
                self.mode = mode.as_str().to_string();
                self.hot_tier = Some(hot_tier.clone());
                self.log_file = log_file.clone();
                for t in targets {
                    self.target(t);
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
                BuildOutcome::Failed { error } => {
                    let pending = std::mem::take(&mut self.target(target).pending);
                    let info = match pending.first() {
                        Some(d) => ErrorInfo {
                            target: target.clone(),
                            location: d.location(),
                            message: d.message.clone(),
                            rendered: d.rendered.trim_end().to_string(),
                            count: pending.len(),
                        },
                        None => ErrorInfo {
                            target: target.clone(),
                            location: None,
                            message: error.clone(),
                            rendered: error.clone(),
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
            DevEvent::Error { .. }
            | DevEvent::Output { .. }
            | DevEvent::StageFinished { .. }
            | DevEvent::BuildTimed { .. } => {}
        }
    }

    fn clear_error(&mut self, target: &str) {
        if self.error.as_ref().is_some_and(|e| e.target == target) {
            self.error = None;
        }
    }
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

    #[test]
    fn the_log_pane_gets_the_plain_lines_without_colour() {
        let mut m = started();
        run(
            &mut m,
            vec![(10, DevEvent::Output {
                source: "cargo".into(),
                line: "\u{1b}[1m\u{1b}[92m   Compiling\u{1b}[0m app v0.1.0".into(),
            })],
        );
        assert_eq!(m.log.back().unwrap(), "   Compiling app v0.1.0");
    }
}
