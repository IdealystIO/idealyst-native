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
//!
//! The browser call is a web-glue binding declared here (own-web-bindings
//! phase 3).

use web_glue::string;

use crate::{resolve_id, Notification as Note, NotificationId, NotifyError, PushToken};

web_glue::import! {
    // `new Notification(title, { body, tag })`. The instance is not kept:
    // re-posting the tag replaces it (see the module docs). Throws (→ Err)
    // without the Notification API, or where construction is disallowed
    // (Chrome on Android requires a service worker registration).
    #[catch]
    fn js_notify(tp: usize, tl: usize, bp: usize, bl: usize, gp: usize, gl: usize) =
        "(tp, tl, bp, bl, gp, gl) => { \
           new Notification(G.str(tp, tl), { body: G.str(bp, bl), tag: G.str(gp, gl) }); }";
}

pub(super) async fn notify(n: Note) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);

    // Fold subtitle onto the body — web Notification has no subtitle field.
    let body = match &n.subtitle {
        Some(sub) if !sub.is_empty() => format!("{sub}\n{}", n.body),
        _ => n.body.clone(),
    };

    // `tag` coalesces: a later notification with the same tag replaces this
    // one, giving the same update-by-id behavior the native backends have.
    let (tp, tl) = string::abi(&n.title);
    let (bp, bl) = string::abi(&body);
    let (gp, gl) = string::abi(id.as_str());
    unsafe { js_notify(tp, tl, bp, bl, gp, gl) }
        .map(|()| id)
        .map_err(|e| NotifyError::Backend(e.message()))
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
