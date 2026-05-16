//! `Primitive::Pressable` — a `<div>` with a click handler attached.
//!
//! Unlike `Primitive::Button`, this is a bare container: no
//! `<button>` element, no UA chrome (no outset border, no system
//! font, no implicit `type=submit`). Everything visual comes from
//! the attached stylesheet + children. The framework's state-bit
//! machinery (`state hovered`, `state pressed`, `state focused`)
//! works through CSS pseudo-classes, which apply to any element
//! including `<div>`.
//!
//! We DO add `role="button"` and `tabindex="0"` so the element is
//! a keyboard-reachable button for assistive tech.

use crate::WebBackend;
use framework_core::{PressableHandle, PressableOps, ViewportRect};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, on_click: Rc<dyn Fn()>) -> Node {
    let el = b
        .doc
        .create_element("div")
        .expect("create pressable")
        .unchecked_into::<web_sys::HtmlElement>();
    // Accessibility: announce as a button to assistive tech and
    // make it focusable via Tab. Keyboard activation (Enter/Space)
    // is wired below.
    let _ = el.set_attribute("role", "button");
    let _ = el.set_attribute("tabindex", "0");
    // Hand cursor — every clickable element on the web should have
    // one, and `cursor` isn't in the framework's styled-property
    // model. Set inline at create time; the stylesheet's style
    // attribute writes (which use class-based selectors) compose
    // with this without overwriting.
    let _ = el.style().set_property("cursor", "pointer");

    let on_click_for_mouse = on_click.clone();
    let click_closure = Closure::<dyn FnMut()>::new(move || (on_click_for_mouse)());
    el.set_onclick(Some(click_closure.as_ref().unchecked_ref()));
    b._click_closures.push(click_closure);

    // Keyboard activation. Enter and Space both trigger the press
    // when the element has focus — matching what a real `<button>`
    // does so users on assistive tech / keyboard nav get the same
    // affordance.
    let on_click_for_key = on_click.clone();
    let key_closure: Closure<dyn FnMut(web_sys::KeyboardEvent)> =
        Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
            let k = ev.key();
            if k == "Enter" || k == " " {
                ev.prevent_default();
                (on_click_for_key)();
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
    let _ = el.add_event_listener_with_callback(
        "keydown",
        key_closure.as_ref().unchecked_ref(),
    );
    // Keep the closure alive for the element's lifetime. We piggy-
    // back on `_click_closures` since it has the same disposal
    // posture (cleared on backend drop) — different type but
    // erased the same way through wasm-bindgen's handle table.
    b._pressable_key_closures.push(key_closure);

    el.unchecked_into::<Node>()
}

pub(crate) fn make_handle(node: &Node) -> PressableHandle {
    let html: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("pressable node is not an HtmlElement");
    PressableHandle::new(Rc::new(html), &WebPressableOps)
}

struct WebPressableOps;
impl PressableOps for WebPressableOps {
    fn click(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.click();
        }
    }

    fn rect(&self, node: &dyn Any) -> ViewportRect {
        node.downcast_ref::<web_sys::HtmlElement>()
            .map(measure_element_rect)
            .unwrap_or_default()
    }
}

fn measure_element_rect(el: &web_sys::HtmlElement) -> ViewportRect {
    let r = el.get_bounding_client_rect();
    ViewportRect {
        x: r.x() as f32,
        y: r.y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    }
}
