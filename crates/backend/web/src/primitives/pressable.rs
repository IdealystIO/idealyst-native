//! `Element::Pressable` — a `<div>` with a click handler attached.
//!
//! Unlike `Element::Button`, this is a bare container: no
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
use runtime_shared::{PressableHandle, PressableOps, ViewportRect};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, on_click: Rc<dyn Fn()>) -> Node {
    // HYDRATION: adopt the SSR `<div role=button>` (role/tabindex already
    // set by the SSR `create_pressable`); just wire the handlers below.
    // Its children are adopted separately via cursor descent.
    let adopted = b.hydrate_next("div");
    let el: web_sys::HtmlElement = match adopted {
        Some(el) => el.unchecked_into(),
        None => {
            let el = b
                .doc
                .create_element("div")
                .expect("create pressable")
                .unchecked_into::<web_sys::HtmlElement>();
            // Accessibility: announce as a button + make it Tab-focusable.
            // No inline `cursor` — that's now an author/component-driven
            // style property (`StyleRules::cursor`), so a bare pressable
            // takes the platform default and an author's `cursor` is never
            // shadowed by an un-overridable inline rule.
            let _ = el.set_attribute("role", "button");
            let _ = el.set_attribute("tabindex", "0");
            el
        }
    };
    // Register as a subtree-local remount root if fresh-after-mismatch
    // (no-op when adopted).
    {
        let node: Node = el.clone().unchecked_into();
        b.hydrate_note_fresh(&node);
    }

    let on_click_for_mouse = on_click.clone();
    crate::glue_dom::set_onclick(&el, move || (on_click_for_mouse)());

    // Keyboard activation. Enter and Space both trigger the press
    // when the element has focus — matching what a real `<button>`
    // does so users on assistive tech / keyboard nav get the same
    // affordance.
    //
    // The listener is bubble-phase, so a keydown originating on a focused
    // DESCENDANT (e.g. a `text_input` inside a Modal, whose card layer is a
    // no-op pressable) bubbles up here too. We must only activate when the
    // pressable ITSELF is the key target — otherwise `prevent_default()` on
    // Space/Enter would cancel the descendant input's character insertion /
    // submit. See `regression_web_pressable_ignores_descendant_key`.
    let on_click_for_key = on_click.clone();
    let el_for_key: Node = el.clone().unchecked_into();
    let el_for_key = crate::glue_dom::target(&el_for_key);
    // Element-lifetime: the element owns the listener, and its closure is
    // released when JS collects the element.
    crate::glue_dom::listen_for_element_lifetime(&el, "keydown", Default::default(), move |ev| {
        let ev: web_glue::dom::KeyboardEvent = web_glue::JsCast::unchecked_into(ev);
        let is_self = ev
            .target()
            .map(|t| t.as_js().strict_eq(el_for_key.as_js()))
            .unwrap_or(false);
        if !is_self {
            return;
        }
        let k = ev.key();
        if k == "Enter" || k == " " {
            ev.prevent_default();
            (on_click_for_key)();
        }
    });

    // Consume the press from ancestor `on_touch` recognizers so a pressable
    // inside a clickable row / tappable card doesn't ALSO trigger the
    // ancestor — matching native's single-view touch delivery. See
    // `touch::swallow_ancestor_touch`.
    super::touch::swallow_ancestor_touch(el.as_ref());

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
