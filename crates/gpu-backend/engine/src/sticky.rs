//! `Position::Sticky` on the wgpu backend.
//!
//! Mirrors the iOS reference implementation
//! (`crates/backend/ios/mobile/src/imp/sticky.rs`) but adapted to a
//! GPU-driven renderer that paints rectangles given a position,
//! rather than mutating a tree of UIKit views.
//!
//! ## Semantics
//!
//! A view with `Position::Sticky` behaves like `Relative` until
//! its enclosing scroll container's `offset_y` would scroll the
//! view's natural top above `threshold` from the scroll
//! container's top edge. At that point the view pins at
//! `threshold` from the scroll container's top edge. Scrolling
//! back up un-pins it. Matches CSS sticky.
//!
//! ## Implementation shape
//!
//! Two halves cooperate:
//!
//! 1. **Registry** (this module). A side table on `WgpuBackend`
//!    maps a sticky node's pointer → `StickyChild` carrying the
//!    pin threshold + the cached natural y (in the enclosing
//!    scroll view's content space). `apply_style` registers and
//!    `drop_subtree` deregisters. Per frame, after Taffy
//!    re-computes the layout, [`refresh_layout_positions`] walks
//!    Taffy parents from each sticky child up to its enclosing
//!    `ScrollView` and sums `frame.y` to derive natural-y.
//!
//! 2. **Render-walk hook**. The render walker threads an
//!    `Option<EnclosingScroll>` parameter through recursion; when
//!    it encounters a node registered in the sticky registry, it
//!    consults [`compute_translate`] and adds the resulting
//!    y-shift to the node's draw origin. No matrix / transform
//!    composition needed beyond what the existing animated
//!    `translate_y` path already does.
//!
//! ## Why a registry side-table, not a flag on `NodeData`
//!
//! Sticky pinning is a property of (child, enclosing-scroll) pair,
//! not of the child alone. The enclosing scroll can change at
//! mount time depending on where the framework's walker inserts
//! the child. Tracking the relationship in a registry centralizes
//! the bookkeeping and lets the renderer's hot path do an O(1)
//! HashMap lookup keyed by node pointer — no per-frame ancestor
//! walks.
//!
//! ## v1 scope
//!
//! Vertical pinning via the `top` field only, matching iOS v1.
//! CSS sticky also supports `left`, `bottom`, and `right`;
//! extending here means widening `StickyChild` to track each
//! axis and threading both `(scroll_x, scroll_y)` into
//! [`compute_translate`]. The render walker's enclosing-scroll
//! handle already carries both offsets — only the registry and
//! the natural-position cache need to widen.

use std::collections::HashMap;

use runtime_core::{Length, StyleRules, Tokenized};
use runtime_layout::{LayoutNode, LayoutTree};

use crate::node::{NodeKind, WgpuNode};

/// One sticky child registered against a scroll view.
#[derive(Clone, Debug)]
pub(crate) struct StickyChild {
    /// Pin threshold, in px, read from `StyleRules.top`. The view
    /// pins when `scroll_y + threshold > natural_y`.
    pub(crate) threshold_top: f32,
    /// Layout id of the sticky child's Taffy node. Used by
    /// [`refresh_layout_positions`] to walk Taffy parents up to
    /// the enclosing scroll view.
    pub(crate) child_layout: LayoutNode,
    /// Layout id of the enclosing scroll view's Taffy node, or
    /// `None` if the child has no scrolling ancestor. `None`
    /// matches CSS's "sticky in a non-scrolling parent acts like
    /// relative" semantics — the registry entry stays put (so
    /// `Sticky → Relative` style flips still clear cleanly via
    /// the standard deregister path) but the render walker
    /// won't apply any pin transform.
    pub(crate) scroll_layout: Option<LayoutNode>,
    /// Natural y of the child in the scroll view's content
    /// coordinate space, in px. Refreshed by
    /// [`refresh_layout_positions`] after each Taffy compute.
    /// Initialized to 0; the first layout pass replaces it with a
    /// real value before any render walk reads it.
    pub(crate) natural_y: f32,
}

