//! The web backend's service-worker path, in a real browser.
//!
//! A real worker needs a script served from this origin, so the worker runs
//! in the page: the channel prelude (`web_glue::service_worker::PRELUDE`)
//! and this crate's `sw/notifications.js` — the exact text the web build
//! composes into `idealyst-sw.js` — evaluated against a stand-in worker
//! scope (an `EventTarget` with the page's real IndexedDB and fake
//! `clients`), behind a stand-in `navigator.serviceWorker` whose
//! registration records `showNotification`. A click is a `notificationclick`
//! dispatched into that scope, as the browser does. Its own test binary (so
//! its own page): it replaces `navigator.serviceWorker` for good.
//!
//! Run with `cargo test -p notifications --target wasm32-unknown-unknown`.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use notifications::{
    cancel, notify, on_response, Action, Notification, NotificationId, NotificationResponse, ResponseAction,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

const HANDLERS: &str = include_str!("../sw/notifications.js");

fn run(body: &str) {
    let f = JsValue::global().get("Function").unwrap().construct(&[&JsValue::from_str(body)]).unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

fn eval(expr: &str) -> String {
    run(&format!("globalThis.__out = String({expr});"));
    JsValue::global().get("__out").unwrap().as_string().unwrap()
}

async fn settle() {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str("return new Promise((r) => setTimeout(r, 60));")])
        .unwrap();
    JsFuture::new(&f.call(&JsValue::undefined(), &[]).unwrap()).await.unwrap();
}

/// Install the stand-ins once, then (re)start the worker with an empty
/// outbox and no recorded calls. `page_open` is whether `clients.matchAll`
/// finds this page.
async fn fresh_worker(page_open: bool) {
    let g = JsValue::global();
    g.set("__prelude", &JsValue::from_str(web_glue::service_worker::PRELUDE)).unwrap();
    g.set("__handlers", &JsValue::from_str(HANDLERS)).unwrap();
    run(&format!(
        r#"
        if (!globalThis.__sw) {{
          const sw = (globalThis.__sw = {{}});
          const container = new EventTarget();
          sw.worker = {{ postMessage: (data) => queueMicrotask(() => {{
            const e = new Event("message");
            Object.defineProperty(e, "data", {{ value: data }});
            Object.defineProperty(e, "source", {{ value: sw.page }});
            e.waitUntil = (p) => p.catch((err) => console.error("waitUntil", err));
            sw.self.dispatchEvent(e);
          }}) }};
          sw.page = {{ id: "page-1", focused: true, visibilityState: "visible",
            focus: async () => {{ sw.calls.push("focus"); return sw.page; }},
            postMessage: (data) => queueMicrotask(() => {{
              const e = new MessageEvent("message", {{ data }});
              Object.defineProperty(e, "source", {{ value: sw.worker }});
              container.dispatchEvent(e);
            }}) }};
          // The registration: `showNotification` keeps what it was given, as
          // the browser's notification list does.
          sw.registration = {{
            active: sw.worker,
            scope: "http://127.0.0.1/app/",
            showNotification: async (title, options) => {{
              sw.shown = sw.shown.filter((n) => n.tag !== options.tag);
              const n = {{ title, ...options, close() {{ n.closed = true; sw.calls.push("close:" + n.tag); }} }};
              sw.shown.push(n);
            }},
            getNotifications: async (filter) =>
              sw.shown.filter((n) => !n.closed && (!filter || !filter.tag || n.tag === filter.tag)),
          }};
          container.ready = Promise.resolve(sw.registration);
          container.controller = sw.worker;
          container.getRegistration = async () => sw.registration;
          container.startMessages = () => {{}};
          sw.container = container;
          Object.defineProperty(navigator, "serviceWorker", {{ value: container, configurable: true }});
          // The page-level API must not be used while a worker is active.
          globalThis.Notification = class {{ constructor() {{ throw new Error("page-level Notification used"); }} }};
        }}
        const sw = globalThis.__sw;
        sw.shown = [];
        sw.calls = [];
        sw.pageOpen = {page_open};
        const self = new EventTarget();
        self.indexedDB = indexedDB;
        self.skipWaiting = () => {{}};
        self.registration = sw.registration;
        self.clients = {{
          matchAll: async () => (sw.pageOpen ? [sw.page] : []),
          openWindow: async (url) => {{ sw.calls.push("open:" + url); return null; }},
          claim: async () => {{}},
        }};
        sw.self = self;
        new Function("self", globalThis.__prelude)(self);
        new Function("self", globalThis.__handlers)(self);
        // What the browser does when the user acts on a shown notification.
        sw.user = (type, tag, action) => {{
          const notification = sw.shown.find((n) => n.tag === tag);
          const e = new Event(type);
          Object.defineProperty(e, "notification", {{ value: notification }});
          Object.defineProperty(e, "action", {{ value: action || "" }});
          e.waitUntil = (p) => p.catch((err) => console.error("waitUntil", err));
          self.dispatchEvent(e);
        }};
        "#
    ));
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(
            "return new Promise((r) => { const q = indexedDB.deleteDatabase('idealyst-sw'); \
             q.onsuccess = q.onerror = q.onblocked = () => r(); });",
        )])
        .unwrap();
    JsFuture::new(&f.call(&JsValue::undefined(), &[]).unwrap()).await.unwrap();
}

fn chat() -> Notification {
    Notification::new("Sam", "Lunch?")
        .id("chat-sam")
        .with("chat", "sam")
        .action(Action::new("reply", "Reply").text_input("Send", "Message"))
        .action(Action::new("mute", "Mute").opens_app(false))
}

