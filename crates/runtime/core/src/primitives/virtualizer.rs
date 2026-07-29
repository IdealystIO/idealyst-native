//! Virtualizer — type-erased windowed/recycled list primitive.
//!
//! Authors don't call this directly. They use the generic
//! `flat_list<T>(...)` wrapper in `primitives::flat_list`, which
//! captures their typed data + closures and feeds the type-erased
//! callbacks Virtualizer needs.
//!
//! # Per-backend strategy
//!
//! - **Web**: a JS-side scroll handler (`backend-web/runtime/ts/virtualizer.ts`)
//!   owns the scroll listener + visible-range diff. It calls back
//!   into Rust only when items enter/leave the window. Per-item
//!   scopes are dropped on exit, so signals/effects for unmounted
//!   items are freed.
//! - **iOS**: `UICollectionView` with a flow layout that consults
//!   our `item_height` callback. Real cell recycling: `prepareForReuse`
//!   releases the item subtree, `cellForItemAt` builds the next one.
//! - **Android**: `RecyclerView` with a `ListAdapter` + `DiffUtil`.
//!   `onBindViewHolder` calls Rust to build the subtree;
//!   `onViewRecycled` releases it.
//!
//! All three backends see the same Rust contract.
//!
//! # Stable identity
//!
//! Every item carries a stable `u64` key (typically a hash of its
//! database id or content). When the data changes, the framework
//! diffs old keys against new keys to decide what to preserve.
//! Items whose key still exists keep their mounted subtree intact
//! — they may move in the layout, but their internal signals,
//! refs, and mounted state survive.
//!
//! # Size resolution
//!
//! Two modes per `ItemSize`:
//! - `Known`: author provides exact size per item before mount.
//!   Layout is deterministic. Cheapest.
//! - `Measured`: author provides an *estimate* per item; backend
//!   measures the actual rendered size on mount and stores it.
//!   Subsequent layout uses the measured value. If the item's
//!   rendered size changes later (its content updated), the
//!   backend's layout-observation primitive (ResizeObserver on web,
//!   `layoutSubviews` on iOS, `OnLayoutChangeListener` on Android)
//!   re-fires and refreshes the stored size.

use crate::{Bound, Element, Ref, RefFill};
use std::rc::Rc;

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::virtualizer::*;

