//! `Primitive::TextInput` — an `<input type="text">` with a controlled
//! value signal and a per-keystroke `on_change` callback.

use crate::WebBackend;
use framework_core::primitives::text_input::{TextInputHandle, TextInputOps};
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
) -> Node {
    let input: web_sys::HtmlInputElement = b
        .doc
        .create_element("input")
        .expect("create_element input failed")
        .unchecked_into();
    input.set_type("text");
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
    input.unchecked_into::<Node>()
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
}
