//! Web leaf for the `idea-codeblock` primitive. Registers a
//! `CodeBlockProps` handler against `WebBackend` that builds a
//! `<pre>` of color-styled `<span>` children — the same DOM shape
//! the old runtime-core `Element::CodeBlock` produced.
//!
//! Lifted verbatim from the old
//! `backend-web/src/primitives/code_block.rs` so existing fiddle
//! styling continues to work without changes.

use std::rc::Rc;

use backend_web::WebBackend;

use crate::CodeBlockProps;

/// Install the CodeBlock handler. Call once at app bootstrap:
///
/// ```ignore
/// let mut backend = WebBackend::new("#app");
/// idea_codeblock::register(&mut backend);
/// ```
pub fn register(backend: &mut WebBackend) {
    // `WebBackend::register_external` already takes an
    // `Element`-returning handler and upcasts to `Node` internally,
    // so we don't need a `JsCast` call here.
    backend.register_external::<CodeBlockProps, _>(|props, _b| build_pre(props));
}

fn build_pre(props: &Rc<CodeBlockProps>) -> web_sys::Element {
    // Going through `web_sys::window().document()` rather than the
    // backend's cached `doc` field — `WebBackend::doc` is
    // `pub(crate)` and extensions shouldn't depend on internals.
    // The per-mount cost difference (one extra Reflect into
    // `window` / `document`) is negligible vs. the DOM allocations
    // we're about to do.
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let pre = document
        .create_element("pre")
        .expect("create_element pre failed");
    // Match the layout defaults the textarea overlay relies on:
    // no margin (so it lines up with a positioned `<textarea>`
    // sibling), `white-space: pre` so leading whitespace renders
    // verbatim, `tab-size: 4` to match the textarea's tab width,
    // and `pointer-events: none` so clicks pass through to a
    // sibling `<textarea>` layered on top of (or beneath) it —
    // the fiddle's editor uses exactly that pattern.
    let _ = pre.set_attribute(
        "style",
        "margin: 0; white-space: pre; tab-size: 4; \
         pointer-events: none;",
    );
    write_spans(&document, &pre, &props.spans);
    pre
}

fn write_spans(
    document: &web_sys::Document,
    pre: &web_sys::Element,
    spans: &[(String, runtime_core::Color)],
) {
    for (text, color) in spans {
        let span = document
            .create_element("span")
            .expect("create_element span failed");
        // Inline `color: …` so authors don't have to thread a
        // stylesheet rule per token kind. The Color is already a
        // CSS-style string (`#rrggbb` / `rgb(...)`) since it's
        // what the rest of the framework uses for resolved values.
        let _ = span.set_attribute("style", &format!("color: {};", color.0));
        span.set_text_content(Some(text));
        let _ = pre.append_child(&span);
    }
}
