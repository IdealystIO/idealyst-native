//! Browser tests for the web clipboard backend (`navigator.clipboard`).
//!
//! The real clipboard needs focus and a permission grant a headless test
//! page cannot give, so `navigator.clipboard` is shadowed per test with a
//! stand-in built in JS — what's under test is the SDK's binding: the
//! lookup, the string both ways, and the Promise plumbing.
//!
//! Run with `cargo test -p clipboard --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use clipboard::{set_text, text, ClipboardError};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

/// Shadow `navigator.clipboard` with `value` (the JS expression's result)
/// until the guard drops.
struct ClipboardOverride;

impl ClipboardOverride {
    fn install(js_expr: &str) -> ClipboardOverride {
        let body = format!(
            "Object.defineProperty(navigator, 'clipboard', {{ value: {js_expr}, configurable: true }});"
        );
        run(&body);
        ClipboardOverride
    }
}

impl Drop for ClipboardOverride {
    fn drop(&mut self) {
        run("delete navigator.clipboard;");
    }
}

fn run(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

/// Regression: outside a secure context `navigator.clipboard` is
/// `undefined`. The web-sys port called `writeText` on it anyway, which
/// threw a TypeError through the wasm frames; it must be a `Backend`
/// error.
#[wasm_bindgen_test]
async fn regression_missing_clipboard_api_is_a_backend_error() {
    let _gone = ClipboardOverride::install("undefined");
    assert!(matches!(set_text("x").await, Err(ClipboardError::Backend(_))));
    assert!(matches!(text().await, Err(ClipboardError::Backend(_))));
}

#[wasm_bindgen_test]
async fn text_round_trips_through_the_promises() {
    let _stub = ClipboardOverride::install(
        "(() => { let v = ''; return { \
            writeText: (s) => { v = s; return Promise.resolve(); }, \
            readText: () => Promise.resolve(v) }; })()",
    );
    // Empty clipboard reads as `None`, like the native backends.
    assert_eq!(text().await, Ok(None));
    set_text("コピー ✂ copy").await.unwrap();
    assert_eq!(text().await, Ok(Some("コピー ✂ copy".to_string())));
}

#[wasm_bindgen_test]
async fn a_rejected_read_is_a_backend_error_with_the_reason() {
    let _stub = ClipboardOverride::install(
        "({ writeText: () => Promise.reject(new DOMException('no gesture', 'NotAllowedError')), \
            readText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) })",
    );
    match text().await {
        Err(ClipboardError::Backend(msg)) => assert!(msg.contains("denied"), "{msg}"),
        other => panic!("expected a Backend error, got {other:?}"),
    }
    assert!(matches!(set_text("x").await, Err(ClipboardError::Backend(_))));
}
