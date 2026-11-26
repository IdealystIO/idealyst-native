//! The page's side of the app's service worker.
//!
//! The web build composes one worker, `idealyst-sw.js`, from the handler
//! scripts the app's dependencies contribute (`build_web::service_worker`)
//! and marks the page; the web boot then calls [`register`]. A browser lets
//! one worker control a scope, which is why there is one composed worker
//! rather than one per SDK.
//!
//! # The channel
//!
//! Every composed worker starts with [`PRELUDE`], the worker half of a
//! small protocol; this module is the page half. A contribution calls
//! `self.idealyst.deliver(channel, message)`, and the page's
//! [`on_message`] handler for `channel` receives it. [`post`] sends the
//! other way, to the contribution's `self.idealyst.on(channel, …)`.
//!
//! A message must survive the gaps between the two: the worker is stopped
//! whenever it idles, and the page it is for may not exist yet (a
//! notification clicked with no tab open opens one, which takes a while to
//! boot). So `deliver` writes the message to an IndexedDB outbox, and it
//! only leaves when a page has said it is listening: [`listen`] installs
//! the page's listener and then sends `ready`; a worker with mail but no
//! known listener (it forgets them when it stops) sends `ping`, which a
//! listening page answers with `ready`. On the page, a message for a
//! channel nobody handles yet is held until the first [`on_message`] for
//! it, so a handler an SDK installs after boot still gets what arrived
//! during boot. Each message is delivered once, to one page.
//!
//! Registration is `navigator.serviceWorker.register(url, { updateViaCache:
//! "none" })`: the browser always re-fetches the worker script itself when
//! checking for an update instead of answering from the HTTP cache, so a
//! deploy (or a dev rebuild) that changes a handler takes effect on the
//! next check rather than after the cache expires. The composed worker
//! takes over from an old version immediately (its prelude calls
//! `skipWaiting` + `clients.claim`).

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::{Closure, JsError, JsFuture, JsValue};

/// The worker half of the channel: the start of every composed
/// `idealyst-sw.js` (`build_web::service_worker` writes it ahead of the
/// contributions).
pub const PRELUDE: &str = include_str!("../js/service_worker.js");

