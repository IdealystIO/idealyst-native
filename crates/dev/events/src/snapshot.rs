//! The session's current state, as the events that describe it.
//!
//! A consumer that attaches mid-session — a second browser tab, an
//! editor extension started after `idealyst dev`, a page that just
//! reloaded after a failed build — needs to render "what is happening
//! now" without having seen the history. [`SessionState`] keeps exactly
//! the events that say so, and [`SessionState::snapshot`] replays them:
//!
//! - the latest [`DevEvent::SessionStarted`] and each server's
//!   [`DevEvent::ServerReady`] and watch set;
//! - per target, the latest EPISODE: everything from the change (or the
//!   build) that started it through its outcome and the page's acks —
//!   with cargo progress collapsed to the latest count, and at most
//!   [`MAX_DIAGNOSTICS`] diagnostics;
//! - per target, the last episode that applied a rebuild request, once a
//!   later episode has replaced it, and the last [`MAX_REQUESTS`]
//!   [`DevEvent::RebuildRequested`]s, so a consumer waiting on a request
//!   finds its outcome after other saves have followed it;
//! - the Robot relay's state and the latest list of apps connected to it;
//! - the last few [`DevEvent::Error`]s and [`DevEvent::ServerDown`]s.
//!
//! A snapshot is a prefix-free replay: its events keep their original
//! envelopes (sequence numbers, timestamps), in sequence order, so a
//! consumer processes them exactly as it processes live events.

use std::collections::BTreeMap;

use crate::{BuildCause, Decision, DevEvent, Envelope};

/// Diagnostics kept per episode. A build that fails with more has said
/// what it needed to by then.
pub const MAX_DIAGNOSTICS: usize = 64;

/// Errors kept outside any episode.
const MAX_ERRORS: usize = 8;

/// Rebuild requests kept. Each is a handful of bytes; a consumer waits on
/// one it made moments ago.
pub const MAX_REQUESTS: usize = 32;

#[derive(Debug, Default, Clone)]
struct Episode {
    events: Vec<Envelope>,
    /// Whether the episode's outcome has arrived. A build that starts
    /// while an episode is OPEN belongs to it (the save that needed the
    /// build); one that starts after it closed begins a new one.
    closed: bool,
}