/// Map from sticky-node pointer (`Rc::as_ptr` cast to `usize`) →
/// registry entry. Pointer keying matches the iOS registry shape
/// and the rest of the wgpu engine's id conventions
/// (accessibility ids in `backend_impl::build_a11y_node`,
/// per-node tween keys in the animator).
pub(crate) type StickyRegistry = HashMap<usize, StickyChild>;

/// Pure compute used by the per-frame walk and the unit tests.
///
/// Returns the y-translation that should be applied to the
/// sticky child's draw origin given its natural layout y in the
/// scroll view's content space, the configured pin threshold
/// (the `top` value), and the scroll view's current `offset_y`.
///
/// The math is identical to iOS's — we duplicate the 3-line
/// helper rather than share via `runtime_core` because there's
/// no other consumer and the inline copy keeps the call site
/// readable.
///
/// TODO: horizontal sticky via `left` mirrors this shape with
/// `(natural_x, threshold_left, scroll_x)`. Wire it once a
/// real layout asks for it; CSS supports it but no current
/// page uses it.
#[inline]
pub(crate) fn compute_translate(natural_y: f32, threshold_top: f32, scroll_y: f32) -> f32 {
    // Pin condition: the natural top of the child has scrolled
    // above the threshold band measured from the scroll view's
    // top edge. Translate the child *down* by the overshoot so
    // its rendered position stays at `scroll_y + threshold_top`.
    let pinned_y = scroll_y + threshold_top;
    if pinned_y > natural_y {
        pinned_y - natural_y
    } else {
        0.0
    }
}

/// Extract the pin threshold (the `top` value, in px) from a
/// style. Percent / Auto aren't meaningful for the sticky pin
/// offset (CSS resolves percent against the scroll container's
/// padding box, but no current page uses that and the iOS
/// reference treats both as zero); collapse to 0 here.
pub(crate) fn threshold_top_from_style(style: &StyleRules) -> f32 {
    style
        .top
        .as_ref()
        .map(|t: &Tokenized<Length>| match t.resolve() {
            Length::Px(v) => v,
            _ => 0.0,
        })
        .unwrap_or(0.0)
}

/// Register a sticky child against its enclosing scroll view.
///
/// Idempotent: re-registering replaces the existing entry, so
/// threshold updates from a stylesheet re-apply (e.g. a state
/// overlay flipping `top`) pick up cleanly. The enclosing scroll
/// view is looked up at register time by walking Taffy parents
/// from the child up; if there isn't one yet, the entry still
/// goes into the registry with `scroll_layout: None` so a future
/// `apply_style` → `Sticky → Relative` flip can find and clear
/// it through the normal [`deregister`] path. The render walker
/// no-ops on entries with no scroll ancestor (matches CSS fall-
/// back-to-relative).
///
/// `layout` and `roots` are taken as references rather than the
/// whole `WgpuBackend` so the caller can hold a `&mut` on the
/// backend's `sticky_registry` field while passing the rest
/// non-mutably (rustc's split-borrow won't see through the
/// registry field on its own without disjoint-field syntax).
///
/// Returns `true` when a scroll ancestor was found (the more
/// interesting case for callers / tests); `false` otherwise.
pub(crate) fn register(
    registry: &mut StickyRegistry,
    layout: &LayoutTree,
    roots: &[WgpuNode],
    node: &WgpuNode,
    threshold_top: f32,
) -> bool {
    let node_key = std::rc::Rc::as_ptr(node) as usize;
    let child_layout = node.borrow().layout;
    let scroll_layout = find_enclosing_scroll_view(layout, roots, child_layout);
    registry.insert(
        node_key,
        StickyChild {
            threshold_top,
            child_layout,
            scroll_layout,
            natural_y: 0.0,
        },
    );
    scroll_layout.is_some()
}

/// Remove `node`'s entry from the registry. Safe to call even if
/// the node was never registered (e.g. a non-sticky `apply_style`
/// arriving on a fresh node).
pub(crate) fn deregister(registry: &mut StickyRegistry, node: &WgpuNode) {
    let key = std::rc::Rc::as_ptr(node) as usize;
    registry.remove(&key);
}

/// Variant of [`deregister`] keyed by raw pointer. Used by
/// `drop_subtree` where we no longer have a `WgpuNode` handle
/// (the node has already been removed from its parent's
/// `children`).
pub(crate) fn deregister_by_ptr(registry: &mut StickyRegistry, key: usize) {
    registry.remove(&key);
}

