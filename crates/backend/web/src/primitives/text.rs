//! `Element::Text` — a `<span>` so style application via `class`
//! works uniformly. A raw DOM text node has no `class`/`style`
//! attributes, so styling would be silently dropped.

use crate::WebBackend;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(b: &mut WebBackend, content: &str) -> Node {
    if let Some(span) = b.hydrate_next("span") {
        // SSR already rendered this text; adopt the span as-is (same
        // author tree → same content). Reactive updates retarget it via
        // `update_text`.
        return span.unchecked_into::<Node>();
    }
    let span = b
        .doc
        .create_element("span")
        .expect("create_element span failed");
    span.set_text_content(Some(content));
    let node: Node = span.unchecked_into();
    b.hydrate_note_fresh(&node);
    node
}

/// Hydration-aware variant of [`create_with_inner_text`]: adopts the SSR
/// `<span>` (and its existing child Text node) when hydrating, so the
/// batched-text registry binds the real text node. Falls back to creating
/// fresh when not hydrating / on mismatch.
pub(crate) fn create_with_inner_text_hydrating(b: &mut WebBackend, content: &str) -> (Node, Node) {
    if let Some(span) = b.hydrate_next("span") {
        // The SSR span's first child is its Text node. If somehow absent
        // (empty text), synthesize one so the batched-update path has a
        // node to write.
        let text: Node = match span.first_child() {
            Some(n) => n,
            None => {
                let t = b.doc.create_text_node(content);
                let _ = span.append_child(&t);
                t.unchecked_into::<Node>()
            }
        };
        return (span.unchecked_into::<Node>(), text);
    }
    let (span, text) = create_with_inner_text(b, content);
    b.hydrate_note_fresh(&span);
    (span, text)
}

/// Variant of [`create`] that guarantees the returned span has a
/// child Text node — and returns it alongside so callers (the
/// batched-text path) can store the Text node directly in the
/// JS-side registry. Setting `.nodeValue` on a Text node is an
/// O(1) string-slot assignment; setting `.textContent` on an
/// Element clears all children + creates a new Text node + appends
/// it (the slow DOM-mutation path). At hierarchy scale (20 k
/// leaves fanning out on one signal), the difference is ~30 ms
/// per flush.
pub(crate) fn create_with_inner_text(b: &mut WebBackend, content: &str) -> (Node, Node) {
    let span = b
        .doc
        .create_element("span")
        .expect("create_element span failed");
    let text = b.doc.create_text_node(content);
    span.append_child(&text)
        .expect("append_child for text-node child failed");
    (span.unchecked_into::<Node>(), text.unchecked_into::<Node>())
}

pub(crate) fn update_text(node: &Node, content: &str) {
    // Works for both Element (e.g. our <span>) and Text node cases.
    node.set_text_content(Some(content));
}
