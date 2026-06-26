//! `Element::TextInput` — an `<input type="text">` with a controlled
//! value signal and a per-keystroke `on_change` callback.

use crate::WebBackend;
use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use runtime_core::primitives::text_input::{
    BlurHandler, BlurOutcome, TextInputHandle, TextInputOps,
};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
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
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        on_change(input_clone.value());
    });
    let _ = input.add_event_listener_with_callback(
        "input",
        closure.as_ref().unchecked_ref(),
    );
    // Stash closure under a fresh node id so it lives as long as
    // the node does. Reuse `state_listeners` map since it's the
    // existing per-node closure holder.
    let id = b.node_id(&input.clone().unchecked_into::<Node>());
    b.state_listeners.entry(id).or_default().push(closure);
    if let Some(handler) = on_key_down {
        attach_key_listener_input(&input, id, b, handler);
    }
    // Cancelable blur: `blur` isn't preventable per spec, so when the handler
    // returns `Keep` we synchronously re-`focus()` to retain focus (one frame
    // of flicker — the honest platform limitation; iOS/macOS veto natively).
    if let Some(blur_handler) = on_blur {
        let input_for_blur = input.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            if blur_handler() == BlurOutcome::Keep {
                let _ = input_for_blur.focus();
            }
        });
        let _ = input
            .add_event_listener_with_callback("blur", closure.as_ref().unchecked_ref());
        b.state_listeners.entry(id).or_default().push(closure);
    }
    input.unchecked_into::<Node>()
}

/// Wire a DOM `keydown` listener that calls the Rust `KeyDownHandler`
/// with the same `KeyEvent` shape used by every other backend, and
/// calls `event.preventDefault()` when the handler returns
/// `KeyOutcome::PreventDefault`. Kept as a free function so both
/// `text_input::create` and `text_area::create` can call it without
/// monomorphising over the element type.
///
/// The closure stored under `state_listeners` is typed
/// `FnMut(web_sys::Event)` to match the existing map's value type;
/// we `dyn_into` to `KeyboardEvent` inside.
pub(crate) fn attach_key_listener_input(
    input: &web_sys::HtmlInputElement,
    id: u32,
    b: &mut WebBackend,
    handler: KeyDownHandler,
) {
    let input_clone = input.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        if let Ok(ke) = e.dyn_into::<web_sys::KeyboardEvent>() {
            let sel_start = input_clone.selection_start().ok().flatten().unwrap_or(0) as usize;
            let sel_end = input_clone.selection_end().ok().flatten().unwrap_or(0) as usize;
            let event = key_event_from(&ke, sel_start, sel_end);
            if handler(&event) == KeyOutcome::PreventDefault {
                ke.prevent_default();
            }
        }
    });
    let _ = input.add_event_listener_with_callback(
        "keydown",
        closure.as_ref().unchecked_ref(),
    );
    b.state_listeners.entry(id).or_default().push(closure);
}

/// Convert a browser `KeyboardEvent` into the framework's `KeyEvent`. Shared by
/// the per-input listener and the app-level document listener
/// ([`install_app_key_handler`]); the global path has no input, so it passes
/// `0`/`0` for the selection range.
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