fn responses() -> (Rc<RefCell<Vec<NotificationResponse>>>, notifications::ResponseSubscription) {
    let seen: Rc<RefCell<Vec<NotificationResponse>>> = Rc::default();
    let sink = seen.clone();
    let sub = on_response(move |r| sink.borrow_mut().push(r.clone()));
    (seen, sub)
}

fn response(id: &str, action: ResponseAction) -> NotificationResponse {
    NotificationResponse {
        id: NotificationId::from(id),
        action,
        text: None,
        data: HashMap::from([("chat".to_string(), "sam".to_string())]),
    }
}

/// With an active worker the notification goes through
/// `showNotification`, carrying the buttons and the data the handlers read
/// back; the page-level constructor (which throws on Chrome for Android) is
/// not touched.
#[wasm_bindgen_test]
async fn an_active_worker_shows_it_with_actions_and_data() {
    fresh_worker(true).await;
    notify(chat()).await.unwrap();
    assert_eq!(
        eval("JSON.stringify((({ close, ...n }) => n)(globalThis.__sw.shown[0]))"),
        r#"{"title":"Sam","actions":[{"action":"reply","title":"Reply"},{"action":"mute","title":"Mute"}],"body":"Lunch?","data":{"background":["mute"],"id":"chat-sam","idealyst":1,"payload":{"chat":"sam"}},"renotify":true,"tag":"chat-sam"}"#
    );
}

#[wasm_bindgen_test]
async fn a_click_focuses_the_open_page_and_reports_open() {
    fresh_worker(true).await;
    let (seen, _sub) = responses();
    notify(chat()).await.unwrap();
    run("globalThis.__sw.user('notificationclick', 'chat-sam');");
    settle().await;
    assert_eq!(*seen.borrow(), vec![response("chat-sam", ResponseAction::Open)]);
    assert_eq!(eval("globalThis.__sw.calls.join()"), "close:chat-sam,focus");
}

/// An action with `opens_app(false)` is reported without bringing the app
/// up; one that opens the app focuses it.
#[wasm_bindgen_test]
async fn actions_report_their_id_and_only_opening_ones_focus() {
    fresh_worker(true).await;
    let (seen, _sub) = responses();
    notify(chat()).await.unwrap();
    run("globalThis.__sw.user('notificationclick', 'chat-sam', 'mute');");
    settle().await;
    assert_eq!(*seen.borrow(), vec![response("chat-sam", ResponseAction::Action("mute".into()))]);
    assert_eq!(eval("globalThis.__sw.calls.join()"), "close:chat-sam", "no focus for a background action");

    notify(chat()).await.unwrap();
    run("globalThis.__sw.calls = []; globalThis.__sw.user('notificationclick', 'chat-sam', 'reply');");
    settle().await;
    assert_eq!(seen.borrow()[1], response("chat-sam", ResponseAction::Action("reply".into())));
    assert_eq!(eval("globalThis.__sw.calls.join()"), "close:chat-sam,focus");
}

#[wasm_bindgen_test]
async fn a_user_dismissal_reports_dismiss() {
    fresh_worker(true).await;
    let (seen, _sub) = responses();
    notify(chat()).await.unwrap();
    run("globalThis.__sw.user('notificationclose', 'chat-sam');");
    settle().await;
    assert_eq!(*seen.borrow(), vec![response("chat-sam", ResponseAction::Dismiss)]);
}

/// The bug class the worker path exists for: on the page-level API a click with no
/// tab open was lost. Through the worker it opens the app, and the response
/// waits in the outbox until the opened page is listening.
#[wasm_bindgen_test]
async fn regression_a_click_with_no_tab_open_opens_the_app_and_reaches_it() {
    fresh_worker(true).await;
    let (seen, _sub) = responses();
    notify(chat()).await.unwrap();
    run("globalThis.__sw.pageOpen = false; globalThis.__sw.user('notificationclick', 'chat-sam');");
    settle().await;
    assert!(seen.borrow().is_empty(), "no page yet");
    assert_eq!(eval("globalThis.__sw.calls.join()"), "close:chat-sam,open:http://127.0.0.1/app/");

    // The opened page boots and says it is listening.
    run("globalThis.__sw.pageOpen = true; globalThis.__sw.container.dispatchEvent(new Event('controllerchange'));");
    settle().await;
    assert_eq!(*seen.borrow(), vec![response("chat-sam", ResponseAction::Open)]);
}

#[wasm_bindgen_test]
async fn cancel_closes_the_worker_shown_notification() {
    fresh_worker(true).await;
    notify(chat()).await.unwrap();
    notify(Notification::new("Other", "x").id("other")).await.unwrap();
    cancel(&NotificationId::from("chat-sam")).await;
    settle().await;
    assert_eq!(eval("globalThis.__sw.shown.map((n) => n.tag + ':' + !!n.closed).join()"), "chat-sam:true,other:false");
}

/// The bug: on macOS a notification whose banner was swiped away stays in
/// Notification Center, and posting the same id again updated that entry
/// silently — "Posted as chat-sam", no banner. Each post must close what is
/// shown under its tag first, so it arrives as a new notification; that
/// close is the code's, so it reports no dismissal.
#[wasm_bindgen_test]
async fn regression_a_repost_after_a_swipe_shows_as_a_new_notification() {
    fresh_worker(true).await;
    let (seen, _sub) = responses();
    notify(chat()).await.unwrap();
    // Swiped away: still listed by the registration, not closed.
    notify(Notification::new("Sam", "Still on for lunch?").id("chat-sam")).await.unwrap();
    assert_eq!(eval("globalThis.__sw.calls.join()"), "close:chat-sam", "the shown one is closed before the new post");
    assert_eq!(eval("globalThis.__sw.shown.map((n) => n.body).join()"), "Still on for lunch?");
    settle().await;
    assert!(seen.borrow().is_empty(), "closing it from code is not a dismissal");
}
