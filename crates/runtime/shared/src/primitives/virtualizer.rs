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

/// Handle for `Ref<VirtualizerHandle>` — the imperative surface on a
/// mounted virtualizer.
///
/// A virtualizer OWNS its scroller (unlike `scroll_view`, where the
/// author supplies the container), so without these methods no sibling
/// can align to it: a sticky header, an edge-triggered fetch, or a
/// second pane synced to the same offset all need to read or write the
/// scroll position of a scroller they don't hold. That surface is
/// deliberately the same shape as
/// [`ScrollViewHandle`](super::scroll_view::ScrollViewHandle) — same
/// coordinate space (CSS px / native points, content-box origin), same
/// `scroll_to(x, y)` signature — so swapping a hand-rolled
/// `scroll_view` for a real virtualizer doesn't rewrite the call sites.
///
/// Pair with the builder's `.on_scroll(..)` for the push direction.
#[derive(Clone)]
pub struct VirtualizerHandle {
    node: Rc<dyn Any>,
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

    /// Current scroll offset `(x, y)` of the virtualizer's own
    /// scroller, in the same units the `on_scroll` callback reports.
    /// The off-axis component is always `0.0` — a virtualizer scrolls
    /// on exactly one axis ([`VirtualLayout::axis`]).
    ///
    /// Returns `(0.0, 0.0)` before the scroller exists (a handle read
    /// in the same tick as mount) and on backends with no virtualizer.
    pub fn scroll_offset(&self) -> (f32, f32) {
        self.ops.scroll_offset(&*self.node)
    }

    /// Scroll to an absolute pixel offset within the content box. The
    /// off-axis component is ignored. Mirrors
    /// [`ScrollViewHandle::scroll_to`](super::scroll_view::ScrollViewHandle::scroll_to)
    /// so an app that hand-rolled virtualization over a `scroll_view`
    /// can retarget without changing its call sites.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.ops.scroll_to(&*self.node, x, y);
    }
}

/// Backend-side implementation of [`VirtualizerHandle`]'s methods.
///
/// Every method has a no-op default so a backend can adopt the surface
/// incrementally and a backend with no virtualizer at all
/// (`impl VirtualizerOps for FooOps {}`) still compiles. A default
/// `scroll_offset` returning `(0.0, 0.0)` is the same "harmless on a
/// non-scroller" contract `ScrollOps::node_scroll` already uses.
pub trait VirtualizerOps {
    #[allow(unused_variables)]
    fn scroll_to_index(&self, node: &dyn Any, index: usize) {}

    #[allow(unused_variables)]
    fn scroll_offset(&self, node: &dyn Any) -> (f32, f32) {
        (0.0, 0.0)
    }

    #[allow(unused_variables)]
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {}
}

#[cfg(test)]
mod tests {
    //! `VirtualLayout` / `Lanes` lane resolution — the gap-aware autofit
    //! algebra, narrow-container degradation, and the always-report-grid
    //! rule.
    //!
    //! RELOCATED from `runtime-core/src/primitives/virtualizer.rs`
    //! (deletion baseline §4.2, SV-R — flagged there as the
    //! highest-value relocate in the group). The 4 lane-math cases below
    //! are byte-for-byte the old assertions against the same
    //! `Lanes::resolve` / `VirtualLayout::is_grid`, which live HERE.
    //!
    //! The old file also held 3 cases driving the dead
    //! `Bound<VirtualizerHandle>` builder (`.axis` / `.lanes` /
    //! `.spacing` / `.gap` writing into `Element::Virtualizer`). Those
    //! are NOT relocated as-is — the builder they exercise is being
    //! deleted. Their successor on the surviving core is
    //! `runtime-vocabulary/tests/virtualizer_graphics.rs`, which drives
    //! the vocabulary's `VirtualizerPrim` builder and asserts the same
    //! resulting `VirtualLayout`. What DOES belong here is the default
    //! the builder starts from, so it is pinned as data below.

    use super::*;

    /// The resting layout a virtualizer starts from: a vertical,
    /// single-lane, gapless list — i.e. NOT a grid.
    #[test]
    fn default_layout_is_a_vertical_single_lane_list() {
        let l = VirtualLayout::default();
        assert_eq!(l.axis, Axis::Vertical);
        assert_eq!(l.lanes, Lanes::Fixed(1));
        assert_eq!(l.main_spacing, 0.0);
        assert_eq!(l.cross_spacing, 0.0);
        assert!(!l.is_grid());
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
        assert_eq!(Lanes::AutoFit { min_cross: 100.0 }.resolve(540.0, 10.0), 5);
        // One pixel short of fitting a 5th lane → 4.
        assert_eq!(Lanes::AutoFit { min_cross: 100.0 }.resolve(539.0, 10.0), 4);
        // No gaps: floor(cross / min).
        assert_eq!(Lanes::AutoFit { min_cross: 160.0 }.resolve(500.0, 0.0), 3);
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

    /// Even though AutoFit can resolve to one lane at runtime, the author
    /// asked for grid behavior, so `is_grid()` is true.
    #[test]
    fn autofit_is_always_reported_as_grid() {
        let l = VirtualLayout {
            lanes: Lanes::AutoFit { min_cross: 120.0 },
            ..VirtualLayout::default()
        };
        assert!(l.is_grid());
    }
}

