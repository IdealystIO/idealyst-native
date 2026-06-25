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
use std::any::Any;
use std::rc::Rc;

/// Stable identity for an item. The user-facing API takes a closure
/// `Fn(usize, &T) -> u64`; the framework keeps `MountedItem`s keyed
/// by this u64 across data updates. Two distinct items with the
/// same key are a user bug — the framework treats them as the same
/// identity and will silently drop one.
pub type ItemKey = u64;

/// Size-knowledge strategy. `flat_list<T>` accepts either variant
/// at the typed layer; this is the type-erased form Virtualizer
/// sees.
pub enum ItemSize {
    /// Author tells us the exact size. Backend never measures.
    Known(Rc<dyn Fn(usize) -> f32>),
    /// Author provides an estimate; backend measures on mount and
    /// updates. Use this when items have data-driven content
    /// whose size you can't predict from data alone (e.g. wrapped
    /// text where the wrap width depends on the container).
    Measured(Rc<dyn Fn(usize) -> f32>),
}

impl ItemSize {
    /// Get the size for an index — either the author's known value
    /// or their estimate. Backends call this for the initial layout
    /// before any measurement.
    pub fn initial(&self, idx: usize) -> f32 {
        match self {
            ItemSize::Known(f) | ItemSize::Measured(f) => f(idx),
        }
    }

    /// True if this is `Measured` — backends use this to decide
    /// whether to install a layout observer on each mounted item.
    pub fn is_measured(&self) -> bool {
        matches!(self, ItemSize::Measured(_))
    }
}

/// Scroll / primary axis of a virtualizer.
///
/// The *main axis* is the scroll direction; the *cross axis* is
/// perpendicular to it. In a list the cross axis holds a single item
/// (it fills the container); in a grid it's subdivided into `Lanes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Axis {
    /// Scrolls vertically; grid-rows stack top-to-bottom, lanes run
    /// left-to-right. The default.
    #[default]
    Vertical,
    /// Scrolls horizontally; grid-rows stack left-to-right, lanes run
    /// top-to-bottom.
    Horizontal,
}

impl Axis {
    /// True for `Horizontal`. Backends use this to swap their
    /// scroll-direction / size axes.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Axis::Horizontal)
    }
}

/// Cross-axis subdivision — how many lanes (tracks) items pack into.
///
/// `Fixed(1)` is a plain list (one item per main-axis line). `N > 1`
/// lanes is a uniform grid: item `i` lands in lane `i % N` of
/// grid-row `i / N`. `AutoFit` derives `N` from the container's
/// cross-axis extent at layout time.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lanes {
    /// A fixed lane count. `Fixed(1)` (the default) is a plain list;
    /// `Fixed(3)` is a three-column grid.
    Fixed(usize),
    /// Responsive lane count: the largest `N` whose lanes are each at
    /// least `min_cross` points along the cross axis, given the
    /// container's measured cross extent and `cross_spacing`. Mirrors
    /// CSS `repeat(auto-fill, minmax(min_cross, 1fr))`. Backends read
    /// their container size in the layout pass to resolve `N`, so a
    /// resize re-lanes the grid.
    AutoFit { min_cross: f32 },
}

impl Default for Lanes {
    fn default() -> Self {
        Lanes::Fixed(1)
    }
}

impl Lanes {
    /// Resolve to a concrete lane count for a given cross-axis extent.
    /// `Fixed` ignores `cross`. `AutoFit` solves the largest `N` with
    /// `N*min_cross + (N-1)*cross_spacing <= cross`. Always returns at
    /// least 1 — a zero/unknown container collapses to a single lane
    /// (list) rather than dividing by zero.
    pub fn resolve(self, cross: f32, cross_spacing: f32) -> usize {
        match self {
            Lanes::Fixed(n) => n.max(1),
            Lanes::AutoFit { min_cross } => {
                if min_cross <= 0.0 || cross <= 0.0 {
                    return 1;
                }
                // N*min + (N-1)*gap <= cross
                //   => N <= (cross + gap) / (min + gap)
                let n = ((cross + cross_spacing) / (min_cross + cross_spacing)).floor();
                (n as usize).max(1)
            }
        }
    }
}

/// Full layout descriptor for a virtualizer: scroll axis, cross-axis
/// lane subdivision, and gaps. This is the low-level layout surface —
/// a list is just `Lanes::Fixed(1)`, a uniform grid is
/// `Lanes::Fixed(N)` or `AutoFit`.
///
/// # Forward-compat
///
/// This is a struct (not an enum of List/Grid) deliberately: a future
/// masonry / shortest-lane packing mode can be added as an extra
/// field (e.g. a `pack: LanePacking` enum) without touching the
/// list/grid range math, which keys off `lanes` alone. Construction
/// goes through builder methods, never a struct literal at author
/// sites, so adding a field stays non-breaking for callers.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct VirtualLayout {
    /// Scroll direction. Items flow + recycle along this axis.
    pub axis: Axis,
    /// Cross-axis lane count. `Fixed(1)` = list.
    pub lanes: Lanes,
    /// Gap between successive grid-rows along the main axis.
    pub main_spacing: f32,
    /// Gap between lanes along the cross axis. Only meaningful when
    /// there is more than one lane.
    pub cross_spacing: f32,
}

impl VirtualLayout {
    /// True when more than one lane is configured — i.e. this is a
    /// grid, not a list. `AutoFit` always reports `true` (it may
    /// resolve to one lane at runtime on a narrow container, but the
    /// author asked for grid behavior).
    pub fn is_grid(self) -> bool {
        !matches!(self.lanes, Lanes::Fixed(1))
    }
}

/// Handle for `Ref<VirtualizerHandle>`. Future methods: scroll to
/// index, scroll to top, get visible range, etc.
#[derive(Clone)]
pub struct VirtualizerHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn VirtualizerOps,
}

impl VirtualizerHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn VirtualizerOps) -> Self {
        Self { node, ops }
    }

    /// Scroll the list so the item at `index` is in view.
    pub fn scroll_to_index(&self, index: usize) {
        self.ops.scroll_to_index(&*self.node, index);
    }
}

pub trait VirtualizerOps {
    fn scroll_to_index(&self, node: &dyn Any, index: usize);
}

/// Construct a Virtualizer. Authors typically don't call this
/// directly — `flat_list<T>(...)` is the typed wrapper.
///
/// All callbacks are type-erased: the wrapper holds the actual
/// `T`-typed closures and bridges through these `usize`-only
/// callbacks. The framework's build path handles per-item scope
/// management — `render_item(idx)` runs inside a fresh `Scope`,
/// and that scope is dropped when the item is released.
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

#[cfg(test)]
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
