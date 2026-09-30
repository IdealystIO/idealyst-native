//! App-level keyboard handling + the shared browser→framework key-event
//! translator. Lives OUTSIDE the `prim-text-input` gate: global shortcuts
//! (`set_app_key_handler`) are an app-level capability that must keep
//! working when the text-input primitive is compiled out.

use crate::WebBackend;
use runtime_shared::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};

/// Convert a browser `KeyboardEvent` into the framework's `KeyEvent`. Shared
/// by the per-input listener (`text_input`, gated) and the app-level document
/// listener below; the global path has no input, so it passes `0`/`0` for the
/// selection range.
pub(crate) fn key_event_from(ke: &web_glue::dom::KeyboardEvent, sel_start: usize, sel_end: usize) -> KeyEvent {
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
    // Tear down any existing global listener (the `Listener` detaches as
    // it drops).
    b._app_key_closure = None;
    let Some(handler) = handler else {
        return;
    };
    b._app_key_closure = Some(crate::glue_dom::listen(
        &b.doc,
        "keydown",
        web_glue::dom::ListenerOptions::default(),
        move |e| {
            let ke: web_glue::dom::KeyboardEvent = web_glue::JsCast::unchecked_into(e);
            let event = key_event_from(&ke, 0, 0);
            if handler(&event) == KeyOutcome::PreventDefault {
                ke.prevent_default();
            }
        },
    ));
}