/// Walk Taffy parents from `start` up looking for an ancestor
/// whose `NodeKind` is `ScrollView`. Returns its `LayoutNode` or
/// `None` if there's no scroll ancestor. We walk Taffy parents
/// and resolve each one to its `WgpuNode` via a sweep of the
/// backend's roots — the wgpu engine doesn't keep a layout-id →
/// node side map today.
///
/// O(depth * tree-size) in the worst case. Called once per
/// sticky registration (rare — `apply_style` only, not per
/// frame), so the constant factor isn't on the hot path. Adding
/// a side map is a follow-up if the sweep shows up in a profile.
fn find_enclosing_scroll_view(
    layout: &LayoutTree,
    roots: &[WgpuNode],
    start: LayoutNode,
) -> Option<LayoutNode> {
    let mut cursor = start;
    let mut steps = 0;
    while let Some(parent) = layout.parent_of(cursor) {
        if let Some(parent_node) = find_node_by_layout(roots, parent) {
            if matches!(parent_node.borrow().kind, NodeKind::ScrollView { .. }) {
                return Some(parent);
            }
        }
        cursor = parent;
        steps += 1;
        if steps > 256 {
            // Defensive depth cap. Shouldn't trip; the layout
            // tree is finite and Taffy doesn't construct cycles.
            return None;
        }
    }
    None
}

/// Find a `WgpuNode` whose Taffy layout id matches `target`.
/// Linear sweep over the node tree starting from the backend's
/// roots. Used by [`find_enclosing_scroll_view`].
fn find_node_by_layout(roots: &[WgpuNode], target: LayoutNode) -> Option<WgpuNode> {
    for root in roots {
        if let Some(found) = find_node_by_layout_recursive(root, target) {
            return Some(found);
        }
    }
    None
}

fn find_node_by_layout_recursive(node: &WgpuNode, target: LayoutNode) -> Option<WgpuNode> {
    if node.borrow().layout == target {
        return Some(node.clone());
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        if let Some(found) = find_node_by_layout_recursive(child, target) {
            return Some(found);
        }
    }
    None
}

/// Refresh the cached `natural_y` for every sticky child after a
/// Taffy compute. Walks layout parents from the child up to (but
/// not including) its registered scroll view, summing
/// `frame_of(...).y`. Identical algorithm to iOS's
/// `compute_natural_y_in_scroll`.
///
/// Cheap: O(sum of sticky-child depths). The registry is tiny by
/// construction — at most a handful of sticky elements per app.
pub(crate) fn refresh_layout_positions(
    registry: &mut StickyRegistry,
    layout: &LayoutTree,
) {
    for child in registry.values_mut() {
        let Some(scroll_layout) = child.scroll_layout else {
            // No scroll ancestor — render walker won't apply any
            // pin transform regardless of `natural_y`. Leaving the
            // value untouched matches the "no-op when no scroll
            // ancestor" branch in the walker.
            continue;
        };
        let Some(natural_y) =
            compute_natural_y_in_scroll(child.child_layout, scroll_layout, layout)
        else {
            // Couldn't trace child up to the scroll view (the
            // child may have been detached mid-frame or the
            // scroll view may have been replaced under us).
            // Leave the cached value alone — the next layout
            // pass will retry.
            continue;
        };
        child.natural_y = natural_y;
    }
}

/// Sum Taffy `frame_of(...).y` from `child` up to (but not
/// including) `scroll`. Returns `None` if we walk off the root
/// without finding `scroll`.
fn compute_natural_y_in_scroll(
    child: LayoutNode,
    scroll: LayoutNode,
    layout: &LayoutTree,
) -> Option<f32> {
    let mut sum_y = 0.0_f32;
    let mut cursor = child;
    let mut steps = 0;
    while cursor != scroll {
        sum_y += layout.frame_of(cursor).y;
        let parent = layout.parent_of(cursor)?;
        cursor = parent;
        steps += 1;
        if steps > 256 {
            // Defensive depth cap — symmetric with the walker
            // helper above.
            return None;
        }
    }
    Some(sum_y)
}

