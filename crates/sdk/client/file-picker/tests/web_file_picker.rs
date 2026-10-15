//! The web file-picker leg in a real browser: a dropped file (as backend-web
//! hands it over — a `web_glue::dom::File` in `DroppedFile::source`) becomes a
//! `PickedFile` that streams its bytes; and the `<input type=file>` fallback
//! resolves a cancel as `Cancelled` and cleans its input up.
//!
//! Run with `cargo test -p file-picker --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use file_picker::{FileDropZone, FilePicker, PickOutcome, PickRequest, PickedFile};
use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase, TouchPoint};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::File;
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn js_file(body: &str, name: &str, mime: &str) -> File {
    Function::new_with_args("b, n, t", "return new File([b], n, { type: t });")
        .call3(&JsValue::UNDEFINED, &JsValue::from_str(body), &JsValue::from_str(name), &JsValue::from_str(mime))
        .unwrap()
        .dyn_into()
        .unwrap()
}

/// Regression guard for the crossing that used to be a `web_sys::File`: the
/// dropped file's source is a `web_glue::dom::File`, and file-picker must
/// take it (a type mismatch here silently drops every file).
#[wasm_bindgen_test]
async fn dropped_glue_file_streams_through_the_picker() {
    let body = "hello, dropped file — ünïcode";
    let file = js_file(body, "note.txt", "text/plain");
    let dropped = DroppedFile {
        name: "note.txt".into(),
        mime: "text/plain".into(),
        size: Some(file.size() as u64),
        path: None,
        source: Some(Rc::new(file)),
    };
    let got: Rc<RefCell<Vec<PickedFile>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = got.clone();
    // `FileDropZone` holds a signal (its drag-active state): build and drive
    // it inside a world.
    let world = runtime_world::World::new();
    world.enter(|| {
        let zone = FileDropZone::new().on_drop(move |files| *sink.borrow_mut() = files);
        let handler = zone.handler();
        let _ = handler(&FileDropEvent {
            phase: FileDropPhase::Dropped(vec![dropped]),
            position: TouchPoint::new(0.0, 0.0),
        });
    });

    let files = std::mem::take(&mut *got.borrow_mut());
    assert_eq!(files.len(), 1, "the dropped file became a PickedFile");
    let f = &files[0];
    assert_eq!((f.name(), f.mime(), f.path()), ("note.txt", "text/plain", None));
    assert_eq!(f.size(), Some(body.len() as u64));
    let bytes = f.read_all().await.expect("stream the Blob");
    assert_eq!(String::from_utf8(bytes).unwrap(), body);
}

/// The `<input type=file>` fallback (no File System Access API): dismissing
/// the dialog resolves `Cancelled` and removes the hidden input.
#[wasm_bindgen_test]
async fn input_fallback_cancel_is_cancelled() {
    // Force the fallback, and dismiss the dialog as soon as it opens.
    Function::new_no_args(
        "window.showOpenFilePicker = undefined; \
         HTMLInputElement.prototype.click = function () { \
           setTimeout(() => this.dispatchEvent(new Event('cancel')), 0); };",
    )
    .call0(&JsValue::UNDEFINED)
    .unwrap();
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.expect("pick");
    assert!(matches!(out, PickOutcome::Cancelled));
    let left = Function::new_no_args("return document.querySelectorAll('input[type=file]').length;")
        .call0(&JsValue::UNDEFINED)
        .unwrap()
        .as_f64();
    assert_eq!(left, Some(0.0), "the hidden input is removed");
}

/// Install a fake `showOpenFilePicker` (body `js`, which sees `opts` and
/// returns its promise) and an `<input>` `click()` that counts how many
/// fallback dialogs were opened and dismisses each one.
fn install_fsa_mock(js: &str) {
    Function::new_no_args(&format!(
        "window.__inputDialogs = 0; \
         window.showOpenFilePicker = function (opts) {{ {js} }}; \
         HTMLInputElement.prototype.click = function () {{ window.__inputDialogs++; \
           setTimeout(() => this.dispatchEvent(new Event('cancel')), 0); }};"
    ))
    .call0(&JsValue::UNDEFINED)
    .unwrap();
}