crate::import! {
    // Install the page's listener: `ping` → answer `ready` to the worker;
    // `message` → `cb({ channel, message })`. Then say `ready` — now, and
    // to every worker that later takes control. `startMessages` releases
    // messages the browser queued for this page until a listener existed.
    fn js_listen(cb: u32) =
        "(cb) => { const c = navigator.serviceWorker; const f = G.get(cb); \
           c.addEventListener('message', (e) => { const d = e.data; \
             if (!d || typeof d.idealyst !== 'string') return; \
             if (d.idealyst === 'ping') { if (e.source) e.source.postMessage({ idealyst: 'ready' }); } \
             else if (d.idealyst === 'message') f({ channel: d.channel, message: d.message }); }); \
           const hello = () => c.ready.then((r) => { if (r.active) r.active.postMessage({ idealyst: 'ready' }); }); \
           c.addEventListener('controllerchange', hello); \
           if (c.startMessages) c.startMessages(); \
           hello(); }";
    fn js_post(p: usize, l: usize, message: u32) =
        "(p, l, m) => { const channel = G.str(p, l); const message = G.get(m); \
           navigator.serviceWorker.ready.then((r) => { \
             if (r.active) r.active.postMessage({ idealyst: 'message', channel, message }); }); }";
    fn js_supported() -> u32 =
        "() => (typeof navigator !== 'undefined' && navigator.serviceWorker != null) ? 1 : 0";
    #[catch]
    fn js_register(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(navigator.serviceWorker.register(G.str(p, l), { updateViaCache: 'none' }))";
}

/// Whether this page can register a service worker: the API exists, which
/// needs a secure context (HTTPS, or `localhost`).
pub fn is_supported() -> bool {
    unsafe { js_supported() != 0 }
}

/// Register the service worker script at `url` (relative to the page) and
/// wait for the browser to accept it. Errors when the page cannot have one
/// ([`is_supported`]), when the script cannot be fetched, or when it throws
/// while being evaluated.
pub async fn register(url: &str) -> Result<(), JsError> {
    if !is_supported() {
        return Err(JsError::from(JsValue::from_str(
            "service workers are unavailable here (no navigator.serviceWorker — an insecure context?)",
        )));
    }
    let (p, l) = crate::string::abi(url);
    let promise = unsafe { JsValue::from_raw(js_register(p, l)?) };
    JsFuture::new(&promise).await.map(|_| ())
}

type Handler = Rc<dyn Fn(JsValue)>;

#[derive(Default)]
struct Channels {
    handlers: HashMap<String, Vec<(u64, Handler)>>,
    /// Messages for channels with no handler yet, oldest first.
    held: HashMap<String, Vec<JsValue>>,
    next_id: u64,
}

thread_local! {
    /// The page's message listener. Lives for the page: the worker may
    /// deliver at any point.
    static LISTENER: OnceCell<Closure> = const { OnceCell::new() };
    static CHANNELS: RefCell<Channels> = RefCell::new(Channels::default());
}

/// Install the page's listener and tell the worker this page is ready for
/// its messages. Idempotent; a no-op where the page cannot have a service
/// worker. [`on_message`] calls it; the web boot calls it when the build
/// composed a worker.
pub fn listen() {
    if !is_supported() {
        return;
    }
    LISTENER.with(|cell| {
        if cell.get().is_some() {
            return;
        }
        let closure = Closure::new_fn(|event: JsValue| {
            let Some(channel) = event.get("channel").ok().and_then(|c| c.as_string()) else { return };
            let message = event.get("message").unwrap_or(JsValue::UNDEFINED);
            dispatch(channel, message);
        });
        unsafe { js_listen(closure.as_js().raw()) };
        let _ = cell.set(closure);
    });
}

fn dispatch(channel: String, message: JsValue) {
    let handlers: Vec<Handler> = CHANNELS.with(|c| {
        let mut c = c.borrow_mut();
        match c.handlers.get(&channel) {
            Some(list) if !list.is_empty() => list.iter().map(|(_, h)| h.clone()).collect(),
            _ => {
                c.held.entry(channel).or_default().push(message.clone());
                Vec::new()
            }
        }
    });
    for h in handlers {
        h(message.clone());
    }
}

/// Run `handler` for every message the worker delivers on `channel`
/// (`self.idealyst.deliver(channel, …)`), including those held because they
/// arrived before any handler — those are handed over on a microtask, never
/// inside this call. Every handler for a channel sees every message.
/// Dropping the returned subscription unregisters the handler.
pub fn on_message(channel: &str, handler: impl Fn(JsValue) + 'static) -> MessageSubscription {
    listen();
    let id = CHANNELS.with(|c| {
        let mut c = c.borrow_mut();
        let id = c.next_id;
        c.next_id += 1;
        c.handlers.entry(channel.to_string()).or_default().push((id, Rc::new(handler)));
        id
    });
    let owned = channel.to_string();
    crate::queue_microtask(move || {
        let held = CHANNELS.with(|c| c.borrow_mut().held.remove(&owned)).unwrap_or_default();
        for message in held {
            dispatch(owned.clone(), message);
        }
    });
    MessageSubscription { channel: channel.to_string(), id }
}

/// Send `message` (structured-cloned) to the worker's
/// `self.idealyst.on(channel, …)` handler, once the worker is active. Dropped
/// where the page has no service worker.
pub fn post(channel: &str, message: &JsValue) {
    if !is_supported() {
        return;
    }
    let (p, l) = crate::string::abi(channel);
    unsafe { js_post(p, l, message.raw()) }
}

/// The registration [`on_message`] returns; dropping it unregisters the
/// handler.
#[must_use = "dropping the subscription immediately unregisters the handler"]
pub struct MessageSubscription {
    channel: String,
    id: u64,
}

impl Drop for MessageSubscription {
    fn drop(&mut self) {
        let _ = CHANNELS.try_with(|c| {
            if let Some(list) = c.borrow_mut().handlers.get_mut(&self.channel) {
                list.retain(|(id, _)| *id != self.id);
            }
        });
    }
}
