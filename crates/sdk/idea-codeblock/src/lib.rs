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
//! It used to be `Primitive::CodeBlock` in `framework-core`. A
//! measurement showed the perf justification was real — the
//! equivalent composition (`View` + `Repeat<styled-View+Text>`)
//! generates 100–300× more backend ops per re-render even with the
//! batched-Repeat fast path. The structural gap (composition
//! rebuilds every span on each render; the single-node primitive
//! replaces one node) can't be closed by framework optimization
//! alone.
//!
//! But the primitive doesn't fit framework-core's intent — it isn't
//! a platform-native widget (no platform has a "code block"
//! element) and it's expressible from existing primitives if perf
//! weren't a concern. CLAUDE.md rule 3 says exactly this case
//! belongs in a third-party extension via `Primitive::External`.
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
//! `Primitive::External`: tear down old node + create new node.

use framework_core::{external, Bound, Color, ExternalHandle};

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
/// `Primitive::External`-backed primitive.
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
// Platform-routed `register` — exactly one of the cfg-gated re-exports
// is active per build, selected by `target_arch`. Mirrors the `maps`
// SDK pattern.
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(not(target_arch = "wasm32"))]
pub fn register<B>(_backend: &mut B) {
    // No backend leaf for this target — the framework will render
    // its "External CodeBlockProps not supported" placeholder at
    // mount. Keeps user code uniformly compilable across targets.
}
