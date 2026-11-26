//! Web local notifications and their clicks via the `Notification` Web API.
//!
//! **Runnable on web for immediate `notify`.** Mechanism:
//! - `notify` constructs `new Notification(title, options)`. `options` is
//!   built from the shared fields — `body` (with the subtitle folded on;
//!   web has no subtitle field), `tag` = the resolved id (the browser
//!   coalesces by tag, so re-posting the same id *replaces*), `silent` for
//!   [`Sound::Silent`], `requireInteraction` for [`Priority::Urgent`],
//!   `image` — then handed to the app's [`Notification::web`] hook as a
//!   JSON object before the call.
//! - The shown `Notification` is kept in a page-wide map by tag, so
//!   `cancel` / `cancel_all` can `close()` it.
//! - A click focuses the page, closes the notification and reports
//!   [`ResponseAction::Open`] with the notification's payload. Clicks reach
//!   Rust through one page-lifetime callback installed by [`init`].
//!
//! What the page-level API cannot do, and is therefore not reported or
//! shown here:
//! - **Actions** — buttons exist only on notifications shown by a service
//!   worker (`registration.showNotification`), and their clicks are
//!   delivered to the worker, not the page. A notification's actions are
//!   dropped on web.
//! - **Dismissal** — browsers fire `close` for a user dismissal, but also
//!   when a notification expires off screen, so a `close` cannot be told
//!   apart from a timeout; `Dismiss` is not reported.
//! - **Responses after the page closed** — the click handler lives in the
//!   page.
//! - **`schedule`** — no delayed-delivery API; `NotSupported`.
//! - **`group`** — no grouping concept; dropped.
//!
//! Authorization is **not** requested here — the public `authorize()` goes
//! through the `permissions` crate, which on web calls
//! `Notification.requestPermission()`.
//!
//! ## Web-push token = service-worker seam
//!
//! [`push_token`] for the web is a web-push `PushSubscription`
//! (`registration.pushManager.subscribe({ applicationServerKey })`). It
//! requires a registered service worker and a VAPID key the app owns, so it
//! reports `NotSupported`.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use runtime_core::__serde_json::{self as serde_json, json, Map, Value};
use web_glue::{string, Closure, JsValue};

use crate::{
    body_with_subtitle, deliver_response, resolve_id, Notification as Note, NotificationId, NotifyError, Priority,
    PushToken, ResponseAction, Sound,
};

web_glue::import! {
    // `new Notification(title, options)`, kept in the page-wide map by tag
    // (replacing — and closing — the previous one under that tag), with a
    // click handler that focuses the page, closes it and calls `cb` with
    // the tag. Throws (→ Err) without the Notification API, or where
    // construction is disallowed (Chrome on Android requires a service
    // worker registration).
    #[catch]
    fn js_notify(tp: usize, tl: usize, op: usize, ol: usize, gp: usize, gl: usize, cb: u32) =
        "(tp, tl, op, ol, gp, gl, cb) => { \
           const tag = G.str(gp, gl); const f = G.get(cb); \
           const notes = (globalThis.__idealystNotes ??= new Map()); \
           const old = notes.get(tag); if (old) { notes.delete(tag); old.close(); } \
           const n = new Notification(G.str(tp, tl), JSON.parse(G.str(op, ol))); \
           notes.set(tag, n); \
           n.onclick = () => { \
             try { globalThis.focus(); } catch (_) {} \
             if (notes.get(tag) === n) notes.delete(tag); \
             n.close(); f(tag); }; }";
    // Close the notification shown under `tag`, if any.
    fn js_close(gp: usize, gl: usize) =
        "(gp, gl) => { const notes = globalThis.__idealystNotes; if (!notes) return; \
           const tag = G.str(gp, gl); const n = notes.get(tag); \
           if (n) { notes.delete(tag); n.close(); } }";
    // Close every notification this page showed.
    fn js_close_all() =
        "() => { const notes = globalThis.__idealystNotes; if (!notes) return; \
           for (const n of notes.values()) n.close(); notes.clear(); }";
}

thread_local! {
    /// The click callback every notification calls with its tag. Lives for
    /// the page: notifications shown at any point may be clicked later.
    static ON_CLICK: OnceCell<Closure> = const { OnceCell::new() };
    /// The payload of each notification still on screen, by id — handed
    /// back with its click.
    static PAYLOADS: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
}

pub(super) fn init() {
    ON_CLICK.with(|cell| {
        cell.get_or_init(|| {
            Closure::new_fn(|tag: JsValue| {
                let Some(tag) = tag.as_string() else { return };
                let data = PAYLOADS.with(|p| p.borrow_mut().remove(&tag)).unwrap_or_default();
                deliver_response(NotificationId(tag), ResponseAction::Open, None, data);
            })
        });
    });
}

fn click_handle() -> u32 {
    init();
    ON_CLICK.with(|cell| cell.get().expect("init installs the click callback").as_js().raw())
}

/// The `NotificationOptions` for `n` posted under `id`, after the app's
/// hook.
pub(crate) fn options(n: &Note, id: &NotificationId) -> Map<String, Value> {
    let mut o = Map::new();
    o.insert("body".into(), json!(body_with_subtitle(n)));
    o.insert("tag".into(), json!(id.as_str()));
    if n.sound == Sound::Silent {
        o.insert("silent".into(), json!(true));
    }
    if n.priority == Priority::Urgent {
        o.insert("requireInteraction".into(), json!(true));
    }
    if let Some(image) = &n.image {
        o.insert("image".into(), json!(image));
    }
    if let Some(hook) = &n.hooks.web {
        hook(&mut o);
    }
    o
}

pub(super) async fn notify(n: Note) -> Result<NotificationId, NotifyError> {
    let id = resolve_id(&n);
    let options = serde_json::to_string(&Value::Object(options(&n, &id)))
        .map_err(|e| NotifyError::Backend(format!("options: {e}")))?;
    let cb = click_handle();
    let (tp, tl) = string::abi(&n.title);
    let (op, ol) = string::abi(&options);
    let (gp, gl) = string::abi(id.as_str());
    unsafe { js_notify(tp, tl, op, ol, gp, gl, cb) }.map_err(|e| NotifyError::Backend(e.message()))?;
    PAYLOADS.with(|p| p.borrow_mut().insert(id.0.clone(), n.data));
    Ok(id)
}

pub(super) async fn schedule(_n: Note, _after: std::time::Duration) -> Result<NotificationId, NotifyError> {
    // No native delayed-notification API on the web.
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(id: &NotificationId) {
    PAYLOADS.with(|p| p.borrow_mut().remove(id.as_str()));
    let (gp, gl) = string::abi(id.as_str());
    unsafe { js_close(gp, gl) }
}

pub(super) async fn cancel_all() {
    PAYLOADS.with(|p| p.borrow_mut().clear());
    unsafe { js_close_all() }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // Web-push needs a registered service worker + a VAPID key the app
    // owns (host seam — see module docs).
    Err(NotifyError::NotSupported)
}
