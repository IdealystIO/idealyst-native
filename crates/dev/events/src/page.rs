//! What a running page is told about the session.
//!
//! The dev server pushes a projection of the event stream to connected
//! pages as `dev-state` events on the reload channel, where the injected
//! status overlay renders it: building with progress, patched with the
//! tier and the time, the rustc error that stopped a build. The objects
//! are the same versioned [`crate::Envelope`]s every other consumer gets —
//! one schema — filtered to what describes the build's state.

use crate::{DevEvent, ServerKind};

/// Whether a page is sent `event`.
///
/// Left out: history (log and subprocess lines, warnings), bookkeeping a
/// page has no use for (watch sets, per-stage timings, the addresses of
/// servers other than the one serving a full-stack page, which units cargo
/// recompiled and why), and the page's
/// own acks, which it already knows.
pub fn wants(event: &DevEvent) -> bool {
    match event {
        DevEvent::SessionStarted { .. }
        | DevEvent::ChangeDetected { .. }
        | DevEvent::Decided { .. }
        | DevEvent::OverlayPushed { .. }
        | DevEvent::PatchBuilt { .. }
        | DevEvent::PatchFailed { .. }
        | DevEvent::Superseded { .. }
        | DevEvent::BuildStarted { .. }
        | DevEvent::StageStarted { .. }
        | DevEvent::CargoProgress { .. }
        | DevEvent::BuildFinished { .. }
        | DevEvent::SidecarApplied { .. }
        | DevEvent::ServerDown { .. }
        | DevEvent::ServerRecompiling { .. }
        | DevEvent::Error { .. } => true,
        // Errors only: a page shows what stopped the build, and a crate's
        // warnings would bury it.
        DevEvent::Diagnostic { diagnostic, .. } => diagnostic.is_error(),
        // A full-stack page is served BY this server: the badge says
        // "restarting server" through a restart, and this is what ends it.
        // The other servers are addresses the page already knows.
        DevEvent::ServerReady { kind, .. } => *kind == ServerKind::FullStack,
        DevEvent::Watching { .. }
        | DevEvent::WatcherReady { .. }
        | DevEvent::RebuildRequested { .. }
        | DevEvent::RebuildTaken { .. }
        | DevEvent::StreamRoute { .. }
        | DevEvent::StageFinished { .. }
        | DevEvent::PatchStep { .. }
        | DevEvent::BuildTimed { .. }
        | DevEvent::UnitBuilt { .. }
        | DevEvent::UnitDirty { .. }
        | DevEvent::CheckStarted { .. }
        | DevEvent::CheckFinished { .. }
        | DevEvent::PageAck { .. }
        | DevEvent::Warning { .. }
        | DevEvent::Log { .. }
        | DevEvent::Output { .. } => false,
        // The page knows its own Robot connection better than the relay
        // does — it reports it itself (`status_overlay.js`'s `robot`).
        DevEvent::RobotRelay { .. } | DevEvent::RobotApps { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page's status line says "starting server" until the server
    /// answers; without this event it kept saying so through a whole
    /// second compile inside `cargo run`.
    #[test]
    fn regression_the_page_hears_that_cargo_run_is_recompiling_the_server() {
        assert!(wants(&DevEvent::ServerRecompiling { target: crate::SERVER_TARGET.into(), cause: None }));
    }

    /// A check applies nothing, so the page's status line must not move.
    #[test]
    fn a_check_says_nothing_to_the_page() {
        assert!(!wants(&DevEvent::CheckStarted { target: "web".into(), request: 1, tests: None }));
        assert!(!wants(&DevEvent::CheckFinished {
            target: "web".into(),
            report: crate::CheckReport {
                request: 1,
                ok: false,
                compiled: vec![],
                errors: vec![],
                failure: None,
                tests: None,
                ms: 1,
            },
        }));
    }
}
