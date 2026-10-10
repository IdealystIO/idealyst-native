//! App-level keyboard handling + the shared browser→framework key-event
//! translator. Lives OUTSIDE the `prim-text-input` gate: global shortcuts
//! (`set_keyboard_sink`) are an app-level capability that must keep
//! working when the text-input primitive is compiled out.

use crate::WebBackend;
use runtime_shared::primitives::key::{AppKeyEvent, KeyEvent, KeyOutcome, KeyPhase, KeyboardSink};

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

/// Build the framework's [`AppKeyEvent`] from a browser `KeyboardEvent`. The
/// DOM already speaks the vocabulary the framework normalizes to (`key` and
/// `code` ARE the Web spec values), so this is a field copy.
pub(crate) fn app_key_event_from(ke: &web_glue::dom::KeyboardEvent, phase: KeyPhase) -> AppKeyEvent {
    AppKeyEvent {
        phase,
        key: ke.key(),
        code: ke.code(),
        repeat: phase == KeyPhase::Down && ke.repeat(),
        shift: ke.shift_key(),
        ctrl: ke.ctrl_key(),
        alt: ke.alt_key(),
        meta: ke.meta_key(),
    }
}

/// Install (or, with `None`, remove) the APP-LEVEL key source: `keydown` and
/// `keyup` listeners on `document` — they fire for every key regardless of
/// focus (the events bubble from a focused input) — plus a window `blur`
/// listener that reports focus loss. The blur matters for games: switch
/// tabs or alt-tab while holding a key and the browser never sends that
/// key's `keyup` to this page, so without it the key would stay "held".
/// Replacing removes the prior listeners first; `None` removes + drops them.
pub(crate) fn install_keyboard_sink(b: &mut WebBackend, sink: Option<KeyboardSink>) {
    // Tear down any existing listeners (each `Listener` detaches as it drops).
    b._app_key_listeners.clear();
    let Some(sink) = sink else {
        return;
    };
    for (ty, phase) in [("keydown", KeyPhase::Down), ("keyup", KeyPhase::Up)] {
        let sink = sink.clone();
        b._app_key_listeners.push(crate::glue_dom::listen(
            &b.doc,
            ty,
            web_glue::dom::ListenerOptions::default(),
            move |e| {
                let ke: web_glue::dom::KeyboardEvent = web_glue::JsCast::unchecked_into(e);
                let event = app_key_event_from(&ke, phase);
                // PreventDefault on keydown stops typing / page scroll
                // (Space, arrows); on keyup it is harmless.
                if sink.key(&event) == KeyOutcome::PreventDefault {
                    ke.prevent_default();
                }
            },
        ));
    }
    if let Some(win) = web_glue::dom::window() {
        b._app_key_listeners.push(crate::glue_dom::listen(
            &win,
            "blur",
            web_glue::dom::ListenerOptions::default(),
            move |_| sink.focus_lost(),
        ));
    }
}
