//! `Backend::mark_preserves_focus` — the web half of the framework's
//! focus-preserving press region (see `runtime_shared::backend`).
//!
//! In browsers, moving keyboard focus is the *default action* of
//! `mousedown` — and per the Pointer Events spec, canceling `pointerdown`
//! suppresses the compatibility mouse events (and with them the focus
//! move), while `click` still fires. So one capture-phase `pointerdown`
//! listener calling `preventDefault()` makes every press inside the
//! marked subtree focus-neutral: a click on an option row in a combobox
//! menu commits without blurring the input that anchors the menu.
//!
//! CAPTURE phase is load-bearing: pressables install a bubble-phase
//! `pointerdown` listener that calls `stopPropagation()` (the
//! ancestor-touch swallow, see `pressable.rs` / `touch.rs`). A bubble
//! listener on the marked ancestor would never see a press that starts
//! on a row; the capture listener runs before any bubble-phase
//! propagation control can cancel it.
//!
//! Side effects of the canceled default inside the marked subtree —
//! no text selection by drag, no `:active` via compat mouse events —
//! are acceptable for the popover/adornment surfaces this is meant for
//! (their press feedback is signal-driven, not UA-driven).

use crate::WebBackend;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn mark(b: &mut WebBackend, node: &Node) {
    let el: web_sys::Element = match node.clone().dyn_into() {
        Ok(e) => e,
        Err(_) => return,
    };
    let id = b.node_id(node);
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        ev.prevent_default();
    });
    // `..._and_bool(true)` = useCapture.
    let _ = el.add_event_listener_with_callback_and_bool(
        "pointerdown",
        closure.as_ref().unchecked_ref(),
        true,
    );
    b.state_listeners.entry(id).or_default().push(closure);
}