/// Render-walk context describing the nearest enclosing scroll
/// view. Threaded through `walk(...)` so a sticky descendant can
/// look up its scroll context without re-walking ancestors per
/// frame.
#[derive(Copy, Clone, Debug)]
pub(crate) struct EnclosingScroll {
    /// Taffy id of the scroll view. Matches
    /// [`StickyChild::scroll_layout`] when the child is registered
    /// against this scroll view.
    pub(crate) scroll_layout: LayoutNode,
    /// Current `offset_y` of the scroll view, read once at walk
    /// time so a sticky child sees the same scroll position as
    /// the surrounding rect generation.
    pub(crate) scroll_offset_y: f32,
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    //! Regression coverage for `Position::Sticky` on the wgpu
    //! backend. Per CLAUDE.md §8, every bug fix lands with a test
    //! named after the bug being prevented.
    //!
    //! Tests #1 (pure compute) and #2 (registry lifecycle) run
    //! host-side without a wgpu Device — they only touch the
    //! registry's `HashMap` keying and the pure
    //! [`compute_translate`] function. Test #3 (fall-back to
    //! relative when no scroll ancestor) likewise needs only the
    //! registry, since the render walker's "no scroll ancestor →
    //! no translate" branch is the same property exercised by
    //! `compute_translate` returning 0 at scroll_y=0.
    //!
    //! Full end-to-end coverage (walk through walk(), produce
    //! rects with the pinning translate, verify against a golden)
    //! requires a wgpu Device + Queue + a Painter — those live in
    //! `host-winit` integration tests and are skipped here.
    //! Document any future bugs in those layers with a host-shell
    //! regression test there.
    use super::*;

    /// Pin compute: scrolling past the threshold translates the
    /// child down by the overshoot; scrolling above the threshold
    /// leaves the child at its natural position. Boundary case
    /// (`scroll_y + threshold == natural_y`) stays at 0 — matches
    /// the iOS reference's `>` (strict) comparison.
    #[test]
    fn regression_sticky_compute_translate_pins_past_threshold() {
        // Child sits at y=100 in the scroll view's content; pin
        // threshold (top) is 20px from the scroll view's top edge.
        let natural_y = 100.0;
        let threshold = 20.0;

        // Far above the pin point — no translate.
        assert_eq!(compute_translate(natural_y, threshold, 0.0), 0.0);

        // Just at the pin point (scroll_y + threshold == natural_y).
        // Boundary: still 0 (`>` strict).
        assert_eq!(compute_translate(natural_y, threshold, 80.0), 0.0);

        // 1px past the pin point — translate by 1px.
        let t = compute_translate(natural_y, threshold, 81.0);
        assert!((t - 1.0).abs() < 1e-5, "expected ~1.0, got {t}");

        // Way past the pin point — translate compensates fully so
        // the child renders at scroll_y + threshold = 300.
        let t = compute_translate(natural_y, threshold, 280.0);
        assert!(
            (t - 200.0).abs() < 1e-5,
            "expected ~200.0 (so rendered y == scroll_y + threshold = 300), got {t}",
        );

        // Sanity: rendered y while pinned == scroll_y + threshold.
        let scroll_y = 500.0;
        let t = compute_translate(natural_y, threshold, scroll_y);
        let rendered_y = natural_y + t;
        assert!(
            (rendered_y - (scroll_y + threshold)).abs() < 1e-5,
            "pinned rendered_y should equal scroll_y + threshold",
        );
    }

