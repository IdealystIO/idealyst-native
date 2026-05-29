//! `idea-codeblock` — read-only colored-text panel primitive.
//!
//! A flat sequence of `(text, color)` runs rendered as a single
//! native node (web: a `<pre>` containing one `<span>` per run).
//! Built for **syntax-highlighted source display** — the fiddle's
//! editor overlays a tokenized snippet behind a transparent
//! `<textarea>` so colored code shows through under the user's
//! cursor.
//!
//! ## Why this is a third-party primitive, not a framework one
//!
//! It used to be `Element::CodeBlock` in `runtime-core`. A
//! measurement showed the perf justification was real — the
//! equivalent composition (`View` + `Repeat<styled-View+Text>`)
//! generates 100–300× more backend ops per re-render even with the
//! batched-Repeat fast path. The structural gap (composition
//! rebuilds every span on each render; the single-node primitive
//! replaces one node) can't be closed by framework optimization
//! alone.
//!
//! But the primitive doesn't fit runtime-core's intent — it isn't
//! a platform-native widget (no platform has a "code block"
//! element) and it's expressible from existing primitives if perf
//! weren't a concern. CLAUDE.md rule 3 says exactly this case
//! belongs in a third-party extension via `Element::External`.
//! So we kept the fast single-node renderer but moved the type out
//! of core.
//!
//! ## Usage
//!
//! ```ignore
//! use idea_codeblock::{code_block, CodeBlockProps};
//!
//! // At app bootstrap, once per backend:
//! let mut backend = WebBackend::new("#app");
//! idea_codeblock::register(&mut backend);
//!
//! // Inside an effect / arm body:
//! let spans: Vec<(String, Color)> = tokenize(source);
//! code_block(spans).with_style(my_codeblock_style())
//! ```
//!
//! On targets without a registered backend (iOS, Android), the
//! framework renders a "not supported" placeholder at mount.
//! Re-renders pay the same per-render cost as any other
//! `Element::External`: tear down old node + create new node.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{
    external, Backend, Bound, Color, ExternalHandle, RegisterExternal, StyleRules, Tokenized,
};
use std::rc::Rc;

/// Type-erased payload for the CodeBlock external primitive. Lives
/// here because the framework dispatches handlers by [`TypeId`], and
/// the type needs to be the SAME type across the call site and the
/// backend leaf — so it lives in the umbrella crate that both depend
/// on.
///
/// [`TypeId`]: std::any::TypeId
#[derive(Clone)]
pub struct CodeBlockProps {
    /// Color-run sequence. Each tuple is one run of same-colored
    /// text; the backend emits one styled child per tuple. Consecutive
    /// same-color runs are NOT auto-coalesced — the tokenizer caller
    /// decides whether to coalesce.
    pub spans: Vec<(String, Color)>,
}

/// Convenience handle alias — saves callers writing
/// `Ref<ExternalHandle<CodeBlockProps>>` everywhere.
pub type CodeBlockHandle = ExternalHandle<CodeBlockProps>;

/// Construct a `CodeBlock` external primitive from a flat span list.
///
/// Returns a `Bound<CodeBlockHandle>` so `.with_style(...)` and
/// `.bind(...)` work the same way they would for any other
/// `Element::External`-backed primitive.
///
/// ```ignore
/// code_block(vec![
///     ("fn ".into(),    Color("#888".into())),
///     ("hello".into(),  Color("#0a0".into())),
///     ("() { … }".into(), Color("#444".into())),
/// ])
/// ```
pub fn code_block(spans: Vec<(String, Color)>) -> Bound<CodeBlockHandle> {
    external(CodeBlockProps { spans })
}

// =============================================================================
// Generic, backend-neutral handler + registration.
//
// The renderer builds its DOM *through the `Backend` trait*
// (`create_element("pre")` + `create_text` + `apply_style`) rather than
// reaching for `web_sys` directly. That single generic handler then:
//   - SERVER-RENDERS on the SSR backend (real `<pre>` + colored spans in
//     the HTML — crawlable, and the first paint matches the live app), and
//   - on web goes through the hydration adoption cursor (the
//     `create_*` calls adopt the server's nodes), so a server-rendered
//     code block HYDRATES cleanly instead of desyncing the cursor.
//
// Backends with no tag concept (iOS/Android) get `create_element`'s
// `div` fallback — a container of colored text runs.
// =============================================================================

/// Build the code block's node tree on any backend.
fn build_code_block<B: Backend>(props: &Rc<CodeBlockProps>, backend: &mut B) -> B::Node {
    let a11y = AccessibilityProps::default();
    let mut pre = backend.create_element("pre");
    for (text, color) in &props.spans {
        let span = backend.create_text(text, &a11y);
        // Per-run color via the framework's style path (a class on
        // web/SSR) so it resolves identically on both — keeping the
        // server and client DOM in sync for hydration.
        let mut rules = StyleRules::default();
        rules.color = Some(Tokenized::Literal(color.clone()));
        backend.apply_style(&span, &Rc::new(rules));
        backend.insert(&mut pre, span);
    }
    pre
}

/// Register the code-block handler on any backend that owns an external
/// registry (web, SSR, …). One call, every target — app bootstrap does
/// `idea_codeblock::register(&mut backend)` and the SSR path does the
/// same, so both render the block identically.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    backend.register_external::<CodeBlockProps, _>(build_code_block::<B>);
}
