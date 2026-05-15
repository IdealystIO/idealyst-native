//! `Primitive::Image` — an `<img>` with reactive `src`.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, src: &str, alt: Option<&str>) -> Node {
    let img = b
        .doc
        .create_element("img")
        .expect("create_element img failed");
    let _ = img.set_attribute("src", src);
    if let Some(a) = alt {
        let _ = img.set_attribute("alt", a);
    }
    img.unchecked_into::<Node>()
}

pub(crate) fn update_src(node: &Node, src: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = el.set_attribute("src", src);
    }
}