    /// Registry must shrink back to empty after a register +
    /// deregister round-trip. The leak-equivalent regression
    /// would surface as a non-empty registry after deregister and
    /// would leave stale `natural_y` values pinned to a since-
    /// removed node id (which the renderer would then read every
    /// frame, producing ghost translates on whatever node Rc
    /// happens to reuse the slot).
    ///
    /// We exercise the registry's `HashMap` behaviour directly —
    /// `register` requires a `WgpuBackend` with a populated tree
    /// (it walks Taffy parents to find the scroll ancestor), which
    /// pulls in glyphon + wgpu Device dependencies. Constructing
    /// that for a unit test would be ~50 lines of setup. The
    /// HashMap behaviour IS the regression we care about; the
    /// "walks parents to find scroll" path is covered by the
    /// integration in `host-winit`.
    #[test]
    fn regression_sticky_registry_unregisters_on_node_removal() {
        let mut registry: StickyRegistry = HashMap::new();
        assert_eq!(registry.len(), 0);

        // Two distinct pointer ids stand in for two sticky nodes.
        let key_a = 0x1000_usize;
        let key_b = 0x2000_usize;

        // We can't build a `LayoutNode` from outside `runtime_layout`
        // (the inner SlotMap key isn't pub-constructable), so we
        // hand-craft a `StickyChild` via the same field-by-field
        // shape `register` produces — except for `child_layout` /
        // `scroll_layout`, which we leave at a dummy value pulled
        // from a fresh `LayoutTree`. The test only inspects
        // `registry.len()`, `contains_key`, and the `threshold_top`
        // round-trip, so the layout-id values are inert.
        let mut layout = LayoutTree::new();
        let dummy_a = layout.new_node();
        let dummy_b = layout.new_node();

        registry.insert(
            key_a,
            StickyChild {
                threshold_top: 12.0,
                child_layout: dummy_a,
                scroll_layout: None,
                natural_y: 0.0,
            },
        );
        registry.insert(
            key_b,
            StickyChild {
                threshold_top: 16.0,
                child_layout: dummy_b,
                scroll_layout: None,
                natural_y: 0.0,
            },
        );
        assert_eq!(registry.len(), 2);
        assert!(registry.contains_key(&key_a));
        assert!(registry.contains_key(&key_b));
        assert_eq!(registry.get(&key_a).unwrap().threshold_top, 12.0);

        // Simulate one node being removed via `drop_subtree` → the
        // `deregister_by_ptr` path. The OTHER entry must survive.
        deregister_by_ptr(&mut registry, key_a);
        assert_eq!(registry.len(), 1);
        assert!(!registry.contains_key(&key_a));
        assert!(registry.contains_key(&key_b));

        // Removing the last entry empties the registry.
        deregister_by_ptr(&mut registry, key_b);
        assert_eq!(registry.len(), 0);

        // Removing a key that isn't there is a no-op (matches the
        // HashMap::remove contract; defends against a re-fire of
        // `drop_subtree` on the same node).
        deregister_by_ptr(&mut registry, key_a);
        assert_eq!(registry.len(), 0);
    }

    /// A sticky child with no scrolling ancestor must not apply a
    /// pin transform. The registry entry's `scroll_layout` is
    /// `None` in that case; `refresh_layout_positions` skips the
    /// recompute and the render walker (which keys the translate
    /// off `EnclosingScroll`) won't have a matching scroll context
    /// to pin against. The observable result is "renders like
    /// `Position::Relative`," matching CSS.
    ///
    /// We verify the registry- and refresh-side invariants
    /// directly. The walker-side check (no `EnclosingScroll` ⇒ no
    /// translate) is structural: the walker's sticky branch reads
    /// the registry entry and uses `scroll_layout` to validate
    /// against the current scroll context — if `scroll_layout`
    /// is `None`, no validation succeeds.
    #[test]
    fn regression_sticky_falls_back_to_relative_without_scroll_ancestor() {
        let mut registry: StickyRegistry = HashMap::new();
        let mut layout = LayoutTree::new();
        let dummy = layout.new_node();
        let key = 0xDEAD_BEEF_usize;

        registry.insert(
            key,
            StickyChild {
                threshold_top: 20.0,
                child_layout: dummy,
                scroll_layout: None,
                natural_y: 0.0,
            },
        );

        // refresh_layout_positions must NOT panic and must NOT
        // touch `natural_y` for an entry with no scroll ancestor.
        refresh_layout_positions(&mut registry, &layout);
        let entry = registry.get(&key).unwrap();
        assert_eq!(
            entry.natural_y, 0.0,
            "natural_y must stay at its initial value when scroll_layout is None",
        );
        assert!(
            entry.scroll_layout.is_none(),
            "scroll_layout must remain None — no ancestor was found at register time",
        );

        // The pure-function path is symmetric: zero scroll means
        // no pin even with a registered entry. (Combined with
        // `scroll_layout: None` blocking the walker's lookup,
        // this is the structural fall-back-to-relative property.)
        assert_eq!(
            compute_translate(100.0, 20.0, 0.0),
            0.0,
            "scroll_y=0 implies no pin regardless of threshold",
        );
    }
}