/// See the module docs.
#[derive(Debug, Default, Clone)]
pub struct SessionState {
    session: Option<Envelope>,
    servers: BTreeMap<(String, String), Envelope>,
    watching: BTreeMap<String, Envelope>,
    /// Per target, the watcher's `watcher_ready`: kept so a consumer that
    /// attaches later knows the watcher is taking saves. A new watcher's
    /// `watching` (not a rewatch) clears it.
    ready: BTreeMap<String, Envelope>,
    targets: BTreeMap<String, Episode>,
    /// Per target, the latest closed episode that applied a rebuild
    /// request ([`DevEvent::RebuildTaken`]), once a newer episode replaced
    /// it in `targets`.
    answered: BTreeMap<String, Episode>,
    requests: Vec<Envelope>,
    errors: Vec<Envelope>,
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event in.
    pub fn apply(&mut self, envelope: &Envelope) {
        let e = envelope.clone();
        match &envelope.event {
            DevEvent::SessionStarted { .. } => {
                // A new session: nothing before it describes the present.
                *self = Self::default();
                self.session = Some(e);
            }
            DevEvent::ServerReady { target, kind, .. } => {
                let kind = serde_json::to_string(kind).unwrap_or_default();
                self.servers.insert((target.clone(), kind), e);
            }
            DevEvent::Watching { target, rewatch, .. } => {
                if !rewatch {
                    self.ready.remove(target);
                }
                self.watching.insert(target.clone(), e);
            }
            DevEvent::WatcherReady { target } => {
                self.ready.insert(target.clone(), e);
            }
            // How the page reaches the stream: state, like an address.
            DevEvent::StreamRoute { target, .. } => {
                self.servers.insert((target.clone(), "stream_route".into()), e);
            }
            DevEvent::ChangeDetected { target, .. } => self.begin(target, e),
            DevEvent::BuildStarted { target, cause } => {
                let ep = self.targets.entry(target.clone()).or_default();
                // A save's build continues the save's episode; the initial
                // build and a forced one start their own.
                let continues = !ep.closed
                    && !ep.events.is_empty()
                    && matches!(cause, BuildCause::Save { .. });
                if continues {
                    ep.events.push(e);
                } else {
                    self.begin(target, e);
                }
            }
            DevEvent::RebuildRequested { .. } => {
                self.requests.push(e);
                if self.requests.len() > MAX_REQUESTS {
                    self.requests.remove(0);
                }
            }
            // Joins the open episode, or opens one: the watcher sends it
            // before the work that applies the request reports anything.
            DevEvent::RebuildTaken { target, .. } => {
                let open = self.targets.get(target).is_some_and(|ep| !ep.closed && !ep.events.is_empty());
                if open {
                    self.targets.get_mut(target).expect("checked").events.push(e);
                } else {
                    self.begin(target, e);
                }
            }
            DevEvent::CargoProgress { target, .. } => {
                let ep = self.targets.entry(target.clone()).or_default();
                ep.events.retain(|x| !matches!(x.event, DevEvent::CargoProgress { .. }));
                ep.events.push(e);
            }
            DevEvent::Diagnostic { target, .. } => {
                let ep = self.targets.entry(target.clone()).or_default();
                let n = ep
                    .events
                    .iter()
                    .filter(|x| matches!(x.event, DevEvent::Diagnostic { .. }))
                    .count();
                if n < MAX_DIAGNOSTICS {
                    ep.events.push(e);
                }
            }
            DevEvent::Decided { target, decision } => {
                let ep = self.targets.entry(target.clone()).or_default();
                ep.events.push(e);
                if matches!(decision, Decision::Unchanged) {
                    ep.closed = true;
                }
            }
            DevEvent::BuildFinished { target, .. }
            | DevEvent::Superseded { target, .. }
            | DevEvent::PatchBuilt { target, .. }
            | DevEvent::OverlayPushed { target, .. }
            | DevEvent::SidecarApplied { target, .. } => {
                let ep = self.targets.entry(target.clone()).or_default();
                ep.events.push(e);
                ep.closed = true;
            }
            DevEvent::StageStarted { target, .. }
            | DevEvent::StageFinished { target, .. }
            | DevEvent::PatchStep { target, .. }
            | DevEvent::BuildTimed { target, .. }
            | DevEvent::PatchFailed { target, .. }
            | DevEvent::PageAck { target, .. } => {
                self.targets.entry(target.clone()).or_default().events.push(e);
            }
            // State, like a server's address: the latest says it all.
            DevEvent::RobotRelay { .. } => {
                self.servers.insert(("robot".into(), "relay".into()), e);
            }
            DevEvent::RobotApps { .. } => {
                self.servers.insert(("robot".into(), "apps".into()), e);
            }
            DevEvent::Error { .. } | DevEvent::ServerDown { .. } => {
                self.errors.push(e);
                if self.errors.len() > MAX_ERRORS {
                    self.errors.remove(0);
                }
            }
            // History, not state: a late consumer does not need old log
            // lines to render the present.
            DevEvent::Warning { .. } | DevEvent::Log { .. } | DevEvent::Output { .. } => {}
            // Per-unit history, hundreds to a build: nothing a consumer
            // joining later needs to know the session's present.
            DevEvent::UnitBuilt { .. } | DevEvent::UnitDirty { .. } => {}
        }
    }

    /// Start `target`'s next episode with `e`. The requests an OPEN
    /// episode took carry into it: a watcher reports taking a request
    /// first, then the change or build that applies it. A closed episode
    /// that applied a request is kept in `answered`, unless it closed by
    /// being superseded, which applied nothing (its restart takes the
    /// request again).
    fn begin(&mut self, target: &str, e: Envelope) {
        let old = self.targets.remove(target).unwrap_or_default();
        let took = |x: &Envelope| matches!(x.event, DevEvent::RebuildTaken { .. });
        let mut events = Vec::new();
        if !old.closed {
            events.extend(old.events.iter().filter(|x| took(x)).cloned());
        } else if old.events.iter().any(took)
            && !old.events.last().is_some_and(|x| matches!(x.event, DevEvent::Superseded { .. }))
        {
            self.answered.insert(target.to_string(), old);
        }
        events.push(e);
        self.targets.insert(target.to_string(), Episode { events, closed: false });
    }

