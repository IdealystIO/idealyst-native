//! `web_glue::service_worker` in a real browser.
//!
//! A successful registration needs a worker script served from the page's
//! origin, which this harness doesn't serve; the web build's end-to-end
//! tests cover that. What is checked here is that every failure surfaces as
//! an `Err` from the awaited call — never an uncaught exception or an
//! unhandled promise rejection at boot.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::{service_worker, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn run(body: &str) {
    let f = JsValue::global().get("Function").unwrap().construct(&[&JsValue::from_str(body)]).unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

#[wasm_bindgen_test]
async fn a_script_that_cannot_be_fetched_is_an_err() {
    assert!(service_worker::is_supported(), "the test page is a secure context");
    let err = service_worker::register("no-such-worker-script.js").await.unwrap_err();
    assert!(!err.message().is_empty());
}

#[wasm_bindgen_test]
async fn no_service_worker_api_is_an_err_not_a_throw() {
    run("Object.defineProperty(navigator, 'serviceWorker', { value: undefined, configurable: true });");
    let supported = service_worker::is_supported();
    let out = service_worker::register("idealyst-sw.js").await;
    run("delete navigator.serviceWorker;");
    assert!(!supported);
    let msg = out.unwrap_err().message();
    assert!(msg.contains("unavailable"), "{msg}");
    assert!(service_worker::is_supported(), "the stub is gone");
}
