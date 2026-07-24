//! Windows handler for the `code_block` external.
//!
//! The Windows backend is a painted single-surface scene (no
//! per-widget HWNDs for content), so the "one native rich-text
//! widget" the other handlers build maps to the backend's painted
//! colored-runs leaf: `WindowsBackend::create_colored_code_leaf`
//! measures the pre-tokenized `(text, color)` runs once in the
//! platform monospace font (Cascadia Mono → Consolas fallback) and
//! the scene painter draws them per line with per-run colors — one
//! painted node, no per-token anything, matching the crate's
//! single-node contract.
//!
//! The leaf sits inside one plain `create_view` node, which is what
//! the framework hands the author's `.with_style` on the
//! `code_block(...)` — background, radius, and padding land there and
//! the padded leaf inherits the inset via layout, same observable
//! model as the other backends' author-driven padding.
//!
//! No wrapping, matching every sibling handler (they pair no-wrap
//! with a horizontal scroll region): code keeps its authored line
//! structure and the author-styled view clips overflow.

use crate::CodeBlockProps;
use backend_windows::{WindowsBackend, WindowsNode};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, RegisterExternal};
use std::rc::Rc;

/// Register the Windows `code_block` external handler on `backend`.
pub fn register(backend: &mut WindowsBackend) {
    backend.register_external::<CodeBlockProps, _>(build);
}

/// `Element::External` handler: author-styled view + one painted
/// colored-runs leaf.
fn build(props: &Rc<CodeBlockProps>, b: &mut WindowsBackend) -> WindowsNode {
    let a11y = AccessibilityProps::default();
    let mut outer = b.create_view(&a11y);
    let leaf = b.create_colored_code_leaf(&props.spans);
    b.insert(&mut outer, leaf);
    outer
}
