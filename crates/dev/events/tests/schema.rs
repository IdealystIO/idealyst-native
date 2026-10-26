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

/// Every `type` the schema allows, read from the schema itself. The schema
/// is derived from `DevEvent`, so this is the enum's variant set, which a
/// match here cannot enumerate (`DevEvent` is `#[non_exhaustive]`).
fn schema_types(schema: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    match schema {
        serde_json::Value::Object(map) => {
            if let Some(tag) = map.get("properties").and_then(|p| p.get("type")) {
                if let Some(c) = tag.get("const").and_then(|c| c.as_str()) {
                    out.insert(c.to_string());
                }
                for e in tag.get("enum").and_then(|e| e.as_array()).into_iter().flatten() {
                    if let Some(e) = e.as_str() {
                        out.insert(e.to_string());
                    }
                }
            }
            map.values().for_each(|v| schema_types(v, out));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| schema_types(v, out)),
        _ => {}
    }
}

#[test]
fn every_variant_validates() {
    let v = validator();
    let every = common::every_event();
    // The list must stay exhaustive: a variant added to `DevEvent` without
    // a fixture in `common::every_event` fails here.
    let seen: std::collections::BTreeSet<String> = every
        .iter()
        .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string())
        .collect();
    let mut all = std::collections::BTreeSet::new();
    schema_types(&json_schema(), &mut all);
    assert!(all.len() >= 28, "the schema walk found too few event types: {all:?}");
    assert_eq!(seen, all, "`common::every_event` does not cover every DevEvent variant");
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
