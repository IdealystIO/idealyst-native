//! `Primitive::Text` — a `<span>` so style application via `class`
//! works uniformly. A raw DOM text node has no `class`/`style`
//! attributes, so styling would be silently dropped.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, content: &str) -> Node {
    let span = b
        .doc
        .create_element("span")
        .expect("create_element span failed");
    span.set_text_content(Some(content));
    span.unchecked_into::<Node>()
}

pub(crate) fn update_text(node: &Node, content: &str) {
    // Works for both Element (e.g. our <span>) and Text node cases.
    node.set_text_content(Some(content));
}
