//! `dev_status`: a session's present, summarized.
//!
//! A session's `GET /status` answers with its registry entry and its
//! event snapshot (the events that describe its present, as its event
//! stream opens with). That snapshot is the raw material, and much of it
//! is bulk: one `watching` event per watcher listing every watched root
//! (a full-stack workspace watches ~90 directories, from two watchers),
//! every diagnostic of the last build, and a start and a finish per build
//! stage. So the answer folds it: per target, the latest outcome and the
//! work in flight (through the same [`Tracker`] `wait_for_build` reads),
//! how many roots it watches, the warnings of its last build, and the
//! URLs it serves; the session's Robot relay and the apps on it once.
//! An event this does not know is passed through under `other`, so
//! nothing a newer session reports is silently dropped. `detail` returns
//! the snapshot as the session sent it.

use std::collections::BTreeMap;

use dev_events::Envelope;
use serde_json::{json, Value};

use crate::build_wait::Tracker;

/// Fold `body` (`GET /status`'s answer: `session` and `events`).
pub fn summarize(body: &Value) -> Value {
    let session = &body["session"];
    let events: &[Value] = body["events"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let no_watch = session["no_watch"].as_bool().unwrap_or(false);
    let honour_ready = session["reports_watcher_ready"].as_bool().unwrap_or(false);

    let mut tracker = Tracker::default();
    let mut watching: BTreeMap<String, usize> = BTreeMap::new();
    let mut warnings: BTreeMap<String, usize> = BTreeMap::new();
    let mut urls: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut down: BTreeMap<String, Value> = BTreeMap::new();
    let mut acks: BTreeMap<String, Value> = BTreeMap::new();
    let mut started = Value::Null;
    let mut relay = Value::Null;
    let mut apps = Value::Null;
    let mut other: Vec<Value> = Vec::new();

    for e in events {
        if let Ok(envelope) = serde_json::from_value::<Envelope>(e.clone()) {
            tracker.feed(&envelope);
        }
        let target = e["target"].as_str().unwrap_or("session").to_string();
        match e["type"].as_str().unwrap_or("") {
            "session_started" => {
                started = json!({ "mode": e["mode"], "hot_tier": e["hot_tier"], "server": e["server"] });
            }
            "watching" => {
                watching.insert(target, e["roots"].as_array().map_or(0, Vec::len));
            }
            "build_started" => {
                warnings.remove(&target);
            }
            "diagnostic" if e["diagnostic"]["level"] == "warning" => {
                *warnings.entry(target).or_default() += 1;
            }
            "server_ready" => {
                let kind = e["kind"].as_str().unwrap_or("server").to_string();
                urls.entry(target).or_default().insert(kind, e["url"].clone());
            }
            "stream_route" => {
                urls.entry(target).or_default().insert("stream_route".into(), json!({ "route": e["route"], "url": e["url"] }));
            }
            "server_down" => {
                let mut d = e.clone();
                if let Some(o) = d.as_object_mut() {
                    for k in ["v", "seq", "at_ms", "type", "target"] {
                        o.remove(k);
                    }
                }
                down.insert(target, d);
            }
            "page_ack" => {
                acks.insert(target, e["ack"].clone());
            }
            "robot_relay" => {
                let mut r = e.clone();
                if let Some(o) = r.as_object_mut() {
                    for k in ["v", "seq", "at_ms", "type"] {
                        o.remove(k);
                    }
                }
                relay = r;
            }
            "robot_apps" => apps = e["apps"].clone(),
            // Folded by the tracker, or bulk with nothing to add to it.
            "watcher_ready" | "diagnostic" | "stage_started" | "stage_finished" | "cargo_progress"
            | "change_detected" | "decided" | "build_finished" | "build_timed" | "superseded"
            | "patch_built" | "patch_failed" | "patch_step" | "overlay_pushed" | "rebuild_requested"
            | "rebuild_taken" | "sidecar_applied" | "unit_built" | "unit_dirty" | "output" => {}
            _ => other.push(e.clone()),
        }
    }

    let mut targets = tracker.summary(no_watch, honour_ready);
    let names: Vec<String> = watching
        .keys()
        .chain(urls.keys())
        .chain(down.keys())
        .chain(acks.keys())
        .chain(warnings.keys())
        .cloned()
        .collect();
    for name in names {
        targets.entry(name).or_insert_with(|| json!({}));
    }
    for (name, t) in targets.iter_mut() {
        let o = t.as_object_mut().expect("an object");
        if let Some(n) = watching.get(name) {
            o.insert("watched_roots".into(), json!(n));
        }
        if let Some(n) = warnings.get(name) {
            o.insert("warnings".into(), json!(n));
        }
        if let Some(u) = urls.get(name) {
            o.insert("urls".into(), json!(u));
        }
        if let Some(d) = down.get(name) {
            o.insert("server_down".into(), d.clone());
        }
        if let Some(a) = acks.get(name) {
            o.insert("page".into(), a.clone());
        }
    }
    // The session's own servers (its event stream) are not a target.
    let session_urls = targets.remove("session").and_then(|t| t.get("urls").cloned());

    let mut out = json!({
        "session": session,
        // Every event's seq, a kind this server cannot parse included: a
        // `wait_for_build` after it waits past all of them.
        "cursor": events.iter().filter_map(|e| e["seq"].as_u64()).max().unwrap_or(0).max(tracker.cursor()),
        "targets": targets,
    });
    let o = out.as_object_mut().expect("an object");
    if !started.is_null() {
        o.insert("started".into(), started);
    }
    if let Some(u) = session_urls {
        o.insert("urls".into(), u);
    }
    if !relay.is_null() {
        o.insert("robot_relay".into(), relay);
    }
    if !apps.is_null() {
        o.insert("robot_apps".into(), apps);
    }
    if !other.is_empty() {
        o.insert("other".into(), json!(other));
    }
    o.insert(
        "next".into(),
        json!("`detail: true` returns the session's event snapshot as sent, the watched roots and every diagnostic included"),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `dev_status` returned the raw snapshot, which on a
    /// full-stack workspace listed ~90 watched directories twice (the web
    /// and the server watcher) plus every warning and stage event. The
    /// summary keeps what the state is and counts the bulk.
    #[test]
    fn regression_a_status_folds_the_snapshot_and_lists_no_watched_roots() {
        let roots: Vec<String> = (0..90).map(|i| format!("/w/crates/c{i}/src")).collect();
        let warning = |seq: u64| {
            json!({ "v": 1, "seq": seq, "at_ms": seq, "type": "diagnostic", "target": "web",
                    "diagnostic": { "level": "warning", "message": "unused", "code": "dead_code", "rendered": "x".repeat(800) } })
        };
        let body = json!({
            "session": { "pid": 7, "dir": "/w", "no_watch": true, "reports_watcher_ready": true },
            "events": [
                { "v": 1, "seq": 1, "at_ms": 0, "type": "session_started", "app": "A", "targets": ["web"], "mode": "local", "hot_tier": { "state": "armed" } },
                { "v": 1, "seq": 2, "at_ms": 0, "type": "server_ready", "target": "session", "kind": "events", "url": "http://127.0.0.1:1/__idealyst/events" },
                { "v": 1, "seq": 3, "at_ms": 0, "type": "robot_relay", "state": "listening", "pinned": false, "tcp_port": 5, "ws_url": "ws://127.0.0.1:4" },
                { "v": 1, "seq": 4, "at_ms": 0, "type": "watching", "target": "web", "roots": roots, "rewatch": false },
                { "v": 1, "seq": 5, "at_ms": 0, "type": "watching", "target": "server", "roots": roots, "rewatch": false },
                { "v": 1, "seq": 6, "at_ms": 0, "type": "watcher_ready", "target": "web" },
                { "v": 1, "seq": 10, "at_ms": 0, "type": "change_detected", "target": "web", "paths": ["/w/src/a.rs"], "crates": [], "folded": 0 },
                { "v": 1, "seq": 11, "at_ms": 0, "type": "decided", "target": "web", "decision": { "tier": "rebuild", "reason": "a signature changed" } },
                { "v": 1, "seq": 12, "at_ms": 0, "type": "build_started", "target": "web", "cause": "save", "folded": 0 },
                { "v": 1, "seq": 13, "at_ms": 0, "type": "stage_started", "target": "web", "stage": "cargo" },
                warning(14), warning(15), warning(16),
                { "v": 1, "seq": 17, "at_ms": 0, "type": "stage_finished", "target": "web", "stage": "cargo", "ms": 900 },
                { "v": 1, "seq": 18, "at_ms": 0, "type": "build_finished", "target": "web", "outcome": "reloaded", "gen": 3, "ms": 1200 },
                { "v": 1, "seq": 19, "at_ms": 0, "type": "server_ready", "target": "web", "kind": "full_stack", "url": "http://127.0.0.1:3000" },
                { "v": 1, "seq": 20, "at_ms": 0, "type": "robot_apps", "apps": [{ "id": 5, "label": "Chrome", "platform": "web", "active": true }] },
                { "v": 1, "seq": 21, "at_ms": 0, "type": "something_new", "detail": 1 },
            ],
        });
        let out = summarize(&body);
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains("/w/crates/c0/src"), "a watched root was listed: {text}");
        assert!(!text.contains(&"x".repeat(800)), "a diagnostic was listed");
        assert!(text.len() < 2_000, "{} characters: {text}", text.len());

        let web = &out["targets"]["web"];
        assert_eq!(web["watched_roots"], 90);
        assert_eq!(web["warnings"], 3);
        assert_eq!(web["latest"]["outcome"], "reloaded");
        assert_eq!(web["latest"]["tier"], "rebuild");
        assert_eq!(web["latest"]["gen"], 3);
        assert_eq!(web["urls"]["full_stack"], "http://127.0.0.1:3000");
        assert!(web.get("preparing").is_none());
        // The server watcher announced itself and is not ready yet.
        assert_eq!(out["targets"]["server"]["watched_roots"], 90);
        assert_eq!(out["targets"]["server"]["preparing"], true);
        assert!(out["targets"].get("session").is_none());
        assert_eq!(out["urls"]["events"], "http://127.0.0.1:1/__idealyst/events");
        assert_eq!(out["robot_relay"]["state"], "listening");
        assert_eq!(out["robot_apps"][0]["label"], "Chrome");
        assert_eq!(out["started"]["mode"], "local");
        assert_eq!(out["cursor"], 21);
        assert_eq!(out["other"][0]["type"], "something_new", "an unknown event is passed through");
    }

    /// A `--no-watch` save nothing has built yet shows as queued, and a
    /// build in flight as building with its stage.
    #[test]
    fn a_queued_save_and_a_build_in_flight_are_told_apart() {
        let body = json!({
            "session": { "pid": 7, "no_watch": true },
            "events": [
                { "v": 1, "seq": 1, "at_ms": 0, "type": "change_detected", "target": "web", "paths": ["/w/src/a.rs"], "crates": [], "folded": 0 },
                { "v": 1, "seq": 2, "at_ms": 0, "type": "build_started", "target": "server", "cause": "forced" },
                { "v": 1, "seq": 3, "at_ms": 0, "type": "stage_started", "target": "server", "stage": "cargo" },
            ],
        });
        let out = summarize(&body);
        assert_eq!(out["targets"]["web"]["queued"], true);
        assert_eq!(out["targets"]["server"]["building"]["stage"], "cargo");
    }
}
