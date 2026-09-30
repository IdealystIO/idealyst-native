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
//!   `DroppedFile` has `path: None` and stashes the raw file in `source` (as a
//!   `web_glue::dom::File`) for the `file-picker` SDK to stream over its
//!   `ReadableStream`.
//!
//! We only treat a drag as a *file* drag when `DataTransfer.types` contains
//! `"Files"` — dragging selected text or a link also fires these events, and
//! those must not be swallowed.

use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase, FileDropHandler, TouchPoint};
use std::rc::Rc;
use web_glue::JsCast;
use web_glue::dom::DragEvent;
use web_glue::dom::{Element, Node};

/// Install the drag-and-drop listeners on `node`. The element owns the
/// closures (`glue_dom::listen_for_element_lifetime`), so they are released with it.
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
        let el_for_rect = element.clone();
        crate::glue_dom::listen_for_element_lifetime(&element, event_name, Default::default(), move |ev| {
            let ev: web_glue::dom::DragEvent = web_glue::JsCast::unchecked_into(ev);
            if !is_file_drag(&ev) {
                return;
            }
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Entered,
                position: local_position(&el_for_rect, &ev),
            };
            let response = (h)(&ev_out);
            if response.consumed {
                // Accept the drag: without preventDefault the browser refuses
                // the drop and navigates the tab to the dropped file.
                ev.prevent_default();
            }
        });
    }

    // `dragleave` → Exited.
    {
        let h = handler.clone();
        let el_for_rect = element.clone();
        crate::glue_dom::listen_for_element_lifetime(&element, "dragleave", Default::default(), move |ev| {
            let ev: web_glue::dom::DragEvent = web_glue::JsCast::unchecked_into(ev);
            if !is_file_drag(&ev) {
                return;
            }
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Exited,
                position: local_position(&el_for_rect, &ev),
            };
            let _ = (h)(&ev_out);
        });
    }

    // `drop` → Dropped(files).
    {
        let h = handler.clone();
        let el_for_rect = element.clone();
        crate::glue_dom::listen_for_element_lifetime(&element, "drop", Default::default(), move |ev| {
            let ev: web_glue::dom::DragEvent = web_glue::JsCast::unchecked_into(ev);
            if !is_file_drag(&ev) {
                return;
            }
            // Always prevent the default (navigate-to-file) on an actual drop.
            ev.prevent_default();
            let files = collect_files(&ev);
            let ev_out = FileDropEvent {
                phase: FileDropPhase::Dropped(files),
                position: local_position(&el_for_rect, &ev),
            };
            let _ = (h)(&ev_out);
        });
    }
}

/// True when the drag carries OS files (as opposed to dragged text / a link /
/// an in-page element). `DataTransfer.types` includes `"Files"` in that case.
fn is_file_drag(ev: &DragEvent) -> bool {
    let Some(dt) = ev.data_transfer() else {
        return false;
    };
    dt.types().iter().any(|t| t == "Files")
}

/// Pull the dropped `File`s out of the event into neutral [`DroppedFile`]s.
/// The raw file rides along in `source` for the SDK to stream.
pub(crate) fn collect_files(ev: &DragEvent) -> Vec<DroppedFile> {
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
            // The file-picker SDK's `picked_from_dropped` downcasts this to a
            // `web_glue::dom::File`.
            source: Some(Rc::new(file) as Rc<dyn std::any::Any>),
        });
    }
    out
}

/// Element-local pointer coordinates: `client` minus the rect of `el`, the
/// element the listener is on (the event's `currentTarget`).
fn local_position(el: &Element, ev: &DragEvent) -> TouchPoint {
    let rect = el.get_bounding_client_rect();
    TouchPoint::new(
        ev.client_x() as f32 - rect.x() as f32,
        ev.client_y() as f32 - rect.y() as f32,
    )
}
