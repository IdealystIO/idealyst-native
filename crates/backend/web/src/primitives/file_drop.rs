//! OS file drag-and-drop delivery for the web backend.
//!
//! Implements [`runtime_shared::Backend::install_file_drop_handler`] using the
//! HTML5 drag-and-drop events. Four listeners on the subscribed element cover
//! the drag lifecycle:
//!
//! - `dragenter` / `dragover` → [`FileDropPhase::Entered`]. The browser blocks
//!   a `drop` unless the `dragover` handler calls `preventDefault()`, and its
//!   *default* action for a file drop is to navigate the tab to the file — so
//!   we `preventDefault()` whenever the handler accepts (returns
//!   `consumed: true`). `dragover` fires continuously; we re-fire `Entered`.
//! - `dragleave` → [`FileDropPhase::Exited`].
//! - `drop` → [`FileDropPhase::Dropped`], carrying one [`DroppedFile`] per
//!   `DataTransfer.files` entry. The web has no filesystem path, so each
//!   `DroppedFile` has `path: None` and stashes the raw `web_sys::File` in
//!   `source` for the `file-picker` SDK to stream over its `ReadableStream`.
//!
//! We only treat a drag as a *file* drag when `DataTransfer.types` contains
//! `"Files"` — dragging selected text or a link also fires these events, and
//! those must not be swallowed.

use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase, FileDropHandler, TouchPoint};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{DragEvent, Element, Node};

/// Install the drag-and-drop listeners on `node`. The element owns the
/// closures (see [`super::own_listener`]), so they are released with it.
pub(crate) fn install(node: &Node, handler: FileDropHandler) {
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };

    // `dragenter` and `dragover` both map to `Entered`. `dragover` is the one
    // whose `preventDefault()` actually enables the drop, but firing on both
    // keeps the accept decision live as the pointer moves in.
    for event_name in ["dragenter", "dragover"] {
        let h = handler.clone();
        let closure = Closure::<dyn FnMut(DragEvent)>::new(move |ev: DragEvent| {
            if !is_file_drag(&ev) {
                return;
            }
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Entered,
                position: local_position(&ev),
            };
            let response = (h)(&ev_out);
            if response.consumed {
                // Accept the drag: without preventDefault the browser refuses
                // the drop and navigates the tab to the dropped file.
                ev.prevent_default();
            }
        });
        let _ =
            element.add_event_listener_with_callback(event_name, closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }

    // `dragleave` → Exited.
    {
        let h = handler.clone();
        let closure = Closure::<dyn FnMut(DragEvent)>::new(move |ev: DragEvent| {
            if !is_file_drag(&ev) {
                return;
            }
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Exited,
                position: local_position(&ev),
            };
            let _ = (h)(&ev_out);
        });
        let _ =
            element.add_event_listener_with_callback("dragleave", closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }

    // `drop` → Dropped(files).
    {
        let h = handler.clone();
        let closure = Closure::<dyn FnMut(DragEvent)>::new(move |ev: DragEvent| {
            if !is_file_drag(&ev) {
                return;
            }
            // Always prevent the default (navigate-to-file) on an actual drop.
            ev.prevent_default();
            let files = collect_files(&ev);
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Dropped(files),
                position: local_position(&ev),
            };
            let _ = (h)(&ev_out);
        });
        let _ = element.add_event_listener_with_callback("drop", closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }
}

/// True when the drag carries OS files (as opposed to dragged text / a link /
/// an in-page element). `DataTransfer.types` includes `"Files"` in that case.
fn is_file_drag(ev: &DragEvent) -> bool {
    let Some(dt) = ev.data_transfer() else {
        return false;
    };
    let types = dt.types();
    for i in 0..types.length() {
        if types.get(i).as_string().as_deref() == Some("Files") {
            return true;
        }
    }
    false
}

/// Pull the dropped `File`s out of the event into neutral [`DroppedFile`]s.
/// The raw `web_sys::File` rides along in `source` for the SDK to stream.
fn collect_files(ev: &DragEvent) -> Vec<DroppedFile> {
    let Some(dt) = ev.data_transfer() else {
        return Vec::new();
    };
    let Some(list) = dt.files() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.length() as usize);
    for i in 0..list.length() {
        let Some(file) = list.get(i) else { continue };
        let name = file.name();
        let mime = {
            let t = file.type_();
            if t.is_empty() {
                "application/octet-stream".to_string()
            } else {
                t
            }
        };
        let size = Some(file.size() as u64);
        out.push(DroppedFile {
            name,
            mime,
            size,
            path: None,
            source: Some(Rc::new(file) as Rc<dyn std::any::Any>),
        });
    }
    out
}

/// Element-local pointer coordinates: `client` minus the element's rect.
fn local_position(ev: &DragEvent) -> TouchPoint {
    let fallback = || TouchPoint::new(ev.client_x() as f32, ev.client_y() as f32);
    let Some(target) = ev.current_target() else {
        return fallback();
    };
    let el: Element = match target.dyn_into() {
        Ok(e) => e,
        Err(_) => return fallback(),
    };
    let rect = el.get_bounding_client_rect();
    TouchPoint::new(
        ev.client_x() as f32 - rect.x() as f32,
        ev.client_y() as f32 - rect.y() as f32,
    )
}