/// Construct a Virtualizer. Authors typically don't call this
/// directly — `flat_list<T>(...)` is the typed wrapper.
///
/// All callbacks are type-erased: the wrapper holds the actual
/// `T`-typed closures and bridges through these `usize`-only
/// callbacks. The framework's build path handles per-item scope
/// management — `render_item(idx)` runs inside a fresh `Scope`,
/// and that scope is dropped when the item is released.
#[cfg(feature = "prim-virtualizer")]
pub fn virtualizer(
    item_count: Box<dyn Fn() -> usize>,
    item_key: Box<dyn Fn(usize) -> ItemKey>,
    item_size: ItemSize,
    render_item: Rc<dyn Fn(usize) -> Element>,
) -> Bound<VirtualizerHandle> {
    // Closure-driven entry point: produce a `Derived<usize>` with
    // empty metadata (`is_opaque() == true`) so runtime backends
    // pick up the closure but generator backends report a clear
    // build-time error.
    let item_count = crate::derive::IntoDerived::<usize>::into_derived(item_count);
    Bound::new(Element::Virtualizer {
        item_count,
        item_key,
        item_size,
        render_item,
        row_template: None,
        row_index_signal_id: None,
        overscan: 1.0,
        layout: VirtualLayout::default(),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

impl Bound<VirtualizerHandle> {
    /// Buffer factor outside the visible window. Default `1.0`
    /// (one viewport extent above and below). Higher = smoother
    /// fast-scroll, more memory.
    pub fn overscan(mut self, factor: f32) -> Self {
        if let Element::Virtualizer { overscan, .. } = &mut self.primitive {
            *overscan = factor;
        }
        self
    }

    /// Scroll axis. Default `Axis::Vertical`. `Axis::Horizontal`
    /// gives a horizontally-scrolling list/grid.
    pub fn axis(mut self, axis: Axis) -> Self {
        if let Element::Virtualizer { layout, .. } = &mut self.primitive {
            layout.axis = axis;
        }
        self
    }

    /// Cross-axis lane subdivision. `Lanes::Fixed(1)` (default) is a
    /// list; `Lanes::Fixed(N)` an N-lane uniform grid;
    /// `Lanes::AutoFit { min_cross }` a responsive grid.
    pub fn lanes(mut self, lanes: Lanes) -> Self {
        if let Element::Virtualizer { layout, .. } = &mut self.primitive {
            layout.lanes = lanes;
        }
        self
    }

    /// Gaps: `main` between successive grid-rows along the scroll
    /// axis, `cross` between lanes. For a list, only `main` (the
    /// inter-row gap) is meaningful.
    pub fn spacing(mut self, main: f32, cross: f32) -> Self {
        if let Element::Virtualizer { layout, .. } = &mut self.primitive {
            layout.main_spacing = main;
            layout.cross_spacing = cross;
        }
        self
    }

    /// Convenience: equal gap on both axes.
    pub fn gap(self, gap: f32) -> Self {
        self.spacing(gap, gap)
    }

    pub fn bind(mut self, r: Ref<VirtualizerHandle>) -> Self {
        if let Element::Virtualizer { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Virtualizer(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

#[cfg(all(test, feature = "prim-virtualizer"))]
mod tests {
    use super::*;

    fn empty_virtualizer() -> Bound<VirtualizerHandle> {
        virtualizer(
            Box::new(|| 0),
            Box::new(|i| i as u64),
            ItemSize::Known(Rc::new(|_| 40.0)),
            Rc::new(|_| crate::view(Vec::new()).primitive),
        )
    }

    fn layout_of(b: &Bound<VirtualizerHandle>) -> VirtualLayout {
        match &b.primitive {
            Element::Virtualizer { layout, .. } => *layout,
            _ => unreachable!("virtualizer() builds Element::Virtualizer"),
        }
    }

    #[test]
    fn default_layout_is_vertical_single_lane_list() {
        let l = layout_of(&empty_virtualizer());
        assert_eq!(l.axis, Axis::Vertical);
        assert_eq!(l.lanes, Lanes::Fixed(1));
        assert_eq!(l.main_spacing, 0.0);
        assert_eq!(l.cross_spacing, 0.0);
        assert!(!l.is_grid());
    }

    #[test]
    fn builders_set_axis_lanes_and_spacing() {
        let b = empty_virtualizer()
            .axis(Axis::Horizontal)
            .lanes(Lanes::Fixed(4))
            .spacing(6.0, 10.0);
        let l = layout_of(&b);
        assert_eq!(l.axis, Axis::Horizontal);
        assert_eq!(l.lanes, Lanes::Fixed(4));
        assert_eq!(l.main_spacing, 6.0);
        assert_eq!(l.cross_spacing, 10.0);
        assert!(l.is_grid());
    }

    #[test]
    fn gap_sets_both_axes_equally() {
        let l = layout_of(&empty_virtualizer().gap(12.0));
        assert_eq!(l.main_spacing, 12.0);
        assert_eq!(l.cross_spacing, 12.0);
    }

    #[test]
    fn fixed_lanes_ignore_container_extent() {
        // Fixed always returns its count (>=1), regardless of cross size.
        assert_eq!(Lanes::Fixed(3).resolve(1000.0, 8.0), 3);
        assert_eq!(Lanes::Fixed(3).resolve(0.0, 8.0), 3);
        // Zero clamps up to one — never a divide-by-zero grid.
        assert_eq!(Lanes::Fixed(0).resolve(1000.0, 0.0), 1);
    }

    #[test]
    fn autofit_resolves_gap_aware_lane_count() {
        // 5 lanes of 100 + 4 gaps of 10 = 540 <= 540: exactly 5.
        assert_eq!(
            Lanes::AutoFit { min_cross: 100.0 }.resolve(540.0, 10.0),
            5
        );
        // One pixel short of fitting a 5th lane → 4.
        assert_eq!(
            Lanes::AutoFit { min_cross: 100.0 }.resolve(539.0, 10.0),
            4
        );
        // No gaps: floor(cross / min).
        assert_eq!(
            Lanes::AutoFit { min_cross: 160.0 }.resolve(500.0, 0.0),
            3
        );
    }

    #[test]
    fn autofit_degrades_to_one_lane_on_unknown_or_narrow_container() {
        // Container narrower than one min lane → 1 (a list), never 0.
        assert_eq!(Lanes::AutoFit { min_cross: 200.0 }.resolve(150.0, 0.0), 1);
        // Zero/unknown container extent → 1.
        assert_eq!(Lanes::AutoFit { min_cross: 200.0 }.resolve(0.0, 0.0), 1);
        // Nonsense min → 1.
        assert_eq!(Lanes::AutoFit { min_cross: 0.0 }.resolve(500.0, 0.0), 1);
    }

    #[test]
    fn autofit_is_always_reported_as_grid() {
        // Even though AutoFit can resolve to one lane at runtime, the
        // author asked for grid behavior, so `is_grid()` is true.
        let b = empty_virtualizer().lanes(Lanes::AutoFit { min_cross: 120.0 });
        assert!(layout_of(&b).is_grid());
    }
}
