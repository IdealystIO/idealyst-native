//! Web local notifications via the `Notification` Web API.
//!
//! **Runnable on web for immediate `notify`.** Mechanism:
//! - `notify` constructs `new Notification(title, { body })`. The
//!   subtitle (web has no distinct field) is folded onto the body. The
//!   `tag` option is set to the resolved id so re-posting the same id
//!   *replaces* the existing notification (the browser coalesces by tag) —
//!   matching the update semantics of the native backends.
//! - `cancel` / `cancel_all`: the `Notification` API has no general
//!   "dismiss by tag" once shown without holding the handle, so these are
//!   best-effort no-ops on web (a re-post with the same tag replaces; the
//!   user dismisses from the OS shade). Documented, not faked.
//! - `schedule` has **no native web API** — there is no delayed-delivery
//!   primitive — so it returns `NotSupported`. (Schedule from a service
//!   worker / your server instead.)
//!
//! Authorization is **not** requested here — the public `authorize()` goes
//! through the `permissions` crate, which on web calls
//! `Notification.requestPermission()`. A `notify` before the grant is
//! dropped by the browser, so callers gate on `authorize()`.
//!
//! ## Web-push token = service-worker seam
//!
//! [`push_token`] for the web is a web-push `PushSubscription`
//! (`registration.pushManager.subscribe({ applicationServerKey })`). It
//! requires a **registered service worker** and a VAPID application server
//! key the app owns — neither of which a library can synthesize. Until the
//! host registers a service worker, we report `NotSupported`. When a SW is
//! present this is where `subscribe(...)` + `JSON.stringify(subscription)`
//! would slot in; the seam is structured (we probe for the SW registration)
//! but delivery + the VAPID key stay app-owned.

use wasm_bindgen::JsValue;
use web_sys::{Notification, NotificationOptions};

use crate::{resolve_id, Notification as Note, NotificationId, NotifyError, PushToken};

pub(super) async fn notify(n: Note) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);

    // Fold subtitle onto the body — web Notification has no subtitle field.
    let body = match &n.subtitle {
        Some(sub) if !sub.is_empty() => format!("{sub}\n{}", n.body),
        _ => n.body.clone(),
    };

    let opts = NotificationOptions::new();
    opts.set_body(&body);
    // `tag` coalesces: a later notification with the same tag replaces this
    // one, giving the same update-by-id behavior the native backends have.
    opts.set_tag(id.as_str());

    Notification::new_with_options(&n.title, &opts)
        .map(|_| id)
        .map_err(|e| NotifyError::Backend(js_err(&e)))
}

pub(super) async fn schedule(
    _n: Note,
    _after: std::time::Duration,
) -> Result<NotificationId, NotifyError> {
    // No native delayed-notification API on the web.
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(_id: &NotificationId) {
    // Best-effort no-op: a shown web Notification can't be dismissed by tag
    // without its handle (see module docs). Re-posting the tag replaces it.
}

pub(super) async fn cancel_all() {
    // Same as `cancel` — no general dismissal API. Documented no-op.
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // Web-push needs a registered service worker + a VAPID key the app
    // owns (host seam — see module docs). We probe for a SW registration so
    // the seam is honest; with none present there's no token to return.
    Err(NotifyError::NotSupported)
}

/// Render a `JsValue` error as a string for `NotifyError::Backend`.
fn js_err(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Object::from(e.clone()).to_string().as_string())
        .unwrap_or_else(|| "JS error".to_string())
}
