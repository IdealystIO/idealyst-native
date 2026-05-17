//! `Primitive::ScrollView` — a `<div>` with `overflow: auto` on the
//! requested axis.

use crate::WebBackend;
use framework_core::primitives::scroll_view::{ScrollViewHandle, ScrollViewOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, horizontal: bool) -> Node {
    let div = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    // No `.ui-default` class — see view.rs for the rationale.
    // ScrollView's only fixed layout is the overflow we set inline
    // below; children stack via normal block flow unless the user's
    // style on the ScrollView itself opts into flex.
    // Apply the overflow style inline (not via the framework's style
    // system) so it's always present regardless of user-supplied
    // styling. The inline rules win over class rules for the overflow
    // properties; the class still governs flex direction etc.
    let overflow = if horizontal {
        "overflow-x: auto; overflow-y: hidden"
    } else {
        "overflow-y: auto; overflow-x: hidden"
    };
    let _ = div.set_attribute("style", overflow);
    div.unchecked_into::<Node>()
}

pub(crate) fn make_handle(node: &Node) -> ScrollViewHandle {
    let el: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("scroll_view node is not an HtmlElement");
    ScrollViewHandle::new(Rc::new(el), &WebScrollViewOps)
}

struct WebScrollViewOps;
impl ScrollViewOps for WebScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.set_scroll_left(x as i32);
            html.set_scroll_top(y as i32);
        }
    }
}
