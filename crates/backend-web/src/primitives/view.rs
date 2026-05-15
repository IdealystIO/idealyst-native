//! `Primitive::View` — a plain `<div>`. The framework used to stamp
//! every View with a `.ui-default { display: flex; flex-direction:
//! column }` class so authors got React-Native-style flex
//! semantics for free. That class is gone: at 10k+ rows the
//! O(N) flex-container tracking cost in the browser was the
//! single biggest contributor to post-mount layout time.
//!
//! Equivalent semantics now happen at the CSS-emit layer: when a
//! stylesheet sets any flex-container property (`gap`,
//! `flex_direction`, `align_items`, etc.), `rules_to_css`
//! auto-promotes the rule to `display: flex`. Views without flex
//! props are plain blocks — cheap.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend) -> Node {
    let el = b
        .doc
        .create_element("div")
        .expect("create_element failed");
    el.unchecked_into::<Node>()
}

pub(crate) fn insert(parent: &mut Node, child: Node) {
    parent.append_child(&child).expect("append_child failed");
}

pub(crate) fn clear_children(node: &Node) {
    while let Some(child) = node.first_child() {
        node.remove_child(&child).expect("remove_child failed");
    }
}
