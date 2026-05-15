//! `Primitive::Button` — a `<button>` element plus a click closure
//! kept alive in `WebBackend::_click_closures`.

use crate::WebBackend;
use framework_core::{ButtonHandle, ButtonOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, label: &str, on_click: Rc<dyn Fn()>) -> Node {
    let button = b
        .doc
        .create_element("button")
        .expect("create button")
        .unchecked_into::<web_sys::HtmlElement>();
    button.set_text_content(Some(label));
    let closure = Closure::<dyn FnMut()>::new(move || on_click());
    button.set_onclick(Some(closure.as_ref().unchecked_ref()));
    b._click_closures.push(closure);
    button.unchecked_into::<Node>()
}

/// The node was created via `create_element("button")` then upcast to
/// `Node`. Cast it back to `HtmlElement` so the ops table can call
/// `.click()` on it. The clone is cheap — it's a wasm-bindgen JsValue
/// clone (refcount bump on the JS object handle, no DOM duplication).
pub(crate) fn make_handle(node: &Node) -> ButtonHandle {
    let html: web_sys::HtmlElement = node
        .clone()
        .dyn_into()
        .expect("button node is not an HtmlElement");
    ButtonHandle::new(Rc::new(html), &WebButtonOps)
}

/// `ButtonOps` impl for the web backend. The `node` parameter comes
/// from the `ButtonHandle`'s internal `Rc<dyn Any>`, which we built
/// out of an `HtmlElement` in `make_handle`. Downcast back to invoke
/// the DOM API.
struct WebButtonOps;
impl ButtonOps for WebButtonOps {
    fn click(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.click();
        }
    }

    fn rect(&self, node: &dyn Any) -> framework_core::ViewportRect {
        node.downcast_ref::<web_sys::HtmlElement>()
            .map(measure_element_rect)
            .unwrap_or_default()
    }
}

fn measure_element_rect(el: &web_sys::HtmlElement) -> framework_core::ViewportRect {
    let r = el.get_bounding_client_rect();
    framework_core::ViewportRect {
        x: r.x() as f32,
        y: r.y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    }
}