    /// The events that describe the present, in sequence order.
    pub fn snapshot(&self) -> Vec<Envelope> {
        let mut out: Vec<Envelope> = self
            .session
            .iter()
            .chain(self.servers.values())
            .chain(self.watching.values())
            .chain(self.ready.values())
            .chain(self.answered.values().flat_map(|ep| ep.events.iter()))
            .chain(self.targets.values().flat_map(|ep| ep.events.iter()))
            .chain(self.requests.iter())
            .chain(self.errors.iter())
            .cloned()
            .collect();
        out.sort_by_key(|e| e.seq);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildOutcome, Mode, HotTier, SCHEMA_VERSION};

    fn env(seq: u64, event: DevEvent) -> Envelope {
        Envelope { v: SCHEMA_VERSION, seq, at_ms: seq * 10, event }
    }

    fn types(events: &[Envelope]) -> Vec<String> {
        events
            .iter()
            .map(|e| serde_json::to_value(&e.event).unwrap()["type"].as_str().unwrap().to_string())
            .collect()
    }

    /// A consumer that attaches after the watcher became ready learns it
    /// from the snapshot; a new watcher (not a rewatch) clears it.
    #[test]
    fn the_snapshot_says_whether_each_watcher_is_taking_saves() {
        let watching = |rewatch| DevEvent::Watching { target: "web".into(), roots: vec![], rewatch };
        let ready = || DevEvent::WatcherReady { target: "web".into() };
        let mut s = SessionState::new();
        for (i, e) in [watching(false), ready(), watching(true)].into_iter().enumerate() {
            s.apply(&env(i as u64 + 1, e));
        }
        assert_eq!(types(&s.snapshot()), vec!["watcher_ready", "watching"], "a rewatch keeps the readiness");
        s.apply(&env(4, watching(false)));
        assert_eq!(types(&s.snapshot()), vec!["watching"], "a new watcher is not ready yet");
    }

