//! `Element::Toggle` — an `<input type="checkbox" role="switch">`.

use crate::WebBackend;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    initial_value: bool,
    on_change: Rc<dyn Fn(bool)>,
) -> Node {
    let input: web_sys::HtmlInputElement = b
        .doc
        .create_element("input")
        .expect("create_element input failed")
        .unchecked_into();
    input.set_type("checkbox");
    // role="switch" gives screen readers a switch UX even though
    // the underlying widget is a checkbox.
    let _ = input.set_attribute("role", "switch");
    input.set_checked(initial_value);
    let input_clone = input.clone();
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        on_change(input_clone.checked());
    });
    let _ = input.add_event_listener_with_callback(
        "change",
        closure.as_ref().unchecked_ref(),
    );
    let id = b.node_id(&input.clone().unchecked_into::<Node>());
    b.state_listeners.entry(id).or_default().push(closure);
    input.unchecked_into::<Node>()
}

pub(crate) fn update_value(node: &Node, value: bool) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        if input.checked() != value {
            input.set_checked(value);
        }
    }
}
