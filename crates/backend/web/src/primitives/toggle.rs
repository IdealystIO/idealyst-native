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
    // Cursor-aware: adopt the SSR-rendered `<input>` if present so the
    // walker doesn't mint a second checkbox alongside it. Without this,
    // SSR's `<input type="checkbox">` stays in the DOM and the fresh
    // one is inserted as a sibling — the user sees two checkboxes.
    // Same pattern as `create_element`: adopt via `hydrate_next`, fall
    // back to a fresh `create_element` (recorded via `hydrate_note_fresh`
    // so the framework can armor a subtree remount if the cursor was
    // misaligned).
    let input: web_sys::HtmlInputElement = if let Some(el) = b.hydrate_next("input") {
        el.unchecked_into()
    } else {
        let el: web_sys::HtmlInputElement = b
            .doc
            .create_element("input")
            .expect("create_element input failed")
            .unchecked_into();
        b.hydrate_note_fresh(&el.clone().unchecked_into::<Node>());
        el
    };
    // Idempotent — SSR already sets these, but a freshly-built path
    // needs them and the writes are no-ops on an adopted node.
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
    // Consume the press from ancestor `on_touch` recognizers, exactly as
    // `button` / `link` / `pressable` do — a toggle inside a clickable row
    // must not ALSO trigger the row, which is native's single-view delivery.
    //
    // For a checkbox this is load-bearing beyond the double-fire: a checkbox
    // activates on the synthesized `click`, and `click` is dispatched at the
    // element the pointer was TARGETED at, which an ancestor's
    // `setPointerCapture` moves off the checkbox. Since the ancestor captures
    // as soon as its handler consumes the `Began` (see
    // `touch::install`), an unswallowed checkbox inside a gesture surface
    // stops toggling at all — verified in Chrome, where the `change` event
    // simply never fires. Swallowing the press means the ancestor never
    // consumes it, so it never captures. (A slider and a text field are not
    // exposed the same way: Blink drives a range thumb and caret placement
    // from its own internal capture, both verified unaffected.)
    super::touch::swallow_ancestor_touch(input.as_ref());
    input.unchecked_into::<Node>()
}

pub(crate) fn update_value(node: &Node, value: bool) {
    if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
        if input.checked() != value {
            input.set_checked(value);
        }
    }
}
