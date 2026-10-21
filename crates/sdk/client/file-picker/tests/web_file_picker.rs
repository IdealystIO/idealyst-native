//! The web file-picker leg in a real browser: a dropped file (as backend-web
//! hands it over — a `web_glue::dom::File` in `DroppedFile::source`) becomes a
//! `PickedFile` that streams its bytes; and a pick goes through one
//! `<input type=file>` (never the File System Access picker), returns the
//! chosen files, resolves a cancel as `Cancelled` and cleans its input up.
//!
//! The browser's own file dialog can't be driven from a test, so each test
//! replaces `HTMLInputElement.prototype.click` with one that sets the
//! input's `files` and fires `change` / `cancel` the way the dialog does.
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

/// Regression guard for the drop → picker crossing: the
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

/// In a browser without the File System Access API (Safari, Firefox),
/// dismissing the dialog resolves `Cancelled` and removes the hidden input.
#[wasm_bindgen_test]
async fn input_cancel_without_fsa_is_cancelled() {
    // No FSA, and dismiss the dialog as soon as it opens.
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

/// Install a File System Access `showOpenFilePicker` that counts its calls
/// and hands back one handle whose `getFile()` rejects `NotAllowedError` even
/// after `requestPermission` — Chrome on macOS refusing to read a fresh
/// Desktop screenshot — plus an `<input>` `click()` that counts the dialogs
/// it opens and "picks" `files` (a JS array expression) in each one.
fn install_pickers(files: &str) {
    Function::new_no_args(&format!(
        "window.__fsaCalls = 0; window.__inputDialogs = 0; \
         window.showOpenFilePicker = function () {{ window.__fsaCalls++; \
           return Promise.resolve([{{ name: 'denied.png', \
             requestPermission: () => Promise.resolve('granted'), \
             getFile: () => Promise.reject(new DOMException( \
               'The request is not allowed by the user agent or the platform in the current context.', \
               'NotAllowedError')) }}]); }}; \
         HTMLInputElement.prototype.click = function () {{ window.__inputDialogs++; \
           const dt = new DataTransfer(); for (const f of {files}) dt.items.add(f); \
           this.files = dt.files; \
           setTimeout(() => this.dispatchEvent(new Event(dt.files.length ? 'change' : 'cancel')), 0); }};"
    ))
    .call0(&JsValue::UNDEFINED)
    .unwrap();
}

fn counter(name: &str) -> Option<f64> {
    Function::new_no_args(&format!("return window.{name};")).call0(&JsValue::UNDEFINED).unwrap().as_f64()
}

/// The reported bug (CrewForge, Chrome on macOS): `pick()` went through the
/// File System Access picker, the chosen handle's `getFile()` rejected
/// `NotAllowedError`, and the pick failed with "could not read the picked
/// file". A pick only ever reads, so the web picker uses `<input type=file>`
/// (a `File` the browser already granted) and never asks a handle for one.
/// The name carries the U+202F macOS puts before "AM" in screenshot names;
/// it must round-trip unchanged.
#[wasm_bindgen_test]
async fn regression_read_only_pick_reads_through_the_input_not_fsa() {
    install_pickers(
        "[new File([new Uint8Array([137, 80, 78, 71])], 'Screenshot 2026-10-21 at 9.20.40\\u202fAM.png', \
           { type: 'image/png' })]",
    );
    let out = FilePicker::new()
        .pick(PickRequest::documents(["image/png", "image/jpeg", "image/webp"]))
        .await
        .expect("the pick reads the chosen file");
    let PickOutcome::Picked(files) = out else { panic!("expected the picked file") };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name(), "Screenshot 2026-10-21 at 9.20.40\u{202F}AM.png");
    assert_eq!(files[0].mime(), "image/png");
    assert_eq!(files[0].read_all().await.expect("read"), [137, 80, 78, 71]);
    assert_eq!(counter("__fsaCalls"), Some(0.0), "the File System Access picker is never used");
    assert_eq!(counter("__inputDialogs"), Some(1.0), "exactly one dialog");
}

/// `multiple()` picks every selected file through the one input dialog.
#[wasm_bindgen_test]
async fn input_pick_returns_every_selected_file() {
    install_pickers(
        "[new File(['a'], 'a.txt', { type: 'text/plain' }), new File(['bc'], 'b.txt', { type: 'text/plain' })]",
    );
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"]).multiple()).await.expect("pick");
    let PickOutcome::Picked(files) = out else { panic!("expected the picked files") };
    let names: Vec<_> = files.iter().map(|f| f.name().to_string()).collect();
    assert_eq!(names, ["a.txt", "b.txt"]);
    assert_eq!(files[1].read_all().await.expect("read"), b"bc");
    assert_eq!(counter("__inputDialogs"), Some(1.0));
}

/// Dismissing the dialog is a cancel — even where the File System Access API
/// exists — and opens no second dialog.
#[wasm_bindgen_test]
async fn input_cancel_with_fsa_present_is_cancelled_once() {
    install_pickers("[]");
    let out = FilePicker::new().pick(PickRequest::documents(["text/plain"])).await.expect("pick");
    assert!(matches!(out, PickOutcome::Cancelled));
    assert_eq!(counter("__fsaCalls"), Some(0.0));
    assert_eq!(counter("__inputDialogs"), Some(1.0));
}
