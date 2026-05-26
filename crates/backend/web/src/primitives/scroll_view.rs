//! `Primitive::ScrollView` — a `<div>` with `overflow: auto` on the
//! requested axis.

use crate::WebBackend;
use runtime_core::primitives::scroll_view::{ScrollViewHandle, ScrollViewOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    horizontal: bool,
    on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
) -> Node {
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

    // Wire `on_scroll`. The callback receives CSS-pixel offsets
    // (`scrollLeft`/`scrollTop`) directly \u{2014} same units the
    // framework already uses for layout, so author code doesn't
    // need to translate.
    //
    // `Closure<dyn Fn>` rather than `FnMut` so wasm-bindgen doesn't
    // emit the `FnMut` runtime recursion guard. The author's callback
    // may write a signal whose subscribers can mutate layout in ways
    // that synchronously re-fire `scroll`; the guard would reject
    // that as recursive even though the second call is benign.
    //
    // `.forget()` leaks the Closure so JS can keep invoking it for
    // the lifetime of the element. We trade per-ScrollView leakage
    // (one Closure object) for never holding a dangling function
    // ref on the DOM listener side \u{2014} which would crash the
    // page with a "closure invoked after being dropped" throw.
    if let Some(cb) = on_scroll {
        let element_for_handler = div.clone();
        let scroll_handler: Closure<dyn Fn(web_sys::Event)> =
            Closure::wrap(Box::new(move |_evt: web_sys::Event| {
                if let Some(html) = element_for_handler.dyn_ref::<web_sys::HtmlElement>() {
                    let x = html.scroll_left() as f32;
                    let y = html.scroll_top() as f32;
                    cb(x, y);
                }
            }));
        let _ = div.add_event_listener_with_callback(
            "scroll",
            scroll_handler.as_ref().unchecked_ref(),
        );
        scroll_handler.forget();
    }

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
