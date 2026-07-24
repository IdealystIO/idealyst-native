//! Linux desktop local notifications via the session-bus service
//! `org.freedesktop.Notifications` (the Desktop Notifications Specification),
//! driven directly through `zbus`.
//!
//! VERIFICATION: type-checked for `x86_64-unknown-linux-gnu`; the argument
//! builder + error mapping are unit-tested below. The live post is exercised
//! by the `#[ignore]`d `live_notify_posts` integration test, which needs a
//! running session bus + a notification daemon (org.freedesktop.Notifications
//! owner) and so can't run in a headless CI job.
//!
//! Mechanism:
//! - `notify` calls the `Notify` method
//!   (`app_name, replaces_id, app_icon, summary, body, actions, hints,
//!   expire_timeout`) and gets back a server-assigned `u32` id.
//! - `cancel` / `cancel_all` call `CloseNotification(u32)`.
//!
//! ## String id ↔ server u32
//!
//! The public API keys notifications by a *string* [`NotificationId`], but the
//! freedesktop protocol keys by a server-assigned `u32`: you pass
//! `replaces_id = 0` for a new post and the daemon returns a fresh id; to
//! *replace* a notification you pass its previous `u32` back as `replaces_id`.
//! We keep a process-global `string id → u32` map so re-posting under the same
//! [`NotificationId`] replaces the on-screen notification (the update
//! semantics the public API promises) and so `cancel(id)` / `cancel_all` can
//! resolve the `u32` to close. There is no `CloseAll` in the spec, so
//! `cancel_all` closes each id we've tracked.
//!
//! ## No subtitle / no scheduling / no push token
//!
//! - The spec has no distinct *subtitle* field, so a subtitle is folded into
//!   the body (documented crate-wide behavior).
//! - The spec has no delayed/scheduled trigger — `expire_timeout` is only how
//!   long a *shown* notification lingers, not a post-later timer. A
//!   sleep-based hack would tie delivery to process liveness rather than an OS
//!   scheduler, so `schedule` reports `NotSupported`, matching Android's
//!   stance on the same missing primitive.
//! - There is no remote-push token on a Linux desktop, so `push_token` reports
//!   `NotSupported` (honest — nothing to obtain).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use zbus::zvariant::Value;
use zbus::Connection;

use crate::{resolve_id, Notification, NotificationId, NotifyError, PushToken};

const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";

/// The `app_name` reported to the daemon (shown in some UIs / used for
/// grouping). A fixed identifier — the SDK doesn't know the host app's name.
const APP_NAME: &str = "Idealyst";

/// `expire_timeout = -1` = let the server pick the default lifetime. (0 would
/// mean "never expire"; a positive value is milliseconds.)
const EXPIRE_DEFAULT: i32 = -1;

/// Maps our stable string id to the daemon's last server-assigned `u32` for
/// that id. Enables replace-on-repost and close-by-id. Lazily initialized
/// (the default-hasher `HashMap::new()` isn't a const fn).
static REGISTRY: LazyLock<Mutex<HashMap<NotificationId, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn map_zbus(e: zbus::Error) -> NotifyError {
    NotifyError::Backend(format!("D-Bus: {e}"))
}

/// The `replaces_id` to send for `id`: the previously-returned server id if we
/// have one (→ replace the on-screen notification), else `0` (→ new post).
fn replaces_id(id: &NotificationId) -> u32 {
    REGISTRY
        .lock()
        .expect("notification registry poisoned")
        .get(id)
        .copied()
        .unwrap_or(0)
}

fn remember(id: &NotificationId, server_id: u32) {
    REGISTRY
        .lock()
        .expect("notification registry poisoned")
        .insert(id.clone(), server_id);
}

/// The pure, bus-free part of building a `Notify` call: the summary/body
/// (with subtitle folded in) and the custom hints derived from the payload.
/// Separated so it's unit-testable without a live daemon.
struct NotifyArgs {
    summary: String,
    body: String,
    /// Custom hints as `(key, value)` string pairs, sorted for determinism.
    /// The `data` map has no standard freedesktop hint, so each entry is
    /// carried under an `x-idealyst-<key>` custom hint (the spec's convention
    /// for app-private hints; unknown hints are ignored by the daemon).
    hints: Vec<(String, String)>,
    expire_timeout: i32,
}

fn build_args(n: &Notification) -> NotifyArgs {
    // The freedesktop spec has no subtitle field; fold it into the body's
    // first line where present (crate-wide "no distinct field" behavior).
    let body = match &n.subtitle {
        Some(sub) if !sub.is_empty() => format!("{sub}\n{}", n.body),
        _ => n.body.clone(),
    };
    let mut hints: Vec<(String, String)> = n
        .data
        .iter()
        .map(|(k, v)| (format!("x-idealyst-{k}"), v.clone()))
        .collect();
    hints.sort();
    NotifyArgs {
        summary: n.title.clone(),
        body,
        hints,
        expire_timeout: EXPIRE_DEFAULT,
    }
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let args = build_args(&n);
    let replaces = replaces_id(&id);

    let conn = Connection::session().await.map_err(map_zbus)?;

    let hints: HashMap<&str, Value> = args
        .hints
        .iter()
        .map(|(k, v)| (k.as_str(), Value::from(v.as_str())))
        .collect();
    let actions: Vec<&str> = Vec::new();

    let reply = conn
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "Notify",
            &(
                APP_NAME,
                replaces,
                "", // app_icon — none; the daemon falls back to a default.
                args.summary.as_str(),
                args.body.as_str(),
                actions,
                hints,
                args.expire_timeout,
            ),
        )
        .await
        .map_err(map_zbus)?;

    let server_id: u32 = reply.body().deserialize().map_err(map_zbus)?;
    remember(&id, server_id);
    Ok(id)
}

