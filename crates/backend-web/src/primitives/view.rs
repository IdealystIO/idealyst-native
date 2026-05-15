//! `Primitive::View` — a styled `<div>`. The framework's flex-column
//! default reaches every view via the `.ui-default` baseline class
//! stamped in `apply_default_class`.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend) -> Node {
    let el = b
        .doc
        .create_element("div")
        .expect("create_element failed");
    b.apply_default_class(&el);
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