fn input_dialogs_opened() -> Option<f64> {
    Function::new_no_args("return window.__inputDialogs;").call0(&JsValue::UNDEFINED).unwrap().as_f64()
}

/// The reported bug (VS Code's embedded Electron browser): the FSA dialog was
/// shown, the user picked, and the pick was rejected afterwards with a
/// non-`AbortError` — `pick()` then opened the `<input>` as a SECOND dialog.
/// A post-dialog rejection must surface as an error, with no second dialog.
#[wasm_bindgen_test]
async fn regression_post_dialog_rejection_never_reopens_picker() {
    install_fsa_mock(
        "return Promise.reject(new DOMException('The request is not allowed by the user agent', 'NotAllowedError'));",
    );
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await;
    let err = out.err().expect("a post-dialog rejection is an error, not a pick or a cancel");
    assert!(err.to_string().contains("NotAllowedError"), "the reason is kept: {err}");
    assert_eq!(input_dialogs_opened(), Some(0.0), "no second (input) dialog");
}

/// Same bug through the other door: the picker returned a handle but its
/// `getFile()` rejects (and a read-permission request does not help). The
/// error names the picked file; no second dialog.
#[wasm_bindgen_test]
async fn regression_getfile_failure_after_pick_never_reopens_picker() {
    install_fsa_mock(
        "return Promise.resolve([{ name: 'report.pdf', \
           getFile: () => Promise.reject(new DOMException('cannot read', 'NotReadableError')) }]);",
    );
    let err = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.err().expect("an error");
    let msg = err.to_string();
    assert!(msg.contains("report.pdf") && msg.contains("NotReadableError"), "{msg}");
    assert_eq!(input_dialogs_opened(), Some(0.0), "no second (input) dialog");
}

/// A handle without a read grant yet: `getFile()` rejects `NotAllowedError`;
/// the picker asks for read permission once and returns the user's file.
#[wasm_bindgen_test]
async fn getfile_not_allowed_recovers_through_request_permission() {
    install_fsa_mock(
        "let granted = false; \
         return Promise.resolve([{ name: 'a.txt', \
           requestPermission: () => { granted = true; return Promise.resolve('granted'); }, \
           getFile: () => granted ? Promise.resolve(new File(['abc'], 'a.txt', { type: 'text/plain' })) \
             : Promise.reject(new DOMException('no grant', 'NotAllowedError')) }]);",
    );
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.expect("pick");
    let PickOutcome::Picked(files) = out else { panic!("expected the picked file") };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].read_all().await.expect("read"), b"abc");
    assert_eq!(input_dialogs_opened(), Some(0.0));
}

/// A rejection the spec raises BEFORE any dialog (no user activation →
/// `SecurityError`) still falls back to the `<input>` — exactly once.
#[wasm_bindgen_test]
async fn pre_dialog_security_error_falls_back_to_input_once() {
    install_fsa_mock("return Promise.reject(new DOMException('Must be handling a user gesture', 'SecurityError'));");
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.expect("pick");
    assert!(matches!(out, PickOutcome::Cancelled));
    assert_eq!(input_dialogs_opened(), Some(1.0), "exactly one (fallback) dialog");
}

/// Dismissing the FSA dialog is a cancel, with no fallback dialog.
#[wasm_bindgen_test]
async fn fsa_abort_is_cancelled_without_fallback() {
    install_fsa_mock("return Promise.reject(new DOMException('The user aborted a request.', 'AbortError'));");
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.expect("pick");
    assert!(matches!(out, PickOutcome::Cancelled));
    assert_eq!(input_dialogs_opened(), Some(0.0));
}
