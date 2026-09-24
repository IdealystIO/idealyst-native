//! Shared test data.

use dev_events::{
    BuildCause, BuildOutcome, CrateTiming, Decision, DevEvent, Diagnostic, HotTier, Mode, PageAck,
    ServerKind, SessionServer, SidecarUpdate, StreamRoute, Timing, SERVER_TARGET,
};

/// One of every variant, so a variant added without serde support (or
/// with a tag that collides) fails here.
pub fn every_event() -> Vec<DevEvent> {
    let web = || "web".to_string();
    vec![
        DevEvent::SessionStarted {
            app: "Lab".into(),
            targets: vec!["web".into(), "ios".into()],
            mode: Mode::RuntimeServer,
            hot_tier: HotTier::Off { reason: "runtime-server mode".into() },
            log_file: Some("/t/dev.log".into()),
            server: None,
        },
        // A full-stack session declares its server's row.
        DevEvent::SessionStarted {
            app: "CrewForge".into(),
            targets: vec!["web".into()],
            mode: Mode::Local,
            hot_tier: HotTier::Armed,
            log_file: None,
            server: Some(SessionServer::named("crewforge-server")),
        },
        DevEvent::ServerReady { target: web(), kind: ServerKind::FullStack, url: "http://127.0.0.1:3100".into() },
        DevEvent::BuildStarted { target: SERVER_TARGET.into(), cause: BuildCause::Initial },
        DevEvent::CargoProgress { target: SERVER_TARGET.into(), compiled: 3, total: Some(9), current: None },
        DevEvent::BuildFinished {
            target: SERVER_TARGET.into(),
            outcome: BuildOutcome::Ready { gen: 1 },
            ms: 41_000,
        },
        DevEvent::ServerReady { target: web(), kind: ServerKind::ReloadStream, url: "http://x".into() },
        DevEvent::StreamRoute {
            target: web(),
            route: StreamRoute::SameOrigin,
            url: "http://127.0.0.1:3100/__idealyst/reload".into(),
        },
        DevEvent::ServerReady { target: "session".into(), kind: ServerKind::Events, url: "http://127.0.0.1:1/__idealyst/events".into() },
        DevEvent::Watching { target: web(), roots: vec!["/a".into()], rewatch: true },
        DevEvent::ChangeDetected {
            target: web(),
            paths: vec!["/a/src/app.rs".into()],
            crates: vec!["a".into()],
            folded: 1,
        },
        DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 2 } },
        DevEvent::Decided {
            target: web(),
            decision: Decision::HotPatch { crates: vec!["a".into()], files: vec!["src/app.rs".into()] },
        },
        DevEvent::Decided { target: web(), decision: Decision::Rebuild { reason: None } },
        DevEvent::Decided { target: web(), decision: Decision::Unchanged },
        DevEvent::OverlayPushed { target: web(), sites: 2, ms: 3 },
        DevEvent::PatchBuilt {
            target: web(),
            files: vec!["src/app.rs".into()],
            crates: vec![CrateTiming { name: "a".into(), ms: None }],
            redirected: 4,
            steps: vec![Timing { name: "cargo".into(), ms: 300 }],
            skipped: vec![],
            bytes: 1024,
            ms: 310,
        },
        DevEvent::PatchFailed { target: web(), files: vec![], reason: "r".into() },
        DevEvent::BuildStarted { target: web(), cause: BuildCause::Forced },
        DevEvent::StageStarted { target: web(), stage: "cargo".into() },
        DevEvent::StageFinished { target: web(), stage: "cargo".into(), ms: 1 },
        DevEvent::CargoProgress { target: web(), compiled: 1, total: None, current: Some("a".into()) },
        DevEvent::Diagnostic {
            target: web(),
            diagnostic: Diagnostic {
                level: "warning".into(),
                message: "unused".into(),
                code: None,
                file: None,
                line: None,
                column: None,
                package: None,
                rendered: "warning: unused".into(),
                ansi: None,
            },
        },
        DevEvent::BuildTimed { target: web(), stages: vec![], total_ms: 0 },
        DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::Failed { error: "e".into() },
            ms: 9,
        },
        DevEvent::PageAck { target: web(), ack: PageAck::Overlay { applied: None, refused: None } },
        DevEvent::PageAck {
            target: web(),
            ack: PageAck::Failed { what: "hot_patch".into(), error: "x".into() },
        },
        DevEvent::SidecarApplied {
            target: "runtime-server".into(),
            how: SidecarUpdate::Respawn,
            ms: 900,
            reason: Some("rebuild".into()),
        },
        DevEvent::Warning { source: "s".into(), message: "m".into() },
        DevEvent::Error { source: "s".into(), message: "m".into() },
        DevEvent::Log { source: "dev".into(), line: "l".into() },
        DevEvent::Output { source: "cargo".into(), line: "   Compiling a".into(), target: None },
        DevEvent::Output {
            source: dev_events::SERVER_OUTPUT_SOURCE.into(),
            line: "GET /api/health 200".into(),
            target: Some(SERVER_TARGET.into()),
        },
        DevEvent::SessionStarted {
            app: "CrewForge".into(),
            targets: vec!["web".into()],
            mode: Mode::Local,
            hot_tier: HotTier::Armed,
            log_file: None,
            server: Some(
                SessionServer::named("crewforge-server")
                    .with_log_file("/cf/target/idealyst/crewforge-main/server.log"),
            ),
        },
    ]
}

