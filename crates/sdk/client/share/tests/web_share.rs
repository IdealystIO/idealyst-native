//! Browser tests for the web share backend (`navigator.share`).
//!
//! Headless Chrome has no share sheet, so `navigator.share` is shadowed per
//! test with a JS stand-in: what's under test is the SDK's binding — the
//! feature probe, the `ShareData` it builds, and how each settle maps to a
//! `ShareOutcome` / `ShareError`.
//!
//! Run with `cargo test -p share --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use share::{share, ShareContent, ShareError, ShareOutcome};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

fn run(body: &str) {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap();
}

/// Shadow `navigator.share` with the JS expression's value until the guard
/// drops.
struct ShareOverride;

impl ShareOverride {
    fn install(js_expr: &str) -> ShareOverride {
        run(&format!(
            "Object.defineProperty(navigator, 'share', {{ value: {js_expr}, configurable: true, writable: true }});"
        ));
        ShareOverride
    }
}

impl Drop for ShareOverride {
    fn drop(&mut self) {
        run("delete navigator.share; delete globalThis.__shared;");
    }
}

fn last_shared() -> String {
    JsValue::global().get("__shared").unwrap().as_string().unwrap_or_default()
}

#[wasm_bindgen_test]
async fn missing_web_share_api_is_not_supported() {
    let _gone = ShareOverride::install("undefined");
    assert_eq!(share(ShareContent::text("hi")).await, Err(ShareError::NotSupported));
}

#[wasm_bindgen_test]
async fn resolved_share_completes_with_only_the_present_members() {
    let _stub = ShareOverride::install(
        "(d) => { globalThis.__shared = JSON.stringify(d); return Promise.resolve(); }",
    );
    let out = share(ShareContent::url("https://idealyst.dev/x").with_title("Tïtle")).await;
    assert_eq!(out, Ok(ShareOutcome::Completed));
    // No `text` key at all — an absent member is left off, not set to "".
    assert_eq!(last_shared(), r#"{"title":"Tïtle","url":"https://idealyst.dev/x"}"#);
}

#[wasm_bindgen_test]
async fn abort_error_is_a_dismissal() {
    let _stub = ShareOverride::install(
        "() => Promise.reject(new DOMException('user cancelled', 'AbortError'))",
    );
    assert_eq!(share(ShareContent::text("hi")).await, Ok(ShareOutcome::Dismissed));
}

#[wasm_bindgen_test]
async fn other_rejections_are_backend_errors() {
    let _stub = ShareOverride::install(
        "() => Promise.reject(new DOMException('no activation', 'NotAllowedError'))",
    );
    match share(ShareContent::text("hi")).await {
        Err(ShareError::Backend(msg)) => assert!(msg.contains("NotAllowedError"), "{msg}"),
        other => panic!("expected a Backend error, got {other:?}"),
    }
}

/// A synchronous throw from `navigator.share` (e.g. a TypeError for bad
/// ShareData) is caught by the binding and reported, not propagated.
#[wasm_bindgen_test]
async fn a_synchronous_throw_is_a_backend_error() {
    let _stub = ShareOverride::install("() => { throw new TypeError('bad data'); }");
    match share(ShareContent::text("hi")).await {
        Err(ShareError::Backend(msg)) => assert!(msg.contains("bad data"), "{msg}"),
        other => panic!("expected a Backend error, got {other:?}"),
    }
}
