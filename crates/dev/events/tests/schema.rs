//! The event schema is a contract: every object a session emits must
//! validate against the schema `idealyst dev --events-schema` prints.
//!
//! Checked two ways: one of every event variant (so a variant added
//! without the schema noticing fails here), and a recorded session — a
//! real run's `--events-file`, trimmed — so the check covers what the
//! dev loop actually emits, not only what this file thought to build.

mod common;
#[path = "support/validator.rs"]
mod validator;

use dev_events::{json_schema, DevEvent, Envelope, SCHEMA_VERSION};

fn validator() -> validator::Validator {
    validator::Validator::new(json_schema())
}

fn check(v: &validator::Validator, instance: &serde_json::Value) {
    let errors = v.errors(instance);
    assert!(errors.is_empty(), "{instance}\nfails the schema: {errors:#?}");
}

#[test]
fn the_schema_carries_its_version() {
    let schema = json_schema();
    assert_eq!(schema["x-schema-version"], SCHEMA_VERSION);
    assert!(schema["$id"].as_str().unwrap().ends_with(&format!("/v{SCHEMA_VERSION}.json")));
}

#[test]
fn every_variant_validates() {
    let v = validator();
    let every = common::every_event();
    // The list must stay exhaustive: this match fails to compile when
    // a variant is added, pointing here.
    let mut seen = std::collections::BTreeSet::new();
    for event in &every {
        seen.insert(match event {
            DevEvent::SessionStarted { .. } => "session_started",
            DevEvent::ServerReady { .. } => "server_ready",
            DevEvent::Watching { .. } => "watching",
            DevEvent::ChangeDetected { .. } => "change_detected",
            DevEvent::Decided { .. } => "decided",
            DevEvent::OverlayPushed { .. } => "overlay_pushed",
            DevEvent::PatchBuilt { .. } => "patch_built",
            DevEvent::PatchFailed { .. } => "patch_failed",
            DevEvent::BuildStarted { .. } => "build_started",
            DevEvent::StageStarted { .. } => "stage_started",
            DevEvent::StageFinished { .. } => "stage_finished",
            DevEvent::CargoProgress { .. } => "cargo_progress",
            DevEvent::Diagnostic { .. } => "diagnostic",
            DevEvent::BuildTimed { .. } => "build_timed",
            DevEvent::BuildFinished { .. } => "build_finished",
            DevEvent::PageAck { .. } => "page_ack",
            DevEvent::SidecarApplied { .. } => "sidecar_applied",
            DevEvent::Warning { .. } => "warning",
            DevEvent::Error { .. } => "error",
            DevEvent::Log { .. } => "log",
            DevEvent::Output { .. } => "output",
        });
    }
    assert_eq!(seen.len(), 21, "`common::every_event` is missing a variant: has {seen:?}");
    for (i, event) in every.into_iter().enumerate() {
        let envelope = Envelope { v: SCHEMA_VERSION, seq: i as u64 + 1, at_ms: 5, event };
        check(&v, &serde_json::to_value(&envelope).unwrap());
    }
}

#[test]
fn a_recorded_session_validates() {
    let v = validator();
    let recorded = include_str!("fixtures/recorded_session.jsonl");
    let mut n = 0;
    for line in recorded.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).expect(line);
        check(&v, &value);
        // And it reads back as the typed envelope.
        let _: Envelope = serde_json::from_value(value).expect(line);
        n += 1;
    }
    assert!(n > 10, "the recording is a real session, not a stub ({n} events)");
}

#[test]
fn an_object_off_the_contract_is_rejected() {
    let v = validator();
    let bad = serde_json::json!({"v": 1, "seq": 1, "at_ms": 0, "type": "decided", "target": "web"});
    assert!(!v.errors(&bad).is_empty(), "a decided event without its decision must not validate");
    let wrong_version = serde_json::json!({
        "v": SCHEMA_VERSION + 1, "seq": 1, "at_ms": 0, "type": "log", "source": "dev", "line": "x"
    });
    assert!(!v.errors(&wrong_version).is_empty(), "another major version must not validate");
    let unknown_type = serde_json::json!({"v": 1, "seq": 1, "at_ms": 0, "type": "teleported"});
    assert!(!v.errors(&unknown_type).is_empty());
}