    /// A consumer waiting on a rebuild request finds the build that
    /// applied it after a later save replaced it as the latest episode.
    /// The build reports taking the request before it starts, and a
    /// superseded attempt is not kept as the one that applied it.
    #[test]
    fn regression_a_rebuild_requests_outcome_outlives_the_saves_after_it() {
        let web = || "web".to_string();
        let taken = || DevEvent::RebuildTaken { target: web(), request: 1 };
        let forced = || DevEvent::BuildStarted { target: web(), cause: BuildCause::Forced };
        let mut s = SessionState::new();
        let events = vec![
            DevEvent::RebuildRequested { request: 1, targets: vec![web()] },
            taken(),
            forced(),
            DevEvent::Superseded { target: web(), work: crate::SupersededWork::Rebuild, ms: 3 },
            taken(),
            forced(),
            DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Reloaded { gen: 2 }, ms: 9 },
            DevEvent::ChangeDetected { target: web(), paths: vec!["a.rs".into()], crates: vec![], folded: 0 },
            DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 1 } },
            DevEvent::OverlayPushed { target: web(), sites: 1, ms: 1 },
        ];
        for (i, e) in events.into_iter().enumerate() {
            s.apply(&env(i as u64 + 1, e));
        }
        let snap = s.snapshot();
        assert_eq!(
            snap.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 5, 6, 7, 8, 9, 10],
            "the request, the restart that applied it (not the superseded attempt), then the latest save"
        );
    }

    /// A watcher reports taking the request, then the change or forced
    /// build that applies it, which starts a new episode: the request
    /// must stay in it.
    #[test]
    fn a_taken_request_stays_in_the_episode_its_build_starts() {
        let web = || "web".to_string();
        let mut s = SessionState::new();
        let events = vec![
            DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 9 },
            DevEvent::RebuildTaken { target: web(), request: 4 },
            DevEvent::ChangeDetected { target: web(), paths: vec!["a.rs".into()], crates: vec![], folded: 0 },
            DevEvent::Decided { target: web(), decision: Decision::Rebuild { reason: None } },
        ];
        for (i, e) in events.into_iter().enumerate() {
            s.apply(&env(i as u64 + 1, e));
        }
        assert_eq!(types(&s.snapshot()), vec!["rebuild_taken", "change_detected", "decided"]);
    }

    #[test]
    fn a_late_consumer_sees_the_session_and_each_targets_last_episode() {
        let web = || "web".to_string();
        let mut s = SessionState::new();
        let events = vec![
            DevEvent::SessionStarted {
                app: "Lab".into(),
                targets: vec![web()],
                mode: Mode::Local,
                hot_tier: HotTier::Armed,
                log_file: None,
                server: None,
            },
            DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial },
            DevEvent::CargoProgress { target: web(), compiled: 1, total: Some(3), current: None },
            DevEvent::CargoProgress { target: web(), compiled: 3, total: Some(3), current: None },
            DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 9 },
            DevEvent::Log { source: "dev".into(), line: "noise".into() },
            // A save: the old episode is replaced.
            DevEvent::ChangeDetected { target: web(), paths: vec!["a.rs".into()], crates: vec![], folded: 0 },
            DevEvent::Decided { target: web(), decision: Decision::Rebuild { reason: None } },
            DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } },
            DevEvent::CargoProgress { target: web(), compiled: 2, total: Some(3), current: None },
        ];
        for (i, e) in events.into_iter().enumerate() {
            s.apply(&env(i as u64 + 1, e));
        }
        let snap = s.snapshot();
        assert_eq!(
            types(&snap),
            vec!["session_started", "change_detected", "decided", "build_started", "cargo_progress"]
        );
        assert!(snap.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    /// A superseded rebuild never finishes: `superseded` is what closes its
    /// episode, so the restart's episode (a new `change_detected`, then
    /// its build) is the one a late consumer sees.
    #[test]
    fn a_superseded_rebuild_closes_its_episode() {
        let web = || "web".to_string();
        let change = |seq| {
            env(seq, DevEvent::ChangeDetected { target: web(), paths: vec!["a.rs".into()], crates: vec![], folded: 0 })
        };
        let mut s = SessionState::new();
        s.apply(&change(1));
        s.apply(&env(2, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } }));
        s.apply(&env(
            3,
            DevEvent::Superseded { target: web(), work: crate::SupersededWork::Rebuild, ms: 900 },
        ));
        assert!(s.targets["web"].closed, "superseded must close the episode");
        assert_eq!(types(&s.snapshot()), vec!["change_detected", "build_started", "superseded"]);

        s.apply(&change(4));
        s.apply(&env(5, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 1 } }));
        assert_eq!(types(&s.snapshot()), vec!["change_detected", "build_started"]);
        assert_eq!(s.snapshot()[0].seq, 4);
    }

    /// A panel or page that attaches after an app connected must still
    /// see it: the relay and the latest app list are state, and only the
    /// latest list describes the present.
    #[test]
    fn a_late_consumer_sees_the_robot_relay_and_the_latest_app_list() {
        let app = |id, active| crate::RobotApp { id, platform: Some("web".into()), label: None, active };
        let mut s = SessionState::new();
        s.apply(&env(1, DevEvent::RobotRelay {
            state: crate::RelayState::Listening { ws_url: "ws://127.0.0.1:1".into(), tcp_port: 2, pinned: false },
        }));
        s.apply(&env(2, DevEvent::RobotApps { apps: vec![app(1, true)] }));
        s.apply(&env(3, DevEvent::RobotApps { apps: vec![app(1, false), app(2, true)] }));
        let snap = s.snapshot();
        assert_eq!(types(&snap), vec!["robot_relay", "robot_apps"]);
        assert_eq!(snap[1].seq, 3, "only the latest list is the present");
    }

    #[test]
    fn a_failed_build_stays_in_the_snapshot_until_the_next_episode() {
        let web = || "web".to_string();
        let mut s = SessionState::new();
        s.apply(&env(1, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } }));
        for i in 0..(MAX_DIAGNOSTICS as u64 + 5) {
            s.apply(&env(
                2 + i,
                DevEvent::Diagnostic {
                    target: web(),
                    diagnostic: crate::Diagnostic {
                        level: "error".into(),
                        message: "boom".into(),
                        code: None,
                        file: None,
                        line: None,
                        column: None,
                        package: None,
                        rendered: "error: boom".into(),
                        ansi: None,
                    },
                },
            ));
        }
        s.apply(&env(
            100,
            DevEvent::BuildFinished {
                target: web(),
                outcome: BuildOutcome::Failed { error: "cargo exited".into() },
                ms: 1,
            },
        ));
        let snap = s.snapshot();
        assert_eq!(
            snap.iter().filter(|e| matches!(e.event, DevEvent::Diagnostic { .. })).count(),
            MAX_DIAGNOSTICS
        );
        assert!(matches!(snap.last().unwrap().event, DevEvent::BuildFinished { .. }));
        // A forced rebuild after the failure starts afresh.
        s.apply(&env(101, DevEvent::BuildStarted { target: web(), cause: BuildCause::Forced }));
        assert_eq!(types(&s.snapshot()), vec!["build_started"]);
    }
}
