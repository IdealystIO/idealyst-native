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
//! - the last few [`DevEvent::Error`]s.
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
    targets: BTreeMap<String, Episode>,
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
            DevEvent::Watching { target, .. } => {
                self.watching.insert(target.clone(), e);
            }
            // How the page reaches the stream: state, like an address.
            DevEvent::StreamRoute { target, .. } => {
                self.servers.insert((target.clone(), "stream_route".into()), e);
            }
            DevEvent::ChangeDetected { target, .. } => {
                self.targets.insert(target.clone(), Episode { events: vec![e], closed: false });
            }
            DevEvent::BuildStarted { target, cause } => {
                let ep = self.targets.entry(target.clone()).or_default();
                // A save's build continues the save's episode; the initial
                // build and a forced one start their own.
                let continues = !ep.closed
                    && !ep.events.is_empty()
                    && matches!(cause, BuildCause::Save { .. });
                if !continues {
                    *ep = Episode::default();
                }
                ep.events.push(e);
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
            | DevEvent::PatchBuilt { target, .. }
            | DevEvent::OverlayPushed { target, .. }
            | DevEvent::SidecarApplied { target, .. } => {
                let ep = self.targets.entry(target.clone()).or_default();
                ep.events.push(e);
                ep.closed = true;
            }
            DevEvent::StageStarted { target, .. }
            | DevEvent::StageFinished { target, .. }
            | DevEvent::BuildTimed { target, .. }
            | DevEvent::PatchFailed { target, .. }
            | DevEvent::PageAck { target, .. } => {
                self.targets.entry(target.clone()).or_default().events.push(e);
            }
            DevEvent::Error { .. } => {
                self.errors.push(e);
                if self.errors.len() > MAX_ERRORS {
                    self.errors.remove(0);
                }
            }
            // History, not state: a late consumer does not need old log
            // lines to render the present.
            DevEvent::Warning { .. } | DevEvent::Log { .. } | DevEvent::Output { .. } => {}
        }
    }

    /// The events that describe the present, in sequence order.
    pub fn snapshot(&self) -> Vec<Envelope> {
        let mut out: Vec<Envelope> = self
            .session
            .iter()
            .chain(self.servers.values())
            .chain(self.watching.values())
            .chain(self.targets.values().flat_map(|ep| ep.events.iter()))
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
