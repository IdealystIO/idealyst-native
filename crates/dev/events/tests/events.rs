//! The event vocabulary's wire form, the JSON-lines sink, and cargo's
//! message stream parsed from a real capture.

use std::sync::Arc;

use dev_events::cargo::{closure_size, CargoStream};
use dev_events::{
    BuildCause, BuildOutcome, CrateTiming, Decision, DevEvent, Diagnostic, Envelope, HotTier,
    JsonLines, Mode, PageAck, Reporter, ServerKind, Timing,
};

/// One of every variant, so a variant added without serde support (or
/// with a tag that collides) fails here.
fn every_event() -> Vec<DevEvent> {
    let web = || "web".to_string();
    vec![
        DevEvent::SessionStarted {
            app: "Lab".into(),
            targets: vec!["web".into(), "ios".into()],
            mode: Mode::RuntimeServer,
            hot_tier: HotTier::Off { reason: "runtime-server mode".into() },
            log_file: Some("/t/dev.log".into()),
        },
        DevEvent::ServerReady { target: web(), kind: ServerKind::ReloadStream, url: "http://x".into() },
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
        DevEvent::Warning { source: "s".into(), message: "m".into() },
        DevEvent::Error { source: "s".into(), message: "m".into() },
        DevEvent::Log { source: "dev".into(), line: "l".into() },
        DevEvent::Output { source: "cargo".into(), line: "   Compiling a".into() },
    ]
}

#[test]
fn every_event_round_trips_through_json() {
    for (i, event) in every_event().into_iter().enumerate() {
        let envelope = Envelope { seq: i as u64 + 1, at_ms: 40 * i as u64, event };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(!json.contains('\n'));
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, envelope, "{json}");
    }
}

#[test]
fn the_wire_form_is_flat_and_tagged() {
    // What an editor extension reads: the envelope fields and the event's
    // `type` side by side, snake_case throughout.
    let envelope = Envelope {
        seq: 7,
        at_ms: 1234,
        event: DevEvent::Decided { target: "web".into(), decision: Decision::Overlay { sites: 1 } },
    };
    let v = serde_json::to_value(&envelope).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "seq": 7,
            "at_ms": 1234,
            "type": "decided",
            "target": "web",
            "decision": { "tier": "overlay", "sites": 1 }
        })
    );
}

#[test]
fn the_json_lines_sink_writes_one_object_per_line_in_order() {
    let dir = tempfile_dir();
    let path = dir.join("events.jsonl");
    let r = Reporter::new();
    r.add_sink(Arc::new(JsonLines::file(&path).unwrap()));
    for event in every_event() {
        r.emit(event);
    }
    let text = std::fs::read_to_string(&path).unwrap();
    let parsed: Vec<Envelope> =
        text.lines().map(|l| serde_json::from_str(l).expect(l)).collect();
    assert_eq!(parsed.len(), every_event().len());
    assert_eq!(parsed.iter().map(|e| e.seq).collect::<Vec<_>>(), (1..=parsed.len() as u64).collect::<Vec<_>>());
    assert_eq!(parsed.into_iter().map(|e| e.event).collect::<Vec<_>>(), every_event());
    let _ = std::fs::remove_dir_all(dir);
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dev-events-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const STDOUT: &str = include_str!("fixtures/cargo_stdout.jsonl");
const STDERR: &str = include_str!("fixtures/cargo_stderr.txt");
const METADATA: &str = include_str!("fixtures/cargo_metadata.json");

/// The capture is a real `cargo build --message-format=
/// json-diagnostic-rendered-ansi --color=always` of a two-crate
/// workspace (`fixture-app` with a build script, depending on
/// `fixture-helper`, plus a dev-only `fixture-devonly`) whose `main.rs`
/// has a type error. Paths are rewritten to `/work`.
#[test]
fn a_captured_cargo_stream_becomes_progress_and_a_structured_error() {
    let metadata: serde_json::Value = serde_json::from_str(METADATA).unwrap();
    // app + helper; the dev-dependency is not part of `cargo build`.
    assert_eq!(closure_size(&metadata), Some(2));

    let mut stream = CargoStream::new("web");
    let mut events = vec![stream.set_total(closure_size(&metadata))];
    for line in STDERR.lines() {
        events.extend(stream.stderr_line(line));
    }
    for line in STDOUT.lines() {
        events.extend(stream.stdout_line(line));
    }

    let progress: Vec<(u32, Option<u32>, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            DevEvent::CargoProgress { compiled, total, current, .. } => {
                Some((*compiled, *total, current.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        progress,
        vec![
            (0, Some(2), None),
            // The two `Compiling` lines name the crate in flight.
            (0, Some(2), Some("fixture-app".into())),
            (0, Some(2), Some("fixture-helper".into())),
            // helper's artifact, then app's build-script artifact. The
            // app's `build-script-executed` is not progress.
            (1, Some(2), Some("fixture-helper".into())),
            (2, Some(2), Some("fixture-helper".into())),
        ]
    );

    let diagnostics: Vec<&Diagnostic> = events
        .iter()
        .filter_map(|e| match e {
            DevEvent::Diagnostic { diagnostic, .. } => Some(diagnostic),
            _ => None,
        })
        .collect();
    assert_eq!(diagnostics.len(), 2, "the error and its `rustc --explain` note");
    let error = diagnostics[0];
    assert_eq!(error.level, "error");
    assert_eq!(error.message, "mismatched types");
    assert_eq!(error.code.as_deref(), Some("E0308"));
    assert_eq!(error.location().as_deref(), Some("src/main.rs:3:18"));
    assert!(error.rendered.starts_with("error[E0308]: mismatched types"), "{}", error.rendered);
    assert!(!error.rendered.contains('\x1b'), "the plain rendering carries no colour codes");
    assert!(error.ansi.as_deref().is_some_and(|a| a.contains('\x1b')));
    assert_eq!(stream.errors(), 1);
    assert_eq!(diagnostics[1].level, "failure-note");

    // Every stderr line is passed through, colour intact.
    let outputs: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            DevEvent::Output { line, .. } => Some(line.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(outputs, STDERR.lines().collect::<Vec<_>>());
}

#[test]
fn a_non_json_stdout_line_is_passed_through() {
    let mut stream = CargoStream::new("web");
    assert_eq!(
        stream.stdout_line("hello from a build script"),
        vec![DevEvent::Output { source: "cargo".into(), line: "hello from a build script".into() }]
    );
}

#[test]
fn progress_is_clamped_to_the_total() {
    let mut stream = CargoStream::new("web");
    stream.set_total(Some(1));
    for id in ["a", "b", "c"] {
        stream.stdout_line(&format!(r#"{{"reason":"compiler-artifact","package_id":"{id}"}}"#));
    }
    assert_eq!(stream.compiled(), 1);
}
