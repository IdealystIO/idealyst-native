//! Robot test controls: what the app asked the OS to show, for E2E tests
//! to assert on — a test can't read the device's notification shade, and
//! on a simulator without the grant nothing appears there at all. Compiled
//! only into robot builds (`runtime_vocabulary::robot_only!` in `lib.rs`).

use std::sync::Mutex;

use runtime_core::__serde_json::{json, Value};
use runtime_core::robot_fn;
use runtime_vocabulary::robot_fn::register as register_fn;

use crate::{Notification, NotificationId, NotifyError};

/// Every post, cancel and cancel-all since launch (or the last
/// `notifications.clear_log`), oldest first. A `Mutex`: a post's future
/// may complete on whichever thread the backend resolves it.
static LOG: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn push(entry: Value) {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).push(entry);
}

/// Record a `notify` (`delay_ms: None`) or `schedule` and its outcome.
pub(crate) fn record_post(n: &Notification, delay_ms: Option<u128>, outcome: &Result<NotificationId, NotifyError>) {
    let mut data: Vec<(&String, &String)> = n.data.iter().collect();
    data.sort();
    push(json!({
        "op": if delay_ms.is_some() { "schedule" } else { "notify" },
        "id": outcome.as_ref().ok().map(|id| id.0.clone()).or_else(|| n.id.as_ref().map(|id| id.0.clone())),
        "title": n.title,
        "body": n.body,
        "subtitle": n.subtitle,
        "data": data.into_iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<runtime_core::__serde_json::Map<_, _>>(),
        "delay_ms": delay_ms.map(|d| d as u64),
        "error": outcome.as_ref().err().map(ToString::to_string),
    }));
}

pub(crate) fn record_cancel(id: Option<&NotificationId>) {
    match id {
        Some(id) => push(json!({ "op": "cancel", "id": id.0 })),
        None => push(json!({ "op": "cancel_all" })),
    }
}

/// What the app posted, scheduled and cancelled, oldest first: `{op:
/// "notify"|"schedule", id, title, body, subtitle, data, delay_ms, error}`
/// (`error` is set when the post failed, e.g. without the permission), or
/// `{op: "cancel", id}` / `{op: "cancel_all"}`.
#[robot_fn(name = "notifications.log")]
fn log() -> Vec<Value> {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Empty the log `notifications.log` reads.
#[robot_fn(name = "notifications.clear_log")]
fn clear_log() {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub(crate) fn register() {
    register_fn(log::robot());
    register_fn(clear_log::robot());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::robot::bridge::invoke_command;

    fn call(name: &str) -> Value {
        let out = invoke_command("call_function", &json!({ "name": name, "args": {} })).unwrap();
        runtime_core::__serde_json::from_str::<Value>(&out).unwrap()["value"].clone()
    }

    /// The log records each post with its outcome, and cancels; clear
    /// empties it. One test: `LOG` is process-wide.
    #[test]
    fn the_log_records_posts_outcomes_and_cancels() {
        crate::register_robot_controls();
        call("notifications.clear_log");
        let n = Notification::new("Saved", "Your note is safe").id("note-1").with("route", "/notes/1");
        record_post(&n, None, &Ok(NotificationId("note-1".into())));
        record_post(&Notification::new("Later", "b"), Some(60_000), &Err(NotifyError::NotAuthorized));
        record_cancel(Some(&NotificationId("note-1".into())));
        record_cancel(None);

        let log = call("notifications.log");
        assert_eq!(
            log,
            json!([
                { "op": "notify", "id": "note-1", "title": "Saved", "body": "Your note is safe", "subtitle": null,
                  "data": { "route": "/notes/1" }, "delay_ms": null, "error": null },
                { "op": "schedule", "id": null, "title": "Later", "body": "b", "subtitle": null,
                  "data": {}, "delay_ms": 60000, "error": "notifications not authorized" },
                { "op": "cancel", "id": "note-1" },
                { "op": "cancel_all" },
            ])
        );
        call("notifications.clear_log");
        assert_eq!(call("notifications.log"), json!([]));
    }
}
