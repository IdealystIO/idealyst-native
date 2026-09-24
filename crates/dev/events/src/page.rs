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
/// servers other than the one serving a full-stack page), and the page's
/// own acks, which it already knows.
pub fn wants(event: &DevEvent) -> bool {
    match event {
        DevEvent::SessionStarted { .. }
        | DevEvent::ChangeDetected { .. }
        | DevEvent::Decided { .. }
        | DevEvent::OverlayPushed { .. }
        | DevEvent::PatchBuilt { .. }
        | DevEvent::PatchFailed { .. }
        | DevEvent::BuildStarted { .. }
        | DevEvent::StageStarted { .. }
        | DevEvent::CargoProgress { .. }
        | DevEvent::BuildFinished { .. }
        | DevEvent::SidecarApplied { .. }
        | DevEvent::Error { .. } => true,
        // Errors only: a page shows what stopped the build, and a crate's
        // warnings would bury it.
        DevEvent::Diagnostic { diagnostic, .. } => diagnostic.is_error(),
        // A full-stack page is served BY this server: the badge says
        // "restarting server" through a restart, and this is what ends it.
        // The other servers are addresses the page already knows.
        DevEvent::ServerReady { kind, .. } => *kind == ServerKind::FullStack,
        DevEvent::Watching { .. }
        | DevEvent::StreamRoute { .. }
        | DevEvent::StageFinished { .. }
        | DevEvent::BuildTimed { .. }
        | DevEvent::PageAck { .. }
        | DevEvent::Warning { .. }
        | DevEvent::Log { .. }
        | DevEvent::Output { .. } => false,
    }
}
