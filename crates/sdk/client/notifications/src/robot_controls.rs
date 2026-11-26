//! Robot test controls: what the app asked the OS to show, for E2E tests
//! to assert on — a test can't read the device's notification shade, and
//! on a simulator without the grant nothing appears there at all — and a
//! way to act as the user responding to one, since a test can't tap the
//! shade either. Compiled only into robot builds
//! (`runtime_vocabulary::robot_only!` in `lib.rs`).

use std::sync::Mutex;

use runtime_core::__serde_json::{json, Value};
use runtime_core::robot_fn;
use runtime_vocabulary::robot_fn::register as register_fn;

use std::collections::HashMap;

use crate::{Notification, NotificationId, NotificationResponse, NotifyError, ResponseAction};

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
        "actions": n.actions.iter().map(|a| json!({
            "id": a.id,
            "title": a.title,
            "input": a.input.is_some(),
            "opens_app": a.opens_app,
        })).collect::<Vec<_>>(),
        "delay_ms": delay_ms.map(|d| d as u64),
        "error": outcome.as_ref().err().map(ToString::to_string),
    }));
}

/// Record a response as it is handed to the app's handlers.
pub(crate) fn record_response(r: &NotificationResponse) {
    let mut data: Vec<(&String, &String)> = r.data.iter().collect();
    data.sort();
    let (action, action_id) = match &r.action {
        ResponseAction::Open => ("open", None),
        ResponseAction::Dismiss => ("dismiss", None),
        ResponseAction::Action(id) => ("action", Some(id.clone())),
    };
    push(json!({
        "op": "response",
        "id": r.id.0,
        "action": action,
        "action_id": action_id,
        "text": r.text,
        "data": data.into_iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<runtime_core::__serde_json::Map<_, _>>(),
    }));
}

pub(crate) fn record_cancel(id: Option<&NotificationId>) {
    match id {
        Some(id) => push(json!({ "op": "cancel", "id": id.0 })),
        None => push(json!({ "op": "cancel_all" })),
    }
}

/// What the app posted, scheduled and cancelled, and the responses its
/// handlers received, oldest first: `{op: "notify"|"schedule", id, title,
/// body, subtitle, data, actions, delay_ms, error}` (`error` is set when
/// the post failed, e.g. without the permission), `{op: "cancel", id}` /
/// `{op: "cancel_all"}`, or `{op: "response", id, action:
/// "open"|"dismiss"|"action", action_id, text, data}`.
#[robot_fn(name = "notifications.log")]
fn log() -> Vec<Value> {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Empty the log `notifications.log` reads.
#[robot_fn(name = "notifications.clear_log")]
fn clear_log() {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Act as the user responding to notification `id`: `action` is `"open"`
/// (a tap, the default), `"dismiss"`, or an action id; `text` is the reply
/// typed into the action's field. The response reaches the app's
/// `on_response` handlers exactly as a real one does — queued, then
/// handed over on the UI thread. `data` defaults to the payload of the
/// last post logged under `id`.
#[robot_fn(name = "notifications.respond")]
fn respond(id: String, action: Option<String>, text: Option<String>, data: Option<HashMap<String, String>>) {
    let action = match action.as_deref() {
        None | Some("open") => ResponseAction::Open,
        Some("dismiss") => ResponseAction::Dismiss,
        Some(other) => ResponseAction::Action(other.to_string()),
    };
    let data = data.unwrap_or_else(|| posted_data(&id));
    crate::deliver_response(NotificationId(id), action, text, data);
}

/// The payload of the most recent logged post under `id`.
fn posted_data(id: &str) -> HashMap<String, String> {
    let log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    log.iter()
        .rev()
        .find(|e| matches!(e["op"].as_str(), Some("notify" | "schedule")) && e["id"].as_str() == Some(id))
        .and_then(|e| e["data"].as_object())
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default()
}

pub(crate) fn register() {
    register_fn(log::robot());
    register_fn(clear_log::robot());
    register_fn(respond::robot());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::robot::bridge::invoke_command;

    fn call_with(name: &str, args: Value) -> Value {
        let out = invoke_command("call_function", &json!({ "name": name, "args": args })).unwrap();
        runtime_core::__serde_json::from_str::<Value>(&out).unwrap()["value"].clone()
    }

    fn call(name: &str) -> Value {
        call_with(name, json!({}))
    }

    /// The log records each post with its outcome, cancels, and the
    /// responses `notifications.respond` hands to the app's handlers;
    /// clear empties it. One test: `LOG` is process-wide.
    #[test]
    fn the_log_records_posts_outcomes_cancels_and_responses() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let _queue = crate::response::exclusive_queue();
        crate::register_robot_controls();
        call("notifications.clear_log");
        let n = Notification::new("Saved", "Your note is safe")
            .id("note-1")
            .with("route", "/notes/1")
            .action(crate::Action::new("undo", "Undo").opens_app(false));
        record_post(&n, None, &Ok(NotificationId("note-1".into())));
        record_post(&Notification::new("Later", "b"), Some(60_000), &Err(NotifyError::NotAuthorized));
        record_cancel(Some(&NotificationId("note-1".into())));
        record_cancel(None);

        assert_eq!(
            call("notifications.log"),
            json!([
                { "op": "notify", "id": "note-1", "title": "Saved", "body": "Your note is safe", "subtitle": null,
                  "data": { "route": "/notes/1" },
                  "actions": [{ "id": "undo", "title": "Undo", "input": false, "opens_app": false }],
                  "delay_ms": null, "error": null },
                { "op": "schedule", "id": null, "title": "Later", "body": "b", "subtitle": null,
                  "data": {}, "actions": [], "delay_ms": 60000, "error": "notifications not authorized" },
                { "op": "cancel", "id": "note-1" },
                { "op": "cancel_all" },
            ])
        );

        // Acting as the user: the response reaches the app's handler with
        // the posted payload, and is logged as handled.
        let seen: Rc<RefCell<Vec<NotificationResponse>>> = Rc::default();
        let sink = seen.clone();
        let _sub = crate::on_response(move |r| sink.borrow_mut().push(r.clone()));
        call("notifications.clear_log");
        record_post(&n, None, &Ok(NotificationId("note-1".into())));
        call_with("notifications.respond", json!({ "id": "note-1", "action": "undo" }));
        call_with("notifications.respond", json!({ "id": "chat", "action": "reply", "text": "hi", "data": { "k": "v" } }));
        call_with("notifications.respond", json!({ "id": "note-1" }));
        assert_eq!(
            *seen.borrow(),
            vec![
                NotificationResponse {
                    id: NotificationId("note-1".into()),
                    action: ResponseAction::Action("undo".into()),
                    text: None,
                    data: HashMap::from([("route".to_string(), "/notes/1".to_string())]),
                },
                NotificationResponse {
                    id: NotificationId("chat".into()),
                    action: ResponseAction::Action("reply".into()),
                    text: Some("hi".into()),
                    data: HashMap::from([("k".to_string(), "v".to_string())]),
                },
                NotificationResponse {
                    id: NotificationId("note-1".into()),
                    action: ResponseAction::Open,
                    text: None,
                    data: HashMap::from([("route".to_string(), "/notes/1".to_string())]),
                },
            ]
        );
        let log = call("notifications.log");
        assert_eq!(
            log[1],
            json!({ "op": "response", "id": "note-1", "action": "action", "action_id": "undo", "text": null,
                    "data": { "route": "/notes/1" } })
        );
        assert_eq!(log[3]["action"], "open");

        call("notifications.clear_log");
        assert_eq!(call("notifications.log"), json!([]));
    }
}
