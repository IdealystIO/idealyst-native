//! Hover (pointer-over) delivery for the web backend.
//!
//! Implements [`runtime_shared::Backend::install_hover_handler`] with the
//! Pointer Events API. The element gets two listeners — `pointerenter`
//! and `pointerleave` — which fire the framework's handler with `true`
//! and `false` respectively.
//!
//! We deliberately use `pointerenter`/`pointerleave` (NOT
//! `pointerover`/`pointerout`): the enter/leave pair does **not** bubble
//! and does not re-fire as the pointer crosses into descendant elements,
//! so a single enter and a single leave bracket the whole time the cursor
//! is over the view — exactly the semantics a hover tooltip wants. The
//! over/out pair would spuriously toggle on every child boundary.
//!
//! The element owns its listener closures (see [`super::own_listener`]) —
//! same pattern as touch/wheel.

use runtime_shared::HoverHandler;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Element, Node, PointerEvent};

/// Install `pointerenter` (→ `true`) and `pointerleave` (→ `false`)
/// listeners on `node`.
pub(crate) fn install(node: &Node, handler: HoverHandler) {
    // Hover is only meaningful on real DOM elements; bail silently on text
    // nodes / fragments (mirrors `touch::install`).
    let element: Element = match node.clone().dyn_into::<Element>() {
        Ok(e) => e,
        Err(_) => return,
    };

    // pointerenter → entering = true
    {
        let handler = handler.clone();
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            // Hover is a MOUSE/PEN concept — never touch. On a touch device
            // `pointerenter` fires on touch-DOWN (the finger "enters" the
            // element as it lands), so firing the hover handler here would
            // pop a hover tooltip the instant the user presses, defeating the
            // long-press affordance. Touch goes through the `on_touch`
            // long-press path instead. Mirrors macOS `NSTrackingArea`, which
            // only tracks the mouse.
            if ev.pointer_type() == "touch" {
                return;
            }
            // Born batched via the core `on_hover` cycle wrapper.
            (handler)(true);
        });
        let _ = element
            .add_event_listener_with_callback("pointerenter", closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }

    // pointerleave → entering = false
    {
        let closure = Closure::<dyn FnMut(PointerEvent)>::new(move |ev: PointerEvent| {
            if ev.pointer_type() == "touch" {
                return;
            }
            (handler)(false);
        });
        let _ = element
            .add_event_listener_with_callback("pointerleave", closure.as_ref().unchecked_ref());
        super::own_listener(closure);
    }
}
