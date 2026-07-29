//! Virtualizer payload: `flat_list` / `virtualizer`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtualizer::{ItemKey, ItemSize, VirtualLayout, VirtualizerHandle};
use runtime_scene::Element;

use crate::style_attach::StyleProp;

/// The `virtualizer` / `flat_list` primitive (`walker/virtualizer.rs`).
///
/// Carries the CLOSURE form of the old `Element::Virtualizer` payload.
/// The old variant's structured metadata — `item_count` as a
/// `Derived<usize>`, the pre-built `row_template`, `row_index_signal_id`
/// — existed only for generator backends (Roku wire replay); per the
/// crate docs' sanctioned lowering, generator-backend bindings are
/// deferred and every author shape lowers to the equivalent closures
/// (same observable reactivity on event-driven backends). What remains
/// is exactly what runtime backends consume:
///
/// - `item_count` is a plain closure; the handler's data effect calls it
///   so any signal reads inside subscribe, and re-fires
///   `virtualizer_data_changed` on change — the old core's opaque
///   (`is_opaque()`) `Derived` path.
/// - `render_item` produces a **single-root** scene [`Element`] per
///   index. The handler realizes each row DETACHED, in its own
///   ownership scope, when the platform asks for it (lazy inversion of
///   control: the backend owns the visible-window math).
/// - `item_size` keeps the old `Known`/`Measured` split; the handler
///   owns the measured-size cache exactly as the walker did.
/// - `layout` is the lane model (`VirtualLayout { axis, lanes,
///   spacing }`) that replaced `horizontal: bool` across all backends —
///   inert config passed to `create_virtualizer` once.
pub struct VirtualizerPrim {
    /// Reactive item count — reads inside subscribe the data effect.
    pub item_count: Box<dyn Fn() -> usize>,
    /// Stable identity for an index; the mounted-row and measured-size
    /// caches key off it so state survives reorders.
    pub item_key: Box<dyn Fn(usize) -> ItemKey>,
    /// Size-knowledge strategy (`Known` authoritative / `Measured`
    /// estimate + backend measurement).
    pub item_size: ItemSize,
    /// Materialize the row for an index. Must be single-root (wrap
    /// fragment rows in an item) — same contract as
    /// `MountCx::realize_detached`.
    pub render_item: Rc<dyn Fn(usize) -> Element>,
    /// Buffer factor outside the visible window (viewport extents).
    pub overscan: f32,
    /// Scroll axis + cross-axis lane subdivision + gaps.
    pub layout: VirtualLayout,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(VirtualizerHandle)>>,
}
