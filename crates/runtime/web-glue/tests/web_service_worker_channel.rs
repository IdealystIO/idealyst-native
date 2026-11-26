//! The `web_glue::service_worker` channel in a real browser.
//!
//! A real worker needs a script served from this origin, which this
//! harness doesn't serve, so the worker half runs in the page: `PRELUDE`
//! (the exact text every composed `idealyst-sw.js` starts with) evaluated
//! against a stand-in worker scope — an `EventTarget` with the page's real
//! IndexedDB and a fake `clients` — wired to a stand-in
//! `navigator.serviceWorker`. Restarting the "worker" re-evaluates the
//! prelude on a fresh scope, which is what the browser stopping and waking
//! it does: memory gone, IndexedDB kept. Its own test binary (so its own
//! page): it replaces `navigator.serviceWorker` for good.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::{service_worker, JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn run(body: &str) {
    let f = JsValue::global().get("Function").unwrap().construct(&[&JsValue::from_str(body)]).unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}


/// Evaluate `body` as a function body and await the promise it returns.
async fn eval_async(body: &str) {
    let f = JsValue::global().get("Function").unwrap().construct(&[&JsValue::from_str(body)]).unwrap();
    let p = f.call(&JsValue::undefined(), &[]).unwrap();
    JsFuture::new(&p).await.unwrap();
}

async fn settle() {
    eval_async("return new Promise((r) => setTimeout(r, 50));").await;
}

/// Install the stand-ins once per page (the page's listener, like a real
/// page's, is installed once), and (re)start the worker with an empty
/// outbox unless `keep_outbox`.
async fn fresh_worker(keep_outbox: bool) {
    JsValue::global().set("__prelude", &JsValue::from_str(service_worker::PRELUDE)).unwrap();
    run(r#"
        if (!globalThis.__sw) {
          const sw = (globalThis.__sw = { pageOpen: true, self: null });
          const container = new EventTarget();
          sw.worker = { postMessage: (data) => queueMicrotask(() => {
            const e = new Event("message");
            Object.defineProperty(e, "data", { value: data });
            Object.defineProperty(e, "source", { value: sw.page });
            e.waitUntil = (p) => p.catch((err) => console.error("waitUntil", err));
            sw.self.dispatchEvent(e);
          }) };
          sw.page = { id: "page-1", focused: true, visibilityState: "visible",
            postMessage: (data) => queueMicrotask(() => {
              const e = new MessageEvent("message", { data });
              Object.defineProperty(e, "source", { value: sw.worker });
              container.dispatchEvent(e);
            }) };
          container.ready = Promise.resolve({ active: sw.worker });
          container.controller = sw.worker;
          container.startMessages = () => {};
          container.register = () => Promise.resolve({});
          sw.container = container;
          Object.defineProperty(navigator, "serviceWorker", { value: container, configurable: true });
        }
        const sw = globalThis.__sw;
        sw.pageOpen = true;
        const self = new EventTarget();
        self.indexedDB = indexedDB;
        self.skipWaiting = () => {};
        self.clients = {
          matchAll: async () => (sw.pageOpen ? [sw.page] : []),
          claim: async () => {},
        };
        sw.self = self;
        new Function("self", globalThis.__prelude)(self);
        // A contribution: echo what the page posts on "echo" back on "echo-back".
        self.idealyst.on("echo", (m) => self.idealyst.deliver("echo-back", m + "!"));
    "#);
    if !keep_outbox {
        eval_async(
            "return new Promise((r) => { const q = indexedDB.deleteDatabase('idealyst-sw'); \
             q.onsuccess = q.onerror = q.onblocked = () => r(); });",
        )
        .await;
    }
}

fn collector(channel: &str) -> (Rc<RefCell<Vec<String>>>, service_worker::MessageSubscription) {
    let got: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = got.clone();
    let sub = service_worker::on_message(channel, move |m| {
        sink.borrow_mut().push(m.as_string().unwrap_or_else(|| "<not a string>".into()));
    });
    (got, sub)
}

#[wasm_bindgen_test]
async fn a_delivered_message_reaches_the_channel_handler_once() {
    fresh_worker(false).await;
    let (got, _sub) = collector("greet");
    settle().await;
    // A worker that started after the page said `ready` knows no
    // listener: it pings, the page answers, the outbox flushes.
    run("globalThis.__sw.self.idealyst.deliver('greet', 'hello');");
    settle().await;
    assert_eq!(*got.borrow(), ["hello"]);
    run("globalThis.__sw.self.idealyst.deliver('greet', 'again');");
    settle().await;
    assert_eq!(*got.borrow(), ["hello", "again"], "the listener is now known: straight through");
}

/// The case the outbox exists for: a message delivered while no page is
/// open (a notification clicked with every tab closed), then the worker
/// stops before a page boots. It must still reach the page that does.
#[wasm_bindgen_test]
async fn regression_a_message_survives_no_page_and_a_worker_restart() {
    fresh_worker(false).await;
    let (got, _sub) = collector("launch");
    settle().await;
    run("globalThis.__sw.pageOpen = false; globalThis.__sw.self.idealyst.deliver('launch', 'tap');");
    settle().await;
    assert!(got.borrow().is_empty(), "no page to take it");

    fresh_worker(true).await; // the worker stopped and woke: memory gone, outbox kept
    // The page boots and takes control: it says `ready` to the new worker.
    run("globalThis.__sw.container.dispatchEvent(new Event('controllerchange'));");
    settle().await;
    assert_eq!(*got.borrow(), ["tap"]);
}

#[wasm_bindgen_test]
async fn a_message_for_an_unhandled_channel_waits_for_its_first_handler() {
    fresh_worker(false).await;
    let (_other, _sub) = collector("unrelated");
    settle().await;
    run("globalThis.__sw.self.idealyst.deliver('late', 'early bird');");
    settle().await;
    let (got, _late) = collector("late");
    assert!(got.borrow().is_empty(), "handed over on a microtask, not inside on_message");
    settle().await;
    assert_eq!(*got.borrow(), ["early bird"]);
}

#[wasm_bindgen_test]
async fn post_reaches_the_workers_handler() {
    fresh_worker(false).await;
    let (got, _sub) = collector("echo-back");
    settle().await;
    service_worker::post("echo", &JsValue::from_str("ping"));
    settle().await;
    assert_eq!(*got.borrow(), ["ping!"]);
}

/// Concurrent deliveries each flush; the outbox is drained in one
/// transaction, so none is sent twice and none is lost.
#[wasm_bindgen_test]
async fn concurrent_deliveries_arrive_exactly_once() {
    fresh_worker(false).await;
    let (got, _sub) = collector("burst");
    settle().await;
    run("globalThis.__sw.self.idealyst.deliver('burst', 'warm');");
    settle().await;
    run("const d = globalThis.__sw.self.idealyst; for (let i = 0; i < 5; i++) d.deliver('burst', 'm' + i);");
    settle().await;
    let mut seen = got.borrow().clone();
    seen.sort();
    assert_eq!(seen, ["m0", "m1", "m2", "m3", "m4", "warm"]);
}

#[wasm_bindgen_test]
async fn a_dropped_subscription_stops_receiving() {
    fresh_worker(false).await;
    let (got, sub) = collector("gone");
    settle().await;
    run("globalThis.__sw.self.idealyst.deliver('gone', 'one');");
    settle().await;
    drop(sub);
    run("globalThis.__sw.self.idealyst.deliver('gone', 'two');");
    settle().await;
    assert_eq!(*got.borrow(), ["one"]);
}
