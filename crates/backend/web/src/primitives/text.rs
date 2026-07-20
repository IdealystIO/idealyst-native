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

// =============================================================================
// Styled runs — one paragraph as an outer <span> with one child <span>
// per run. The browser's inline formatting context does the paragraph
// flow: runs wrap mid-sentence as one unit, and unstyled runs inherit
// every font property from the outer span (which carries the node's
// paragraph style class via the regular apply_style path). Styled runs
// get an inline `style` attribute from the shared `css` emitter, so
// tokenized colors emit as `var(--token, fallback)` and theme swaps
// ride the CSS cascade — no JS/wasm work per swap.
// =============================================================================

pub(crate) fn create_styled(b: &mut WebBackend, runs: &[runtime_core::TextRun]) -> Node {
    if let Some(span) = b.hydrate_next_skip_subtree("span") {
        // SSR rendered the identical run structure (same author tree,
        // shared css emitter) — adopt the outer span wholesale,
        // children included. `_skip_subtree` because the run spans are
        // backend-built internals the walker never visits: a plain
        // `hydrate_next` would land the cursor on the first run span
        // and the next walker step would mismatch against it.
        return span.unchecked_into::<Node>();
    }
    let outer = b
        .doc
        .create_element("span")
        .expect("create_element span failed");
    let node: Node = outer.unchecked_into();
    append_runs(b, &node, runs);
    b.hydrate_note_fresh(&node);
    node
}

pub(crate) fn update_styled(b: &mut WebBackend, node: &Node, runs: &[runtime_core::TextRun]) {
    // Rebuild the children. `set_text_content(None)` drops all child
    // spans in one call; static runs carry no listeners or registry
    // entries, so plain removal is safe.
    node.set_text_content(None);
    append_runs(b, node, runs);
}

fn append_runs(b: &mut WebBackend, outer: &Node, runs: &[runtime_core::TextRun]) {
    for run in runs {
        let s = b
            .doc
            .create_element("span")
            .expect("create_element span failed");
        s.set_text_content(Some(&run.text));
        if let Some(style) = &run.style {
            if !style.is_empty() {
                let decl = css::text_run_style_css(style);
                if !decl.is_empty() {
                    s.set_attribute("style", &decl)
                        .expect("set style attribute failed");
                }
            }
        }
        outer
            .append_child(&s)
            .expect("append_child for run span failed");
    }
}