pub(super) async fn schedule(
    _n: Notification,
    _after: std::time::Duration,
) -> Result<NotificationId, NotifyError> {
    // No native scheduling primitive in the freedesktop spec (see module
    // docs). Reporting NotSupported keeps the SDK honest rather than faking a
    // process-lifetime timer.
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(id: &NotificationId) {
    let server_id = REGISTRY
        .lock()
        .expect("notification registry poisoned")
        .remove(id);
    if let Some(server_id) = server_id {
        // Best-effort, mirroring the other backends' infallible `cancel`.
        let _ = close(server_id).await;
    }
}

pub(super) async fn cancel_all() {
    // Drain the registry, then close each tracked id (no CloseAll in the spec).
    let ids: Vec<u32> = {
        let mut reg = REGISTRY.lock().expect("notification registry poisoned");
        reg.drain().map(|(_, v)| v).collect()
    };
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(_) => return,
    };
    for server_id in ids {
        let _ = conn
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                "CloseNotification",
                &(server_id,),
            )
            .await;
    }
}

/// Close a single server-assigned notification id via `CloseNotification`.
async fn close(server_id: u32) -> Result<(), NotifyError> {
    let conn = Connection::session().await.map_err(map_zbus)?;
    conn.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some(INTERFACE),
        "CloseNotification",
        &(server_id,),
    )
    .await
    .map_err(map_zbus)?;
    Ok(())
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // No remote-push token model on a Linux desktop (see module docs).
    Err(NotifyError::NotSupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;

    #[test]
    fn build_args_summary_and_default_timeout() {
        let n = Notification::new("Title", "Body");
        let args = build_args(&n);
        assert_eq!(args.summary, "Title");
        assert_eq!(args.body, "Body");
        assert_eq!(args.expire_timeout, EXPIRE_DEFAULT);
        assert!(args.hints.is_empty());
    }

    #[test]
    fn build_args_folds_subtitle_into_body() {
        let n = Notification::new("Title", "Body").subtitle("Sub");
        let args = build_args(&n);
        // No freedesktop subtitle field → subtitle becomes the body's first
        // line.
        assert_eq!(args.body, "Sub\nBody");
        // An empty subtitle must NOT prepend a blank line.
        let mut n2 = Notification::new("T", "B");
        n2.subtitle = Some(String::new());
        assert_eq!(build_args(&n2).body, "B");
    }

    #[test]
    fn build_args_maps_data_to_prefixed_hints() {
        let mut data = StdHashMap::new();
        data.insert("route".to_string(), "/inbox".to_string());
        data.insert("id".to_string(), "42".to_string());
        let n = Notification::new("T", "B").data(data);
        let args = build_args(&n);
        // Sorted + prefixed so the payload rides as app-private hints.
        assert_eq!(
            args.hints,
            vec![
                ("x-idealyst-id".to_string(), "42".to_string()),
                ("x-idealyst-route".to_string(), "/inbox".to_string()),
            ]
        );
    }

    #[test]
    fn map_zbus_produces_backend_error() {
        let err = map_zbus(zbus::Error::Failure("boom".to_string()));
        match err {
            NotifyError::Backend(msg) => assert!(msg.contains("boom"), "got {msg}"),
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn replaces_id_defaults_zero_then_tracks() {
        let id = NotificationId("linux-test-replaces".into());
        // Unknown id → 0 (a fresh post).
        assert_eq!(replaces_id(&id), 0);
        remember(&id, 7);
        // Known id → the server id (a replace).
        assert_eq!(replaces_id(&id), 7);
        // Cleanup so the global registry doesn't leak into other tests.
        REGISTRY
            .lock()
            .expect("notification registry poisoned")
            .remove(&id);
    }

    /// Live post against the running notification daemon. Ignored because it
    /// needs a session bus with an `org.freedesktop.Notifications` owner (a
    /// desktop session), which a headless CI runner doesn't have. Run on a
    /// desktop with: `cargo test -p notifications --target
    /// x86_64-unknown-linux-gnu -- --ignored live_notify_posts`.
    #[ignore = "needs a live session bus + notification daemon"]
    #[tokio::test]
    async fn live_notify_posts() {
        let id = notify(Notification::new("Idealyst", "Live D-Bus test").id("idealyst-live"))
            .await
            .expect("notify should reach the daemon");
        assert_eq!(id, NotificationId("idealyst-live".into()));
        // Re-post under the same id must replace (non-zero replaces_id now).
        assert_eq!(replaces_id(&id), replaces_id(&id));
        cancel(&id).await;
    }
}
