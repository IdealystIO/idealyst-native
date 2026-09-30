//! Browser tests for the web save backend: `showSaveFilePicker` (stand-in,
//! since a headless page can't answer the dialog) and the `<a download>`
//! fallback (the anchor's `click` is intercepted so no real download
//! starts).
//!
//! Run with `cargo test -p file-export --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use file_export::{ExportError, FileExport, SaveOutcome, SaveRequest};
use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// Install `window.showSaveFilePicker = <expr>` until dropped (absent when
/// `expr` is `None`, which is the fallback path).
struct PickerOverride;

impl PickerOverride {
    fn install(expr: Option<&str>) -> PickerOverride {
        match expr {
            Some(e) => eval(&format!("window.showSaveFilePicker = {e};")),
            None => eval("delete window.showSaveFilePicker;"),
        };
        PickerOverride
    }
}

impl Drop for PickerOverride {
    fn drop(&mut self) {
        eval("delete window.showSaveFilePicker; delete globalThis.__written; delete globalThis.__opts;");
    }
}

/// A picker whose writable records the blob it was handed; `write` is a JS
/// expression for the writable's `write` method.
fn picker(write: &str) -> String {
    format!(
        "(opts) => {{ globalThis.__opts = JSON.stringify(opts); return Promise.resolve({{ \
            createWritable: () => Promise.resolve({{ \
              write: {write}, close: () => Promise.resolve() }}) }}); }}"
    )
}

fn request() -> SaveRequest {
    SaveRequest::bytes("clip.txt", "text/plain", b"hello, saved".to_vec())
}

#[wasm_bindgen_test]
async fn the_picker_path_writes_the_blob() {
    let _p = PickerOverride::install(Some(&picker(
        "(b) => { globalThis.__written = b; return Promise.resolve(); }",
    )));
    let out = FileExport::new().save(request()).await.expect("saved");
    assert_eq!(out, SaveOutcome::Saved { location: None });
    assert_eq!(
        JsValue::global().get("__opts").unwrap().as_string().as_deref(),
        Some(r#"{"suggestedName":"clip.txt"}"#)
    );
    let written = JsValue::global().get("__written").unwrap();
    assert_eq!(written.get("type").unwrap().as_string().as_deref(), Some("text/plain"));
    let text = JsFuture::new(&written.call_method("text", &[]).unwrap()).await.unwrap();
    assert_eq!(text.as_string().as_deref(), Some("hello, saved"));
}

#[wasm_bindgen_test]
async fn dismissing_the_picker_is_cancelled() {
    let _p = PickerOverride::install(Some(
        "() => Promise.reject(new DOMException('dismissed', 'AbortError'))",
    ));
    assert_eq!(FileExport::new().save(request()).await.unwrap(), SaveOutcome::Cancelled);
}

#[wasm_bindgen_test]
async fn a_failed_write_is_a_backend_error() {
    let _p = PickerOverride::install(Some(&picker(
        "() => Promise.reject(new DOMException('disk full', 'QuotaExceededError'))",
    )));
    match FileExport::new().save(request()).await {
        Err(ExportError::Backend(msg)) => {
            assert!(msg.starts_with("write await:") && msg.contains("disk full"), "{msg}")
        }
        other => panic!("expected a Backend error, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn without_the_picker_an_anchor_download_is_clicked() {
    let _p = PickerOverride::install(None);
    eval(
        "globalThis.__savedClick = HTMLAnchorElement.prototype.click; \
         HTMLAnchorElement.prototype.click = function () { \
           globalThis.__clicked = [this.href, this.download]; };",
    );
    let out = FileExport::new().save(request()).await;
    eval("HTMLAnchorElement.prototype.click = globalThis.__savedClick;");
    assert_eq!(out.unwrap(), SaveOutcome::Saved { location: None });
    let clicked = JsValue::global().get("__clicked").unwrap();
    let href = clicked.get("0").unwrap().as_string().unwrap();
    assert!(href.starts_with("blob:"), "{href}");
    assert_eq!(clicked.get("1").unwrap().as_string().as_deref(), Some("clip.txt"));
}

#[wasm_bindgen_test]
async fn a_path_source_is_unsupported_on_web() {
    let r = SaveRequest::path("a.txt", "text/plain", "/tmp/a.txt");
    assert!(matches!(FileExport::new().save(r).await, Err(ExportError::Unsupported)));
}
