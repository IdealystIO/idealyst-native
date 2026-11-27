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
//! on; web has no subtitle field), `renotify`, `silent` for
//! [`Sound::Silent`], `requireInteraction` for [`Priority::Urgent`],
//! `image`. The worker path adds `actions` and the `data` its handlers read
//! back (the id, the payload, and which actions must not open the app). The
//! app's [`Notification::web`] hook edits the finished options object; the
//! tag is the SDK's, set after it (below).
//!
//! ## Replacing
//!
//! Re-posting an id must alert like a new notification, as on every native
//! platform, and replace the one shown. The browser coalesces by `tag`,
//! but a same-tag post is not a dependable alert even with `renotify`:
//! Brave/Chrome on macOS hand the tag to Notification Center as the
//! notification's identity, and a notification whose banner was swiped
//! away still sits there, so a post under its tag updates that entry
//! silently — no banner, no sound. So every post gets a fresh tag
//! (`<id>#<time>.<n>`), and the notifications already shown for the id —
//! found by the id in their `data` (worker path) or the page-wide map (page
//! path) — are closed **after** the new one is shown: closing the swiped
//! entry just before posting suppresses the new banner too (Brave on
//! macOS). The browser does not fire `notificationclose` for a `close()`
//! call, so nothing is reported as a dismissal.
//!
//! [`Notification::alert_once`] is the other way round: when one is shown
//! for the id, the post reuses its tag with `renotify: false`, which the
//! browser swaps in without alerting.
//!
//! ## Responses
//!
//! - **Worker path:** `sw/notifications.js` handles `notificationclick`
//!   (focusing an open tab, or opening the app, unless the pressed action
//!   has `opens_app(false)`) and `notificationclose` (a user dismissal —
//!   closing it, or clearing it from the notification center; not
//!   `close()`, and not a banner swiped away on macOS, which only moves it
//!   into Notification Center), and delivers `{ id, action, payload }` on
//!   the "notifications" channel. [`init`] subscribes to it
//!   (`web_glue::service_worker::on_message`); the channel holds a response
//!   until the page that will handle it is listening, so a click with no
//!   tab open reaches the tab it opens.
//! - **Page path:** the notification's `click` focuses the page, closes the
//!   notification and reports [`ResponseAction::Open`] through one
//!   page-lifetime callback.
//!
//! The shown page-path notifications are kept in a page-wide map by id, and
//! the worker's are found through `registration.getNotifications` by the id
//! in their `data`, so `cancel` / `cancel_all` close both.
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
    // browser refuses (no permission). The notifications already shown for
    // `id` are found by their `data.id`; the tag is picked as the module
    // docs' "Replacing" says. The strings are read up front: they point
    // into wasm memory that is only valid for this call.
    #[catch]
    fn js_show_via_worker(tp: usize, tl: usize, op: usize, ol: usize, ip: usize, il: usize, once: u32, wait: u32) -> u32 =
        "(tp, tl, op, ol, ip, il, once, wait) => { const title = G.str(tp, tl); \
           const options = JSON.parse(G.str(op, ol)); const id = G.str(ip, il); \
           return G.add((async () => { \
             const c = globalThis.navigator && navigator.serviceWorker; if (!c) return false; \
             const reg = await c.getRegistration(); if (!reg) return false; \
             const active = reg.active ? reg : await Promise.race([c.ready, \
               new Promise((r) => setTimeout(() => r(null), wait))]); \
             if (!active) return false; \
             const shown = (await active.getNotifications()) \
               .filter((n) => n.data && n.data.idealyst && n.data.id === id); \
             let keep = null; \
             if (once && shown.length) { keep = shown[0]; options.tag = keep.tag; options.renotify = false; } \
             else { const s = (globalThis.__idealystNoteSeq = (globalThis.__idealystNoteSeq || 0) + 1); \
               options.tag = id + '#' + Date.now().toString(36) + '.' + s; } \
             await active.showNotification(title, options); \
             for (const old of shown) if (old !== keep) old.close(); \
             return true; })()); }";
    // `new Notification(title, options)`, kept in the page-wide map by id,
    // with a click handler that focuses the page, closes it and calls `cb`
    // with the id. The tag is picked as for the worker path; the previous
    // notification for `id` is closed after the new one is shown, unless
    // `once` replaced it in place.
    // Throws (→ Err) without the Notification API, or where construction is
    // disallowed (Chrome on Android).
    #[catch]
    fn js_notify(tp: usize, tl: usize, op: usize, ol: usize, ip: usize, il: usize, once: u32, cb: u32) =
        "(tp, tl, op, ol, ip, il, once, cb) => { \
           const id = G.str(ip, il); const f = G.get(cb); const options = JSON.parse(G.str(op, ol)); \
           const notes = (globalThis.__idealystNotes ??= new Map()); \
           const old = notes.get(id); \
           if (once && old) { options.tag = old.tag; options.renotify = false; } \
           else { const s = (globalThis.__idealystNoteSeq = (globalThis.__idealystNoteSeq || 0) + 1); \
             options.tag = id + '#' + Date.now().toString(36) + '.' + s; } \
           const n = new Notification(G.str(tp, tl), options); \
           notes.set(id, n); \
           if (old) { old.onclick = null; if (!once) old.close(); } \
           n.onclose = () => { if (notes.get(id) === n) notes.delete(id); }; \
           n.onclick = () => { \
             try { globalThis.focus(); } catch (_) {} \
             if (notes.get(id) === n) notes.delete(id); \
             n.close(); f(id); }; }";
    // Close what is shown for `id` (everything when `all`): page-path
    // notifications from the map, worker-path ones from the registration
    // by their `data.id`.
    fn js_close(ip: usize, il: usize, all: u32) =
        "(ip, il, all) => { const id = G.str(ip, il); const notes = globalThis.__idealystNotes; \
           if (notes) { for (const [k, n] of [...notes]) if (all || k === id) { notes.delete(k); n.close(); } } \
           const c = globalThis.navigator && navigator.serviceWorker; if (!c) return; \
           c.getRegistration().then((reg) => reg && reg.getNotifications()) \
             .then((list) => { for (const n of list || []) \
               if (n.data && n.data.idealyst && (all || n.data.id === id)) n.close(); }) \
             .catch(() => {}); }";
    // `JSON.stringify(v)` into `out` (`{}` for undefined / null).
    fn js_stringify(v: u32, out: usize) = "(v, o) => { G.retStr(JSON.stringify(G.get(v) ?? {}), o); }";
}

