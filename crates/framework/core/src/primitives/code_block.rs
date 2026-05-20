//! CodeBlock primitive — read-only colored-text panel.
//!
//! A `CodeBlock` is a flat sequence of `(text, color)` runs; the
//! backend lays them out one after another inside a monospaced
//! `<pre>` (or platform equivalent). Designed for syntax-highlighted
//! source display — the fiddle stamps a tokenized snippet behind a
//! transparent `<textarea>` so the user sees colored Rust under
//! their cursor.
//!
//! The primitive is intentionally minimal: no per-span styles
//! beyond color, no link / hover affordances. If you need richer
//! behavior, compose `Text` primitives instead. The win here is
//! laying out *one* native node that handles the whole panel —
//! cheap to re-render on every keystroke as the tokenizer's output
//! shifts.

use crate::{Bound, Color, Primitive, RefFill};

/// Handle exposed via `Ref<CodeBlockHandle>` for future imperative
/// ops (selection, scroll-to-line, …). Empty for now — the primitive
/// has no imperative surface today, but reserving the handle type
/// avoids a breaking change later.
#[derive(Clone)]
pub struct CodeBlockHandle;

/// Construct a `CodeBlock` from a flat span list. Each tuple is
/// `(text, color)`; consecutive spans of the same color land in
/// separate DOM nodes, so authors who want to collapse identical
/// runs should do so before calling.
pub fn code_block(spans: Vec<(String, Color)>) -> Bound<CodeBlockHandle> {
    Bound::new(Primitive::CodeBlock {
        spans,
        style: None,
        ref_fill: None,
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<CodeBlockHandle> {
    /// Replace the entire span list. Used when the caller's
    /// tokenizer fires on every edit and rebuilds the primitive
    /// in place — the framework's `with_style`-style builder
    /// pattern fits the call site shape.
    pub fn spans(mut self, spans: Vec<(String, Color)>) -> Self {
        if let Primitive::CodeBlock { spans: slot, .. } = &mut self.primitive {
            *slot = spans;
        }
        self
    }
}

/// Reserved RefFill variant. Symmetric with the other primitives;
/// kept for forward compatibility even though the handle has no
/// methods today.
#[allow(dead_code)]
pub(crate) fn _unused_ref_fill_marker(_: RefFill) {}
