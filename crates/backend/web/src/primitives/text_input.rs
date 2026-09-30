//! `Element::TextInput` — an `<input type="text">` with a controlled
//! value signal and a per-keystroke `on_change` callback.

use crate::WebBackend;
use runtime_shared::primitives::key::{KeyDownHandler, KeyOutcome};
use runtime_shared::primitives::text_input::{
    BlurHandler, BlurOutcome, TextInputHandle, TextInputOps,
};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<KeyDownHandler>,
    on_blur: Option<BlurHandler>,
    secure: bool,
) -> Node {
    // Hydration adoption: reuse the SSR `<input>` if the cursor is on
    // a matching tag. Without this, the walker would build a fresh
    // input next to the SSR one and the divergence cascade leaves
    // both in the DOM. Even a leaf input must register with the
    // adoption cursor or every sibling element after it desyncs.
    let input: web_sys::HtmlInputElement = if let Some(el) = b.hydrate_next("input") {
        el.unchecked_into()
    } else {
        let fresh: web_sys::HtmlInputElement = b
            .doc
            .create_element("input")
            .expect("create_element input failed")
            .unchecked_into();
        let node: Node = fresh.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
        fresh
    };
    input.set_type(if secure { "password" } else { "text" });
    input.set_value(initial_value);
    if let Some(p) = placeholder {
        input.set_placeholder(p);
    }
    // Wire native `input` event to the Rust callback. We use
    // `input` rather than `change` so every keystroke fires —
    // matching the controlled-component "single source of truth"
    // expectation.
    let input_clone = input.clone();
    // Tracked under the node id so it lives as long as the node does and
    // detaches at teardown (`state_listeners`).
    let id = b.node_id(&input.clone().unchecked_into::<Node>());
    b.track_listener(id, &input, "input", false, move |_| {
        on_change(input_clone.value());
    });
    if let Some(handler) = on_key_down {
        attach_key_listener_input(&input, id, b, handler);
    }
    // Cancelable blur: `blur` isn't preventable per spec, so when the handler
    // returns `Keep` we synchronously re-`focus()` to retain focus (one frame
    // of flicker — the honest platform limitation; iOS/macOS veto natively).
    if let Some(blur_handler) = on_blur {
        let input_for_blur = input.clone();
        b.track_listener(id, &input, "blur", false, move |_| {
            if blur_handler() == BlurOutcome::Keep {
                let _ = input_for_blur.focus();
            }
        });
    }
    input.unchecked_into::<Node>()
}

/// Install the author `on_focus` notifier on an existing input/textarea `node`:
/// DOM `focus` → `handler(true)`, `blur` → `handler(false)`. Backs
/// `Backend::set_text_input_focus_handler`. The closures are stashed under the
/// node id so they live as long as the node. Used by the idea-ui `Field` to
/// light its bordered shell's ring for an adorned (borderless-input) layout —
/// the bare `<input>`'s own `:focus` can't style the wrapping shell `<div>`.
pub(crate) fn set_focus_handler(b: &mut WebBackend, node: &Node, handler: Rc<dyn Fn(bool)>) {
    let el: web_sys::HtmlElement = match node.clone().dyn_into() {
        Ok(e) => e,
        Err(_) => return,
    };
    let id = b.node_id(node);
    let on_focus = handler.clone();
    b.track_listener(id, &el, "focus", false, move |_| on_focus(true));
    b.track_listener(id, &el, "blur", false, move |_| handler(false));
}

/// Wire a DOM `keydown` listener that calls the Rust `KeyDownHandler`
/// with the same `KeyEvent` shape used by every other backend, and
/// calls `event.preventDefault()` when the handler returns
/// `KeyOutcome::PreventDefault`. Kept as a free function so both
/// `text_input::create` and `text_area::create` can call it without
/// monomorphising over the element type.
///
/// The listener receives a plain glue `Event`; it is checked into a
/// `KeyboardEvent` inside.
pub(crate) fn attach_key_listener_input(
    input: &web_sys::HtmlInputElement,
    id: u32,
    b: &mut WebBackend,
    handler: KeyDownHandler,
) {
    let input_clone = input.clone();
    b.track_listener(id, input, "keydown", false, move |e: web_glue::dom::Event| {
        if let Ok(ke) = web_glue::JsCast::dyn_into::<web_glue::dom::KeyboardEvent>(e) {
            let sel_start = input_clone.selection_start().ok().flatten().unwrap_or(0) as usize;
            let sel_end = input_clone.selection_end().ok().flatten().unwrap_or(0) as usize;
            let event = key_event_from(&ke, sel_start, sel_end);
            if handler(&event) == KeyOutcome::PreventDefault {
                ke.prevent_default();
            }
        }
    });
}

pub(crate) use super::keyboard::key_event_from;


pub(crate) fn update_value(node: &Node, value: &str) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        // Only write if different — avoids cursor-jump artifacts
        // when our own on_change wrote back to the signal.
        if input.value() != value {
            input.set_value(value);
        }
    }
}

pub(crate) fn update_secure(node: &Node, secure: bool) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        // Swap the input type to toggle masking. Browsers preserve the
        // value across a type change; guard against a needless write so a
        // no-op toggle doesn't perturb the field.
        let want = if secure { "password" } else { "text" };
        if input.type_() != want {
            input.set_type(want);
        }
    }
}

pub(crate) fn update_placeholder(node: &Node, placeholder: Option<&str>) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        input.set_placeholder(placeholder.unwrap_or(""));
    }
}

pub(crate) fn make_handle(node: &Node) -> TextInputHandle {
    let input: web_sys::HtmlInputElement = node
        .clone()
        .dyn_into()
        .expect("text_input node is not an HtmlInputElement");
    TextInputHandle::new(Rc::new(input), &WebTextInputOps)
}

struct WebTextInputOps;
impl TextInputOps for WebTextInputOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            let _ = input.focus();
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            let _ = input.blur();
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            input.select();
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            // Splice `text` into the active selection. `setRangeText`
            // is the modern API that does this in one call and
            // dispatches the implicit `input` event the framework's
            // change wire-up listens for, so the controlling Signal
            // updates without us touching `.value` directly.
            let start = input.selection_start().ok().flatten().unwrap_or(0);
            let end = input.selection_end().ok().flatten().unwrap_or(start);
            let _ = input.set_range_text_with_start_and_end(text, start, end);
            // Dispatch an `input` event so the on_change closure
            // wired in `create()` runs and Signals get notified.
            if let Ok(event) = web_sys::Event::new("input") {
                let _ = input.dispatch_event(&event);
            }
        }
    }
}
