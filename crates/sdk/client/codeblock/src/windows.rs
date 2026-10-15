//! Windows handler for the `code_block` payload.
//!
//! The Windows backend is a painted single-surface scene (no per-widget
//! HWNDs for content), so the "one native rich-text widget" the other
//! handlers build maps to the backend's painted colored-runs leaf:
//! `WindowsBackend::create_colored_code_leaf` measures the pre-tokenized
//! `(text, color)` runs once in the platform monospace font (Cascadia Mono
//! → Consolas fallback) and the scene painter draws them per line with
//! per-run colors — one painted node, no per-token anything, matching the
//! crate's single-node contract.
//!
//! The leaf sits inside one plain view, which is the node the author's
//! `.with_style` lands on — background, radius, and padding apply there
//! and the leaf inherits the inset via layout, the same observable model
//! as the other backends' author-driven padding.
//!
//! No wrapping, matching every sibling handler (they pair no-wrap with a
//! horizontal scroll region): code keeps its authored line structure and
//! the author-styled view clips overflow.

use backend_windows::{WindowsBackend, WindowsNode};
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::Color;
use runtime_vocabulary::caps::ViewOps;

/// Author-styled view wrapping one painted colored-runs leaf.
pub(crate) fn build(spans: &[(String, Color)], b: &mut WindowsBackend) -> WindowsNode {
    let mut outer = b.create_view(&AccessibilityProps::default());
    let leaf = b.create_colored_code_leaf(spans);
    b.insert(&mut outer, leaf);
    outer
}
