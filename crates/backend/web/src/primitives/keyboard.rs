//! App-level keyboard handling + the shared browser→framework key-event
//! translator. Lives OUTSIDE the `prim-text-input` gate: global shortcuts
//! (`set_app_key_handler`) are an app-level capability that must keep
//! working when the text-input primitive is compiled out.

use crate::WebBackend;
use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use wasm_bindgen::closure::Closure;

/// Convert a browser `KeyboardEvent` into the framework's `KeyEvent`. Shared
/// by the per-input listener (`text_input`, gated) and the app-level document
/// listener below; the global path has no input, so it passes `0`/`0` for the
/// selection range.
pub(crate) fn key_event_from(ke: &web_sys::KeyboardEvent, sel_start: usize, sel_end: usize) -> KeyEvent {
    KeyEvent {
        key: ke.key(),
        shift: ke.shift_key(),
        ctrl: ke.ctrl_key(),
        alt: ke.alt_key(),
        meta: ke.meta_key(),
        selection_start: sel_start,
        selection_end: sel_end,
    }
}

/// Install (or, with `None`, remove) the APP-LEVEL `keydown` listener on
/// `document` — it fires for every key press regardless of focus, routing each
/// through `handler`. Mirrors the per-input path but at the document level, so
/// app shortcuts work without a focused input. Replacing removes the prior
/// listener first; `None` removes + drops it.
pub(crate) fn install_app_key_handler(b: &mut WebBackend, handler: Option<KeyDownHandler>) {
    use wasm_bindgen::JsCast as _;
    // Tear down any existing global listener.
    if let Some(prev) = b._app_key_closure.take() {
        let _ = b
            .doc
            .remove_event_listener_with_callback("keydown", prev.as_ref().unchecked_ref());
        // `prev` drops here, freeing the JS closure.
    }
    let Some(handler) = handler else {
        return;
    };
    let closure = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ke: web_sys::KeyboardEvent| {
        let event = key_event_from(&ke, 0, 0);
        if handler(&event) == KeyOutcome::PreventDefault {
            ke.prevent_default();
        }
    });
    let _ = b
        .doc
        .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
    b._app_key_closure = Some(closure);
}
