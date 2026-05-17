//! `Primitive::WebView` — an `<iframe>` with reactive `src`.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, url: &str) -> Node {
    let iframe = b
        .doc
        .create_element("iframe")
        .expect("create_element iframe failed");
    let _ = iframe.set_attribute("src", url);
    // Minimal default styling: take a sensible size; authors can
    // override via .with_style(...).
    let _ = iframe.set_attribute("style", "width: 100%; height: 400px; border: 0");
    iframe.unchecked_into::<Node>()
}

pub(crate) fn update_url(node: &Node, url: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = el.set_attribute("src", url);
    }
}
