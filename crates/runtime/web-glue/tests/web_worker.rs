//! `web_glue::worker` in a real browser: a Worker that instantiates this
//! same test module and runs a Rust `fn()` there, with the glue runtime
//! (handles, strings, callbacks, the executor) working in a
//! `WorkerGlobalScope` — no `window`, no `document`.
//!
//! Runs through the workspace's wasm32 runner, i.e. in hybrid mode: the
//! worker imports `__idealyst_glue.js`, which instantiates the module via
//! wasm-bindgen-test's own `init`.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::{ListenerOptions, Listener, MessageEvent};
use web_glue::worker::{self, Worker};
use web_glue::{JsCast, JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

web_glue::import! {
    // The next `message` from `w` (its data), or a rejection carrying the
    // `error` event's message — whichever comes first.
    fn next_event(w: u32) -> u32 =
        "(w) => { const t = G.get(w); return G.add(new Promise((res, rej) => { \
           const m = (e) => { t.removeEventListener('error', x); res(e.data); }; \
           const x = (e) => { e.preventDefault(); t.removeEventListener('message', m); \
             rej(new Error(String(e.message))); }; \
           t.addEventListener('message', m, { once: true }); \
           t.addEventListener('error', x, { once: true }); })); }";
}

async fn next(w: &Worker) -> Result<JsValue, String> {
    let p = unsafe { JsValue::from_raw(next_event(w.as_js().raw())) };
    JsFuture::new(&p).await.map_err(|e| e.message())
}

/// Runs in the worker: proves the scope, then echoes every message back
/// upper-cased through a glue listener, and signals readiness from a task
/// on the glue executor (microtasks work in a worker too).
fn echo_entry() {
    let report = format!(
        "in_worker={} window={}",
        worker::in_worker(),
        web_glue::dom::window().is_some()
    );
    Listener::new(worker::scope(), "message", ListenerOptions::default(), |ev| {
        let data = ev.unchecked_into::<MessageEvent>().data();
        let text = data.as_string().unwrap_or_default();
        worker::post_to_parent(&JsValue::from_str(&text.to_uppercase())).unwrap();
    })
    .into_target_owned();
    web_glue::spawn_local(async move {
        let v = JsFuture::resolve(&JsValue::from_str(&report)).await.unwrap();
        worker::post_to_parent(&v).unwrap();
    });
}

fn panicking_entry() {
    panic!("deliberate panic in a worker entry");
}

#[wasm_bindgen_test]
async fn a_worker_runs_the_entry_in_a_fresh_instance_of_this_module() {
    assert!(!worker::in_worker());
    let w = worker::spawn(echo_entry).expect("spawn");
    let ready = next(&w).await.expect("the worker's ready message");
    assert_eq!(ready.as_string().as_deref(), Some("in_worker=true window=false"));
    // Strings both ways through the glue runtime in the worker, non-ASCII
    // included, and the listener survives repeated messages.
    for msg in ["héllo wörld", "second"] {
        w.post_message(&JsValue::from_str(msg)).unwrap();
        let back = next(&w).await.expect("echo");
        assert_eq!(back.as_string().unwrap(), msg.to_uppercase());
    }
    w.terminate();
}

#[wasm_bindgen_test]
async fn a_panicking_entry_surfaces_as_the_workers_error_event() {
    let w = worker::spawn(panicking_entry).expect("spawn");
    let err = next(&w).await.expect_err("the entry traps, so an error event, not a message");
    assert!(!err.is_empty(), "the error event carries a message");
    w.terminate();
}

#[wasm_bindgen_test]
fn hardware_concurrency_is_reported() {
    assert!(worker::hardware_concurrency().unwrap_or(1) >= 1);
}
