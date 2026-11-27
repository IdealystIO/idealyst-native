//! Web local notifications and their responses.
//!
//! Two ways to show a notification, picked per post:
//!
//! - **Through the app's service worker** (`registration.showNotification`)
//!   when the page has an active one — the web build composes it from
//!   every dependency's handlers, this crate's `sw/notifications.js`
//!   included, and the boot registers it (`docs/service-worker.md`). This
//!   is the path with action buttons, clicks that still arrive after the
//!   tab closed, and dismissals; it is also the only path on Chrome for
//!   Android, where `new Notification` throws.
//! - **The page-level `new Notification(title, options)`** otherwise (an
//!   insecure origin, an app without the worker, the moment before the
//!   worker first activates). No buttons, no dismissals, and the click only
//!   while the page lives.
//!
//! Options come from the shared fields either way — `body` (subtitle folded
//! on; web has no subtitle field), `tag` = the resolved id (the browser
//! coalesces by tag, so re-posting the same id *replaces*), `renotify`
//! (without it a same-tag replacement is swapped in silently — no banner, no
//! sound — where every native platform alerts again), `silent` for
//! [`Sound::Silent`], `requireInteraction` for [`Priority::Urgent`],
//! `image`. The worker path adds `actions` and the `data` its handlers read
//! back (the id, the payload, and which actions must not open the app). The
//! app's [`Notification::web`] hook edits the finished options object.
//!
//! ## Responses
//!
//! - **Worker path:** `sw/notifications.js` handles `notificationclick`
//!   (focusing an open tab, or opening the app, unless the pressed action
//!   has `opens_app(false)`) and `notificationclose` (a user dismissal —
//!   the browser does not fire it for `close()` or for a notification that
//!   merely leaves the screen), and delivers `{ id, action, payload }` on
//!   the "notifications" channel. [`init`] subscribes to it
//!   (`web_glue::service_worker::on_message`); the channel holds a response
//!   until the page that will handle it is listening, so a click with no
//!   tab open reaches the tab it opens.
//! - **Page path:** the notification's `click` focuses the page, closes the
//!   notification and reports [`ResponseAction::Open`] through one
//!   page-lifetime callback.
//!
//! The shown page-path notifications are kept in a page-wide map by tag,
//! and the worker's are found through `registration.getNotifications`, so
//! `cancel` / `cancel_all` close both.
//!
//! Still not on web: a reply field (the web has no text-input action),
//! `schedule` (no delayed-delivery API; `NotSupported`), `group` (no
//! grouping concept; dropped). Action buttons show where the browser
//! supports them (Chrome, Edge); others show the notification without them.
//!
//! Authorization is **not** requested here — the public `authorize()` goes
//! through the `permissions` crate, which on web calls
//! `Notification.requestPermission()`.
//!
//! ## Web-push token
//!
//! [`push_token`] needs a VAPID key the app supplies; until that lands it
//! reports `NotSupported`.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use runtime_core::__serde_json::{self as serde_json, json, Map, Value};
use web_glue::service_worker::MessageSubscription;
use web_glue::{string, Closure, JsFuture, JsValue};

use crate::{
    body_with_subtitle, deliver_response, resolve_id, Notification as Note, NotificationId, NotifyError, Priority,
    PushToken, ResponseAction, Sound,
};

/// The channel `sw/notifications.js` delivers responses on.
const CHANNEL: &str = "notifications";
/// How long a post waits for a registered worker that is still activating
/// before falling back to the page-level API. Covers the first load, where
/// the boot registers the worker and the app posts straight away.
const ACTIVATION_WAIT_MS: u32 = 3000;

web_glue::import! {
    // Show through the active service worker. Resolves `true` when shown,
    // `false` when the page has no worker (or it doesn't activate within
    // `wait` ms) — the caller then uses the page-level API; rejects when the
    // browser refuses (no permission). The strings are read up front: they
    // point into wasm memory that is only valid for this call.
    #[catch]
    fn js_show_via_worker(tp: usize, tl: usize, op: usize, ol: usize, wait: u32) -> u32 =
        "(tp, tl, op, ol, wait) => { const title = G.str(tp, tl); const options = JSON.parse(G.str(op, ol)); \
           return G.add((async () => { \
             const c = globalThis.navigator && navigator.serviceWorker; if (!c) return false; \
             const reg = await c.getRegistration(); if (!reg) return false; \
             const active = reg.active ? reg : await Promise.race([c.ready, \
               new Promise((r) => setTimeout(() => r(null), wait))]); \
             if (!active) return false; \
             await active.showNotification(title, options); return true; })()); }";
    // `new Notification(title, options)`, kept in the page-wide map by tag
    // (replacing — and closing — the previous one under that tag), with a
    // click handler that focuses the page, closes it and calls `cb` with
    // the tag. Throws (→ Err) without the Notification API, or where
    // construction is disallowed (Chrome on Android).
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
    // Close what is shown under `tag` (every tag when `all`): page-path
    // notifications from the map, worker-path ones from the registration.
    fn js_close(gp: usize, gl: usize, all: u32) =
        "(gp, gl, all) => { const tag = G.str(gp, gl); const notes = globalThis.__idealystNotes; \
           if (notes) { for (const [t, n] of [...notes]) if (all || t === tag) { notes.delete(t); n.close(); } } \
           const c = globalThis.navigator && navigator.serviceWorker; if (!c) return; \
           c.getRegistration().then((reg) => reg && reg.getNotifications(all ? {} : { tag })) \
             .then((list) => { for (const n of list || []) if (n.data && n.data.idealyst) n.close(); }) \
             .catch(() => {}); }";
    // `JSON.stringify(v)` into `out` (`{}` for undefined / null).
    fn js_stringify(v: u32, out: usize) = "(v, o) => { G.retStr(JSON.stringify(G.get(v) ?? {}), o); }";
}

