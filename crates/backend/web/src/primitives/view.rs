//! `Element::View` — a plain `<div>`. The framework used to stamp
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
    if let Some(el) = b.hydrate_next("div") {
        return el.unchecked_into::<Node>();
    }
    let el = b
        .doc
        .create_element("div")
        .expect("create_element failed");
    let node: Node = el.unchecked_into();
    b.hydrate_note_fresh(&node);
    node
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
    if let Some(el) = b.hydrate_next("div") {
        // SSR already stamped `display: contents`; adopt as-is.
        return el.unchecked_into::<Node>();
    }
    let el = b
        .doc
        .create_element("div")
        .expect("create_element failed");
    let _ = el.set_attribute("style", css::REACTIVE_ANCHOR_STYLE);
    let node: Node = el.unchecked_into();
    b.hydrate_note_fresh(&node);
    node
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

/// Splice one node into `parent` at child index `index` — the
/// `insert_at` half of child-splicing, used by keyed `Each`
/// reconciliation. `insertBefore` handles BOTH cases the reconciler
/// needs: a node not yet in the DOM is inserted, and a node already
/// elsewhere in `parent` is MOVED to the new position. A reference of
/// `None` (index past the end) appends.
pub(crate) fn insert_at(parent: &mut Node, child: Node, index: usize) {
    // Same overlay guard as `insert`: portaled overlay content lives
    // under `<body>` and must not be yanked into the layout tree.
    if let Some(el) = child.dyn_ref::<web_sys::Element>() {
        if el.has_attribute("data-overlay-skip-insert") {
            return;
        }
    }
    // `child_nodes()` counts 1:1 with the framework's inserted nodes
    // (the backend never injects stray text nodes), so `index` lines up
    // with the reconciler's running child index. `item(index)` past the
    // end returns `None` → `insert_before(child, None)` appends.
    let reference = parent.child_nodes().item(index as u32);
    parent
        .insert_before(&child, reference.as_ref())
        .expect("insert_before failed");
}

/// Detach exactly one child — the `remove_child` half of child-splicing.
/// Tolerant of a node that isn't currently a child (a no-op then) so a
/// reconciler that races an ancestor teardown can't panic.
pub(crate) fn remove_child(parent: &Node, child: &Node) {
    let _ = parent.remove_child(child);
}

pub(crate) fn clear_children(node: &Node) {
    // Single-FFI bulk detach. The previous per-child
    // `first_child` + `remove_child` loop was 2×N boundary crossings
    // — a 10k-row scope teardown crossed ~20,000 times and dominated
    // rebuild benchmarks (~2 seconds for the unmount half of a
    // `Switch` re-key at 10k rows).
    //
    // Casting Node → Element to use `set_inner_html("")` is safe for
    // our use sites: `clear_children` is only called on container
    // nodes (Views, ScrollViews, the root mount, etc.) the framework
    // created via `create_view` / `create_scroll_view` / etc., all
    // of which produce `Element`s. If `dyn_into` fails we fall back
    // to the per-child loop so a non-Element Node (a Text node, say,
    // if a caller ever passed one) still gets detached correctly.
    //
    // Rust-side state for the detached children (`node_ids`,
    // `dynamic` slots, `state_listeners`, etc.) is cleaned up
    // independently via the surrounding Scope's `StyleHandle` /
    // RAII guard drops — that path doesn't depend on the DOM
    // mutation order here.
    let cleared = node
        .clone()
        .dyn_into::<web_sys::Element>()
        .map(|el| {
            el.set_inner_html("");
            true
        })
        .unwrap_or(false);
    if !cleared {
        while let Some(child) = node.first_child() {
            node.remove_child(&child).expect("remove_child failed");
        }
    }
}