thread_local! {
    /// The page-path click callback every page-level notification calls
    /// with its id. Lives for the page: a notification shown at any point
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
            Closure::new_fn(|id: JsValue| {
                let Some(id) = id.as_string() else { return };
                let data = PAYLOADS.with(|p| p.borrow_mut().remove(&id)).unwrap_or_default();
                deliver_response(NotificationId(id), ResponseAction::Open, None, data);
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
    // A placeholder tag: the post picks the real one (module docs'
    // "Replacing"). `renotify` makes a same-tag post alert again, as
    // on every native platform — the web default swaps it in silently —
    // and is only valid with a tag. An `alert_once` replacement turns it
    // off.
    o.insert("tag".into(), json!(id.as_str()));
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
        let (ip, il) = string::abi(id.as_str());
        let promise = unsafe { js_show_via_worker(tp, tl, op, ol, ip, il, n.alert_once as u32, ACTIVATION_WAIT_MS) }
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
    let (ip, il) = string::abi(id.as_str());
    unsafe { js_notify(tp, tl, op, ol, ip, il, n.alert_once as u32, cb) }.map_err(|e| NotifyError::Backend(e.message()))?;
    PAYLOADS.with(|p| p.borrow_mut().insert(id.0.clone(), n.data));
    Ok(id)
}

pub(super) async fn schedule(_n: Note, _after: std::time::Duration) -> Result<NotificationId, NotifyError> {
    // No native delayed-notification API on the web.
    Err(NotifyError::NotSupported)
}

pub(super) async fn cancel(id: &NotificationId) {
    PAYLOADS.with(|p| p.borrow_mut().remove(id.as_str()));
    let (ip, il) = string::abi(id.as_str());
    unsafe { js_close(ip, il, 0) }
}

pub(super) async fn cancel_all() {
    PAYLOADS.with(|p| p.borrow_mut().clear());
    let (ip, il) = string::abi("");
    unsafe { js_close(ip, il, 1) }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // Web-push needs a VAPID key the app owns (see module docs).
    Err(NotifyError::NotSupported)
}