thread_local! {
    /// The page-path click callback every page-level notification calls
    /// with its tag. Lives for the page: a notification shown at any point
    /// may be clicked later.
    static ON_CLICK: OnceCell<Closure> = const { OnceCell::new() };
    /// The worker channel subscription, for the page's lifetime.
    static RESPONSES: OnceCell<MessageSubscription> = const { OnceCell::new() };
    /// The payload of each page-path notification still on screen, by id —
    /// handed back with its click.
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
    if web_glue::service_worker::is_supported() {
        RESPONSES.with(|cell| {
            cell.get_or_init(|| web_glue::service_worker::on_message(CHANNEL, on_worker_response));
        });
    }
}

/// A `{ id, action, payload }` from `sw/notifications.js`.
fn on_worker_response(m: JsValue) {
    let Some(id) = m.get("id").ok().and_then(|v| v.as_string()) else { return };
    let action = parse_action(&m.get("action").ok().and_then(|v| v.as_string()).unwrap_or_default());
    let payload = m.get("payload").unwrap_or(JsValue::UNDEFINED);
    let data = payload_map(&payload);
    deliver_response(NotificationId(id), action, None, data);
}

/// The handler's action encoding as a [`ResponseAction`].
pub(crate) fn parse_action(action: &str) -> ResponseAction {
    match action {
        "dismiss" => ResponseAction::Dismiss,
        other => match other.strip_prefix("action:") {
            Some(id) => ResponseAction::Action(id.to_string()),
            None => ResponseAction::Open,
        },
    }
}

/// The string entries of a plain JS object (the posted payload, cloned back).
fn payload_map(payload: &JsValue) -> HashMap<String, String> {
    let json = string::receive(|o| unsafe { js_stringify(payload.raw(), o) });
    serde_json::from_str::<HashMap<String, Value>>(&json)
        .map(|m| m.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_string()))).collect())
        .unwrap_or_default()
}

fn click_handle() -> u32 {
    init();
    ON_CLICK.with(|cell| cell.get().expect("init installs the click callback").as_js().raw())
}

/// The options for `n` posted under `id`, after the app's hook. `worker`
/// adds what only a worker-shown notification takes: `actions` (the page
/// constructor throws on them) and the `data` the worker's handlers read.
pub(crate) fn options(n: &Note, id: &NotificationId, worker: bool) -> Map<String, Value> {
    let mut o = Map::new();
    o.insert("body".into(), json!(body_with_subtitle(n)));
    o.insert("tag".into(), json!(id.as_str()));
    // A replacement alerts again, as on every native platform; the web
    // default swaps it in silently. Valid because `tag` is always set.
    o.insert("renotify".into(), json!(true));
    if n.sound == Sound::Silent {
        o.insert("silent".into(), json!(true));
    }
    if n.priority == Priority::Urgent {
        o.insert("requireInteraction".into(), json!(true));
    }
    if let Some(image) = &n.image {
        o.insert("image".into(), json!(image));
    }
    if worker {
        if !n.actions.is_empty() {
            let actions: Vec<Value> =
                n.actions.iter().map(|a| json!({ "action": a.id, "title": a.title })).collect();
            o.insert("actions".into(), Value::Array(actions));
        }
        let background: Vec<&str> = n.actions.iter().filter(|a| !a.opens_app).map(|a| a.id.as_str()).collect();
        o.insert(
            "data".into(),
            json!({ "idealyst": 1, "id": id.as_str(), "payload": n.data, "background": background }),
        );
    }
    if let Some(hook) = &n.hooks.web {
        hook(&mut o);
    }
    o
}

fn to_json(o: Map<String, Value>) -> Result<String, NotifyError> {
    serde_json::to_string(&Value::Object(o)).map_err(|e| NotifyError::Backend(format!("options: {e}")))
}

pub(super) async fn notify(n: Note) -> Result<NotificationId, NotifyError> {
    init();
    let id = resolve_id(&n);

    let worker_options = to_json(options(&n, &id, true))?;
    let shown = {
        let (tp, tl) = string::abi(&n.title);
        let (op, ol) = string::abi(&worker_options);
        let promise = unsafe { js_show_via_worker(tp, tl, op, ol, ACTIVATION_WAIT_MS) }
            .map_err(|e| NotifyError::Backend(e.message()))?;
        JsFuture::new(&unsafe { JsValue::from_raw(promise) })
            .await
            .map_err(|e| NotifyError::Backend(e.message()))?
    };
    if shown.as_bool() == Some(true) {
        return Ok(id);
    }

    let page_options = to_json(options(&n, &id, false))?;
    let cb = click_handle();
    let (tp, tl) = string::abi(&n.title);
    let (op, ol) = string::abi(&page_options);
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
    unsafe { js_close(gp, gl, 0) }
}

pub(super) async fn cancel_all() {
    PAYLOADS.with(|p| p.borrow_mut().clear());
    let (gp, gl) = string::abi("");
    unsafe { js_close(gp, gl, 1) }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // Web-push needs a VAPID key the app owns (see module docs).
    Err(NotifyError::NotSupported)
}
