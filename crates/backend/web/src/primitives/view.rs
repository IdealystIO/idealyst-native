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

/// Placeholder `<div>` for reactive `when` / `switch` branches.
///
/// The walker creates this as a stable parent that survives across
/// branch swaps. It has no class (no stylesheet is ever attached),
/// so by default the browser treats it as a plain block-level
/// `<div>` — which collapses widths inside a flex column and
/// breaks `flex: 1` / `width: 100%` on the branch's children.
///
/// `display: contents` removes the placeholder from the layout
/// tree entirely: its children are promoted to direct children of
/// the surrounding parent, inheriting the parent's flex / sizing
/// context as if the anchor weren't there. The anchor still exists
/// in the DOM (so the walker can `clear_children` + reinsert on
/// each rebuild), it's just invisible to layout.
pub(crate) fn create_reactive_anchor(b: &mut WebBackend) -> Node {
    let el = b
        .doc
        .create_element("div")
        .expect("create_element failed");
    let _ = el.set_attribute("style", "display: contents");
    el.unchecked_into::<Node>()
}

pub(crate) fn insert(parent: &mut Node, child: Node) {
    // Overlay primitives portal themselves to `<body>` inside
    // `create_overlay`, so the framework's subsequent
    // `insert(surrounding_parent, overlay_node)` call would yank
    // the overlay back into the layout tree. The web backend
    // stamps overlay-content containers with
    // `data-overlay-skip-insert`; treat that attribute as a "do
    // not parent me" marker.
    if let Some(el) = child.dyn_ref::<web_sys::Element>() {
        if el.has_attribute("data-overlay-skip-insert") {
            return;
        }
    }
    parent.append_child(&child).expect("append_child failed");
}

/// Batched insertion. Builds a `DocumentFragment`, appends every
/// child to it (wasm→JS one FFI per child but no parent-side
/// layout invalidation per call — the parent only sees one
/// mutation when the fragment is finally appended), then appends
/// the fragment to the parent in a single `append_child` call.
///
/// `DocumentFragment` is a Real Browser Trick: appending a fragment
/// moves its children to the new parent and leaves the fragment
/// empty, all without firing per-child mutation observers or
/// triggering per-child layout reflow on the parent. The wasm→JS
/// boundary cost per child is unchanged, but the parent-side
/// browser work scales O(1) in the number of insertions rather
/// than O(N).
pub(crate) fn insert_many(b: &mut crate::WebBackend, parent: &mut Node, children: Vec<Node>) {
    if children.is_empty() {
        return;
    }
    // Filter out overlay-portaled nodes — they already live under
    // `<body>` and must not be moved into the surrounding parent.
    // See `insert` for the single-child rationale.
    let children: Vec<Node> = children
        .into_iter()
        .filter(|c| {
            c.dyn_ref::<web_sys::Element>()
                .map(|el| !el.has_attribute("data-overlay-skip-insert"))
                .unwrap_or(true)
        })
        .collect();
    if children.is_empty() {
        return;
    }
    if children.len() == 1 {
        // No point paying for the fragment dance for a single child.
        // The repeat path can legitimately hit this when the loop
        // bound is 1 at runtime.
        let mut iter = children.into_iter();
        parent
            .append_child(&iter.next().unwrap())
            .expect("append_child failed");
        return;
    }
    let frag = b
        .doc
        .create_document_fragment()
        .unchecked_into::<Node>();
    for child in children {
        frag.append_child(&child).expect("append_child to fragment failed");
    }
    parent.append_child(&frag).expect("append fragment to parent failed");
}

pub(crate) fn clear_children(node: &Node) {
    while let Some(child) = node.first_child() {
        node.remove_child(&child).expect("remove_child failed");
    }
}
