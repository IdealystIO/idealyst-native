//! `virtual_grid` — the two-axis virtualized grid.
//!
//! # Why this is a separate primitive
//!
//! [`virtualizer`](super::virtualizer) scrolls on exactly one axis. Its
//! cross axis is *lanes*, which **divide the viewport's cross extent**
//! (`laneCross = (cross - gaps) / lanes`) — a wrap model, not a second
//! scroll axis. `Lanes::Fixed(364)` doesn't give 364 scrollable
//! columns; it gives 364 lanes about a pixel wide each. Its size model
//! agrees: `ItemSize` is one scalar, the *main-axis* extent, because
//! the cross extent is never the author's to choose.
//!
//! A spreadsheet / schedule grid needs the opposite on the cross axis:
//! columns with author-chosen widths whose total exceeds the viewport
//! and therefore scrolls. That contradicts `Lanes` rather than
//! extending it, and it needs a second size function. Hence a sibling
//! primitive instead of a mode flag — the existing `virtualizer` API
//! is untouched and nothing migrates.
//!
//! It stays a *primitive* (vocabulary, not a scene-`Registry`
//! extension) for the same reason `virtualizer` does: native cell
//! recycling — `UICollectionView`, `NSCollectionView`, `RecyclerView` —
//! cannot be composed from `view` / `scroll_view`.
//!
//! # The model
//!
//! A grid of `col_count() × row_count()` cells. Column `c` is
//! `col_width(c)` wide for every row; row `r` is `row_height(r)` tall
//! for every column. Both axes scroll, and only the cells overlapping
//! the (buffered) viewport are mounted — so the mounted-cell count is
//! bounded by the viewport, not by the data.
//!
//! That is the whole point: a 30-crew × 364-day schedule is 10,920
//! cells, of which a viewport holds perhaps 60.
//!
//! ## Deliberate scope limits
//!
//! - **Sizes are per-column and per-row, not per-cell.** A cell's box
//!   is the intersection of its column and its row. Per-cell sizing is
//!   a spanning/masonry problem with a different engine.
//! - **No `Measured` mode.** [`virtualizer`](super::virtualizer)
//!   supports measure-on-mount because a 1-D list only has to
//!   reconcile one axis. Here a measured cell height would have to
//!   agree with every other cell in its row (and width with its
//!   column), so "measure the cell" is not a well-defined input to the
//!   layout. Sizes are author-supplied. If you need content-derived
//!   sizes, measure your data once and feed the result to
//!   `col_width` / `row_height`.
//!
//! ## The grid's own size
//!
//! The grid fills its parent by default, and it can fit its content
//! instead (`flex_grow: 0` + `flex_basis: auto`). The fitted size is
//! [`GridMetrics::intrinsic_size`]: the content plus any scrollbar the
//! other axis needs. Native backends report it to layout through
//! `runtime_layout::grid_intrinsic_measure`. The web grid's
//! `overflow: auto` box gets the same size from the browser. Fit the grid
//! with those styles, not with a height computed from
//! [`VirtualGridHandle::scrollport`]: the backend already knows the
//! scrollbar when it lays the grid out, and a size the app reads back and
//! feeds into the grid's own height is a layout feedback loop.
//!
//! ## The scrollport
//!
//! [`VirtualGridHandle::scrollport`] / [`VirtualGridHandle::on_scrollport`]
//! report the box the cells show through: the grid's size minus any
//! scrollbar that takes room from it, plus the thickness of each such
//! scrollbar ([`Scrollport`]). Use it for what is VISIBLE — how many
//! columns fit, sizing columns to fill the visible width, how close the
//! view is to an edge. Measuring a wrapper view instead counts a classic
//! scrollbar as visible content.
//!
//! Every backend reports it the same way. Where scrollbars overlay the
//! content (iOS, Android, AppKit's overlay style, a browser with overlay
//! scrollbars) both thicknesses are `0` and the scrollport is the grid's
//! frame. Where they take room (a browser with classic scrollbars, AppKit's
//! legacy style), the scrollport is the inner box (`clientWidth` /
//! `clientHeight` on the web, the clip view's frame on AppKit) and the
//! thicknesses are what the bars took.

use std::any::Any;
use std::rc::Rc;

use crate::LayoutSubscription;

/// Stable identity for a cell, used for keyed reuse across data
/// changes. Same contract (and same `u64` shape) as
/// [`ItemKey`](super::virtualizer::ItemKey).
pub type CellKey = u64;

// ===========================================================================
// Callbacks
// ===========================================================================

/// Callbacks handed to `GridOps::create_virtual_grid`.
///
/// Lives here rather than in [`crate::host`] (where
/// [`VirtualizerCallbacks`](crate::VirtualizerCallbacks) sits for
/// historical reasons) so the whole primitive — callbacks, metrics,
/// handle, ops — reads as one module.
///
/// Everything is `Rc` so a backend can clone into per-event closures.
/// Generic over the backend's `Node` so `mount_cell` returns the real
/// native node with no type erasure.
pub struct GridCallbacks<N: Clone + 'static> {
    /// Current column count. Re-queried on data-changed.
    pub col_count: Rc<dyn Fn() -> usize>,
    /// Current row count. Re-queried on data-changed.
    pub row_count: Rc<dyn Fn() -> usize>,
    /// Width of column `c`, in CSS px / native points. Called for
    /// every column when metrics are rebuilt, so keep it cheap — a
    /// lookup, not a computation over the row set.
    pub col_width: Rc<dyn Fn(usize) -> f32>,
    /// Height of row `r`, same units and same cost expectation.
    pub row_height: Rc<dyn Fn(usize) -> f32>,
    /// Stable identity for cell `(col, row)`. The backend uses it to
    /// keep a mounted cell alive across a data change that moved it.
    pub cell_key: Rc<dyn Fn(usize, usize) -> CellKey>,
    /// Mount cell `(col, row)`: build its subtree inside a fresh
    /// per-cell scope. Returns the native node plus the scope id, which
    /// the backend holds alongside its pooled cell so it can call
    /// `release_cell` later.
    ///
    /// Same ambient contract as the virtualizer's `mount_item`: the
    /// backend MUST invoke this inside `World::enter` (it realizes, and
    /// creation-side work like `theme_ctx` → `inject` aborts outside),
    /// and MUST NOT invoke it synchronously inside
    /// `create_virtual_grid` (the backend is mutably borrowed there).
    pub mount_cell: Rc<dyn Fn(usize, usize) -> (N, u64)>,
    /// Release a previously-mounted cell by scope id. Drops the scope,
    /// freeing every signal/effect inside the cell's subtree. The
    /// backend must also detach the node.
    pub release_cell: Rc<dyn Fn(u64)>,
    /// Author's scroll observer, if any. Called with the grid's
    /// `(x, y)` offset in CSS px / native points — the same contract
    /// and coordinate space as `scroll_view` and `virtualizer`, so all
    /// three report offsets identically. `None` when unset; backends
    /// should then install no scroll observation at all.
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

// ===========================================================================
// Metrics + windowing — the shared engine core
// ===========================================================================

/// Prefix-summed axis extents, and the visible-window search over them.
///
/// **Every backend's engine uses this.** The 1-D virtualizer's
/// equivalent search lives inside the web JS shim and is re-derived by
/// each native backend's collection-view layout; that duplication is
/// exactly what produced four copies of the sticky pin math (see
/// [`crate::sticky`]). Here the arithmetic is written and tested once,
/// and backends supply only the mechanism — how a cell is positioned
/// and recycled.
///
/// Rebuild via [`GridMetrics::build`] whenever the counts or sizes
/// change; the cost is O(cols + rows), not O(cells).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridMetrics {
    /// `col_offsets[c]` is the x origin of column `c`; the vec has
    /// `col_count + 1` entries, so the last is the total content
    /// width.
    pub col_offsets: Vec<f32>,
    /// `row_offsets[r]` is the y origin of row `r`; `row_count + 1`
    /// entries, last is the total content height.
    pub row_offsets: Vec<f32>,
}

impl GridMetrics {
    /// Build prefix sums from the per-axis size functions.
    pub fn build(
        col_count: usize,
        row_count: usize,
        col_width: &dyn Fn(usize) -> f32,
        row_height: &dyn Fn(usize) -> f32,
    ) -> Self {
        Self {
            col_offsets: prefix_sums(col_count, col_width),
            row_offsets: prefix_sums(row_count, row_height),
        }
    }

    /// Total content size `(width, height)`. This is what the backend
    /// gives its scroller as the scrollable extent.
    pub fn content_size(&self) -> (f32, f32) {
        (
            self.col_offsets.last().copied().unwrap_or(0.0),
            self.row_offsets.last().copied().unwrap_or(0.0),
        )
    }

    /// Origin `(x, y)` of cell `(col, row)` in content space.
    /// Out-of-range indices clamp to the content edge rather than
    /// panicking — a backend can legitimately ask about a cell that a
    /// concurrent data change just removed.
    pub fn cell_origin(&self, col: usize, row: usize) -> (f32, f32) {
        (at_or_last(&self.col_offsets, col), at_or_last(&self.row_offsets, row))
    }

    /// Size `(width, height)` of cell `(col, row)`. Zero for an
    /// out-of-range index, same rationale as [`cell_origin`](Self::cell_origin).
    pub fn cell_size(&self, col: usize, row: usize) -> (f32, f32) {
        (span(&self.col_offsets, col), span(&self.row_offsets, row))
    }

    /// The cells overlapping `viewport` at `scroll`, widened by
    /// `overscan` viewports on every side.
    ///
    /// `scroll` and `viewport` are `(x, y)` / `(width, height)` in
    /// content-space units. The result is a half-open-on-the-outside
    /// *inclusive* range pair — see [`GridWindow`].
    pub fn visible_window(
        &self,
        scroll: (f32, f32),
        viewport: (f32, f32),
        overscan: f32,
    ) -> GridWindow {
        let (cols, rows) = self.counts();
        if cols == 0 || rows == 0 {
            return GridWindow::EMPTY;
        }
        let (c0, c1) = axis_range(&self.col_offsets, cols, scroll.0, viewport.0, overscan);
        let (r0, r1) = axis_range(&self.row_offsets, rows, scroll.1, viewport.1, overscan);
        GridWindow {
            col_start: c0,
            col_end: c1,
            row_start: r0,
            row_end: r1,
        }
    }

    /// `(col_count, row_count)` these metrics were built for.
    pub fn counts(&self) -> (usize, usize) {
        (
            self.col_offsets.len().saturating_sub(1),
            self.row_offsets.len().saturating_sub(1),
        )
    }

    /// The grid's **intrinsic size** — the outer box `(width, height)`
    /// that shows all of its content on every axis that is free to
    /// hug, including the scrollbar the OTHER axis needs.
    ///
    /// This is what lets a grid fit its content ("exactly as tall as
    /// its rows, scrolling sideways") with plain layout styles instead
    /// of an author-side scrollbar allowance: a native backend reports
    /// it to the layout engine as the grid's measured content size,
    /// and the browser computes the same thing for the web grid's
    /// `overflow: auto` box (a horizontal scrollbar adds its thickness
    /// to an auto height). Native backends whose scrollers overlay the
    /// content (UIKit, Android, AppKit's overlay style) pass a `gutter`
    /// of `0.0`.
    ///
    /// - `known` — the dimensions the layout engine has already fixed
    ///   (a stretched cross axis, an author `width`/`height`). They
    ///   are returned unchanged.
    /// - `available` — the definite space the grid is laid out in, per
    ///   axis; `None` when that axis is unbounded (max-/min-content).
    ///   An axis scrolls when its content exceeds this box.
    /// - `gutter` — thickness one scrollbar takes from the box.
    ///
    /// A scrollbar on one axis takes `gutter` from the other axis's
    /// box, which can push THAT axis into scrolling too — content that
    /// fit by less than a scrollbar's thickness. The two checks below
    /// settle that cascade; it can't flip back, since a scrollbar only
    /// ever removes space.
    pub fn intrinsic_size(
        &self,
        known: (Option<f32>, Option<f32>),
        available: (Option<f32>, Option<f32>),
        gutter: f32,
    ) -> (f32, f32) {
        let (cw, ch) = self.content_size();
        let g = if gutter.is_finite() { gutter.max(0.0) } else { 0.0 };
        let box_w = known.0.or(available.0);
        let box_h = known.1.or(available.1);
        let overflows = |content: f32, extent: Option<f32>, taken: f32| {
            extent.is_some_and(|e| content > e - taken)
        };
        let mut scroll_x = overflows(cw, box_w, 0.0);
        let mut scroll_y = overflows(ch, box_h, 0.0);
        // Cascade: each axis re-checked against the space the other's
        // scrollbar leaves it. Two rounds reach the fixed point.
        for _ in 0..2 {
            scroll_x = scroll_x || (scroll_y && overflows(cw, box_w, g));
            scroll_y = scroll_y || (scroll_x && overflows(ch, box_h, g));
        }
        (
            known.0.unwrap_or(cw + if scroll_y { g } else { 0.0 }),
            known.1.unwrap_or(ch + if scroll_x { g } else { 0.0 }),
        )
    }
}

/// An inclusive rectangle of cell indices. `col_start > col_end` (or
/// the row equivalent) means "no cells" — see [`GridWindow::is_empty`],
/// and prefer iterating with [`GridWindow::cells`], which handles that
/// case for you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridWindow {
    pub col_start: usize,
    pub col_end: usize,
    pub row_start: usize,
    pub row_end: usize,
}

impl GridWindow {
    /// The canonical empty window. Uses `1..=0` rather than `0..=0`
    /// because `0..=0` is one cell, not zero — the off-by-one that
    /// makes an empty grid mount a phantom cell.
    pub const EMPTY: Self = Self {
        col_start: 1,
        col_end: 0,
        row_start: 1,
        row_end: 0,
    };

    pub fn is_empty(self) -> bool {
        self.col_start > self.col_end || self.row_start > self.row_end
    }

    /// Number of cells the window covers.
    pub fn len(self) -> usize {
        if self.is_empty() {
            return 0;
        }
        (self.col_end - self.col_start + 1) * (self.row_end - self.row_start + 1)
    }

    /// Iterate `(col, row)` in row-major order. Empty windows yield
    /// nothing.
    pub fn cells(self) -> impl Iterator<Item = (usize, usize)> {
        let (cs, ce, rs, re) = if self.is_empty() {
            (1, 0, 1, 0)
        } else {
            (self.col_start, self.col_end, self.row_start, self.row_end)
        };
        (rs..=re).flat_map(move |r| (cs..=ce).map(move |c| (c, r)))
    }

    pub fn contains(self, col: usize, row: usize) -> bool {
        !self.is_empty()
            && col >= self.col_start
            && col <= self.col_end
            && row >= self.row_start
            && row <= self.row_end
    }
}

fn prefix_sums(count: usize, size: &dyn Fn(usize) -> f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(count + 1);
    out.push(0.0);
    let mut acc = 0.0_f32;
    for i in 0..count {
        // Negative or non-finite sizes would corrupt the prefix sums
        // into a non-monotonic array, and the binary search below
        // assumes monotonicity — a bad author size would turn into a
        // wrong window rather than a visibly wrong cell.
        let s = size(i);
        acc += if s.is_finite() && s > 0.0 { s } else { 0.0 };
        out.push(acc);
    }
    out
}

fn at_or_last(offsets: &[f32], i: usize) -> f32 {
    offsets
        .get(i)
        .copied()
        .unwrap_or_else(|| offsets.last().copied().unwrap_or(0.0))
}

fn span(offsets: &[f32], i: usize) -> f32 {
    match (offsets.get(i), offsets.get(i + 1)) {
        (Some(a), Some(b)) => b - a,
        _ => 0.0,
    }
}

/// First/last index on one axis overlapping the buffered viewport.
fn axis_range(
    offsets: &[f32],
    count: usize,
    scroll: f32,
    viewport: f32,
    overscan: f32,
) -> (usize, usize) {
    let buffer = (viewport * overscan.max(0.0)).max(0.0);
    let start_at = (scroll - buffer).max(0.0);
    let end_at = scroll + viewport + buffer;
    let start = index_at_offset(offsets, count, start_at);
    let end = index_at_offset(offsets, count, end_at);
    (start, end)
}

/// Largest `i` with `offsets[i] <= at`, clamped to `0..=count-1`.
/// Binary search — this runs per scroll event, so it must not be a
/// scan over the axis.
fn index_at_offset(offsets: &[f32], count: usize, at: f32) -> usize {
    if count == 0 {
        return 0;
    }
    let mut lo = 0usize;
    let mut hi = count - 1;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if offsets[mid] <= at {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

// ===========================================================================
// Handle
// ===========================================================================

/// Imperative handle for a mounted `virtual_grid`. Same surface shape
/// as [`VirtualizerHandle`](super::virtualizer::VirtualizerHandle),
/// widened to two axes.
#[derive(Clone)]
pub struct VirtualGridHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn VirtualGridOps,
}

impl VirtualGridHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn VirtualGridOps) -> Self {
        Self { node, ops }
    }

    /// Scroll so cell `(col, row)` is at the grid's leading corner.
    pub fn scroll_to_cell(&self, col: usize, row: usize) {
        self.ops.scroll_to_cell(&*self.node, col, row);
    }

    /// Current scroll offset `(x, y)`. Both components are meaningful
    /// here — unlike the 1-D primitives, where the off-axis one is
    /// always `0.0`.
    pub fn scroll_offset(&self) -> (f32, f32) {
        self.ops.scroll_offset(&*self.node)
    }

    /// Scroll to an absolute `(x, y)` offset in content space.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.ops.scroll_to(&*self.node, x, y);
    }

    /// The box the cells show through right now — the grid's size minus
    /// any scrollbar that takes room — and each scrollbar's thickness.
    /// `None` until the grid has been laid out. See [`Scrollport`].
    pub fn scrollport(&self) -> Option<Scrollport> {
        self.ops.scrollport(&*self.node)
    }

    /// Call `f` with the grid's [`Scrollport`] now (when the grid is laid
    /// out) and again whenever it changes: the grid resizes, or a scrollbar
    /// appears, disappears or changes style. Never called twice in a row
    /// with the same value. Drop the returned subscription to stop.
    ///
    /// For reading what is visible. Don't feed it back into the grid's own
    /// size — fit the grid with `flex_grow: 0` + `flex_basis: auto`
    /// instead (module docs).
    pub fn on_scrollport<F: Fn(Scrollport) + 'static>(&self, f: F) -> LayoutSubscription {
        // The change guard lives HERE, once, so every backend's delivery
        // (ResizeObserver, a layout pass, a clip-view notification — all of
        // which can fire at an unchanged size) reaches the author the same
        // way.
        let last: Rc<std::cell::Cell<Option<Scrollport>>> = Rc::new(std::cell::Cell::new(None));
        let f = Rc::new(f);
        let deliver = {
            let last = last.clone();
            let f = f.clone();
            move |s: Scrollport| {
                if last.get() != Some(s) {
                    last.set(Some(s));
                    f(s);
                }
            }
        };
        let sub = self.ops.subscribe_scrollport(&*self.node, Box::new(deliver.clone()));
        if let Some(now) = self.scrollport() {
            deliver(now);
        }
        sub
    }
}

/// The box a grid's cells show through, in CSS px / native points.
///
/// `width` × `height` is the visible area: the grid's frame minus its
/// borders and minus the room any scrollbar takes. `scrollbar_width` is the
/// thickness the VERTICAL scrollbar takes from the width, and
/// `scrollbar_height` the thickness the HORIZONTAL one takes from the
/// height — `0.0` where there is no bar on that axis or the bar overlays
/// the content (every native backend's default, see the module docs).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Scrollport {
    pub width: f32,
    pub height: f32,
    pub scrollbar_width: f32,
    pub scrollbar_height: f32,
}

impl Scrollport {
    /// A scrollport whose scrollbars overlay the content: the visible box
    /// is the whole `width` × `height`.
    pub fn overlay(width: f32, height: f32) -> Self {
        Self { width, height, scrollbar_width: 0.0, scrollbar_height: 0.0 }
    }
}

/// Backend-side implementation of [`VirtualGridHandle`]'s methods.
/// Every method defaults to a harmless no-op so a backend without a
/// grid engine still satisfies the trait.
pub trait VirtualGridOps {
    #[allow(unused_variables)]
    fn scroll_to_cell(&self, node: &dyn Any, col: usize, row: usize) {}

    #[allow(unused_variables)]
    fn scroll_offset(&self, node: &dyn Any) -> (f32, f32) {
        (0.0, 0.0)
    }

    #[allow(unused_variables)]
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {}

    /// The grid's current [`Scrollport`]; `None` before the grid is laid
    /// out (or on a backend without a grid engine).
    #[allow(unused_variables)]
    fn scrollport(&self, node: &dyn Any) -> Option<Scrollport> {
        None
    }

    /// Call `callback` with the grid's [`Scrollport`] after each layout or
    /// scrollbar change that may have changed it. May over-fire with an
    /// unchanged value — [`VirtualGridHandle::on_scrollport`] filters
    /// those. Backends must schedule the post-dispatch flush after calling
    /// it, as `on_layout` delivery does (FRAMEWORK-NOTES #103).
    #[allow(unused_variables)]
    fn subscribe_scrollport(
        &self,
        node: &dyn Any,
        callback: Box<dyn Fn(Scrollport)>,
    ) -> LayoutSubscription {
        LayoutSubscription::noop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 5-column × 4-row grid: columns 100 wide, rows 50 tall.
    fn metrics() -> GridMetrics {
        GridMetrics::build(5, 4, &|_| 100.0, &|_| 50.0)
    }

    #[test]
    fn content_size_is_the_sum_of_both_axes() {
        assert_eq!(metrics().content_size(), (500.0, 200.0));
    }

    #[test]
    fn cell_geometry_comes_from_the_two_axes_independently() {
        let m = GridMetrics::build(3, 3, &|c| [10.0, 20.0, 30.0][c], &|r| [5.0, 15.0, 25.0][r]);
        assert_eq!(m.cell_origin(2, 2), (30.0, 20.0));
        assert_eq!(m.cell_size(2, 2), (30.0, 25.0));
        assert_eq!(m.cell_origin(0, 0), (0.0, 0.0));
    }

    /// The property the whole primitive exists for: the mounted set is
    /// bounded by the VIEWPORT, not by the data. A 364-column year is
    /// the case that made `Lanes` unusable — as lanes it would be 364
    /// one-pixel columns, and as a mounted set it would be every cell.
    #[test]
    fn regression_mounted_cell_count_must_not_grow_with_the_data() {
        let big = GridMetrics::build(364, 30, &|_| 120.0, &|_| 40.0);
        // A 900×600 viewport with no overscan.
        let w = big.visible_window((0.0, 0.0), (900.0, 600.0), 0.0);
        assert_eq!(w.len(), 8 * 16, "window must be viewport-sized");
        assert!(
            w.len() < 200,
            "10,920 cells in the data, {} mounted",
            w.len()
        );
    }

    #[test]
    fn window_tracks_scroll_on_both_axes_independently() {
        let m = metrics();
        // Scrolled to x=250 (mid column 2), y=75 (mid row 1), viewport
        // 200×50, no overscan.
        let w = m.visible_window((250.0, 75.0), (200.0, 50.0), 0.0);
        assert_eq!((w.col_start, w.col_end), (2, 4));
        assert_eq!((w.row_start, w.row_end), (1, 2));
    }

    #[test]
    fn overscan_widens_the_window_on_both_axes() {
        let m = metrics();
        let tight = m.visible_window((200.0, 100.0), (100.0, 50.0), 0.0);
        let loose = m.visible_window((200.0, 100.0), (100.0, 50.0), 1.0);
        assert!(loose.len() > tight.len());
        assert!(loose.col_start <= tight.col_start && loose.col_end >= tight.col_end);
        assert!(loose.row_start <= tight.row_start && loose.row_end >= tight.row_end);
    }

    /// An empty grid must mount NOTHING. `0..=0` is one cell, not
    /// zero, so a naive empty window renders a phantom cell at the
    /// origin — the reason `GridWindow::EMPTY` is `1..=0`.
    #[test]
    fn regression_empty_grid_mounts_a_phantom_cell() {
        let empty = GridMetrics::build(0, 0, &|_| 100.0, &|_| 50.0);
        let w = empty.visible_window((0.0, 0.0), (900.0, 600.0), 1.0);
        assert!(w.is_empty());
        assert_eq!(w.len(), 0);
        assert_eq!(w.cells().count(), 0);

        // Zero on ONE axis is just as empty as zero on both.
        let no_rows = GridMetrics::build(10, 0, &|_| 100.0, &|_| 50.0);
        assert_eq!(no_rows.visible_window((0.0, 0.0), (900.0, 600.0), 1.0).len(), 0);
    }

    /// The window must never run past the data, however far the
    /// scroller has been dragged — an over-scrolled offset (rubber-band,
    /// or a stale offset after the data shrank) would otherwise index
    /// a column that no longer exists.
    #[test]
    fn regression_overscrolled_offset_indexes_past_the_data() {
        let m = metrics();
        let w = m.visible_window((99_999.0, 99_999.0), (200.0, 100.0), 2.0);
        assert_eq!(w.col_end, 4, "clamped to the last column");
        assert_eq!(w.row_end, 3, "clamped to the last row");
        for (c, r) in w.cells() {
            assert!(c < 5 && r < 4);
        }
    }

    /// Variable sizes: the binary search must land on the column that
    /// actually contains the offset, not an evenly-spaced guess.
    #[test]
    fn window_search_handles_variable_extents() {
        // Columns 10, 200, 10, 200 → offsets 0, 10, 210, 220, 420.
        let m = GridMetrics::build(4, 1, &|c| if c % 2 == 0 { 10.0 } else { 200.0 }, &|_| 10.0);
        let w = m.visible_window((215.0, 0.0), (1.0, 10.0), 0.0);
        assert_eq!(w.col_start, 2, "offset 215 falls in column 2 (210..220)");
    }

    /// A negative or NaN size from author code must not corrupt the
    /// prefix sums — the binary search assumes a monotonic array, so a
    /// non-monotonic one returns a silently wrong window rather than a
    /// visibly wrong cell.
    #[test]
    fn regression_bad_author_size_breaks_the_window_search() {
        let m = GridMetrics::build(4, 1, &|c| if c == 1 { -50.0 } else { 100.0 }, &|_| 10.0);
        let offs = &m.col_offsets;
        assert!(
            offs.windows(2).all(|w| w[1] >= w[0]),
            "prefix sums must stay monotonic: {offs:?}"
        );
        let m2 = GridMetrics::build(2, 1, &|_| f32::NAN, &|_| 10.0);
        assert!(m2.col_offsets.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn cells_iterates_row_major_and_contains_agrees() {
        let w = GridWindow { col_start: 1, col_end: 2, row_start: 5, row_end: 6 };
        assert_eq!(
            w.cells().collect::<Vec<_>>(),
            vec![(1, 5), (2, 5), (1, 6), (2, 6)]
        );
        assert!(w.contains(2, 6));
        assert!(!w.contains(0, 6));
        assert!(!GridWindow::EMPTY.contains(0, 0));
    }

    // -----------------------------------------------------------------
    // intrinsic_size — what a fit-to-content grid reports to layout.
    // The CrewForge report: a grid exactly as tall as its rows that
    // scrolls sideways lost its last row to a classic horizontal
    // scrollbar, and the app had to guess a SCROLLBAR_ALLOWANCE.
    // -----------------------------------------------------------------

    /// 10 columns × 120 = 1200 wide, 3 rows × 40 = 120 tall.
    fn wide_grid() -> GridMetrics {
        GridMetrics::build(10, 3, &|_| 120.0, &|_| 40.0)
    }

    /// Regression (CrewForge, "a grid cannot fit its content"): a grid
    /// wider than its 390-wide box hugs its rows PLUS the horizontal
    /// scrollbar, so the rows never lose space to it and no vertical
    /// scroll appears.
    #[test]
    fn regression_fit_height_includes_the_horizontal_scrollbar() {
        let (w, h) = wide_grid().intrinsic_size((Some(390.0), None), (Some(390.0), None), 15.0);
        assert_eq!((w, h), (390.0, 135.0));
    }

    /// Overlay scrollers (every native backend) take no space: the hug
    /// height is exactly the rows — no allowance to subtract on a Mac.
    #[test]
    fn overlay_scrollbars_add_nothing() {
        let (_, h) = wide_grid().intrinsic_size((Some(390.0), None), (Some(390.0), None), 0.0);
        assert_eq!(h, 120.0);
    }

    /// Content that fits on both axes reserves no gutter at all.
    #[test]
    fn content_that_fits_reserves_no_gutter() {
        let size = wide_grid().intrinsic_size((None, None), (Some(2000.0), Some(800.0)), 15.0);
        assert_eq!(size, (1200.0, 120.0));
    }

    /// An unbounded axis never scrolls, so it hugs its content.
    #[test]
    fn unbounded_axes_hug_content() {
        assert_eq!(wide_grid().intrinsic_size((None, None), (None, None), 15.0), (1200.0, 120.0));
    }

    /// The cascade: rows that fit the 130 box by less than a scrollbar
    /// (120 + 15 > 130) are pushed into scrolling by the horizontal bar,
    /// which then puts a vertical bar on the hugging WIDTH axis.
    #[test]
    fn a_scrollbar_that_steals_the_last_pixels_cascades_to_the_other_axis() {
        let m = GridMetrics::build(10, 3, &|_| 120.0, &|_| 40.0);
        let (w, h) = m.intrinsic_size((None, Some(130.0)), (Some(390.0), None), 15.0);
        assert_eq!(h, 130.0, "known height is returned unchanged");
        // Width box 390 < 1200 → horizontal bar → 120 > 130 - 15 → vertical bar.
        assert_eq!(w, 1215.0, "hugging width adds the vertical bar the cascade created");
    }

    /// A known dimension always wins, and a nonsense gutter is ignored.
    #[test]
    fn known_dimensions_win_and_bad_gutters_are_ignored() {
        let m = wide_grid();
        assert_eq!(m.intrinsic_size((Some(50.0), Some(60.0)), (None, None), 15.0), (50.0, 60.0));
        assert_eq!(m.intrinsic_size((Some(390.0), None), (None, None), f32::NAN).1, 120.0);
        assert_eq!(m.intrinsic_size((Some(390.0), None), (None, None), -4.0).1, 120.0);
    }

    // -----------------------------------------------------------------
    // Scrollport — the handle plumbing. The CrewForge report (10-05):
    // `VirtualGridHandle` exposed no client size or scrollbar thickness,
    // so a grid's visible width was measured off a wrapper view, which
    // counts a classic scrollbar as visible content.
    // -----------------------------------------------------------------

    use std::cell::RefCell;

    thread_local! {
        /// What the fake backend reports, and the delivery it holds.
        static PORT: std::cell::Cell<Option<Scrollport>> = const { std::cell::Cell::new(None) };
        static DELIVER: RefCell<Option<Box<dyn Fn(Scrollport)>>> = const { RefCell::new(None) };
        static UNSUBSCRIBED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    struct FakeOps;
    impl VirtualGridOps for FakeOps {
        fn scrollport(&self, _: &dyn Any) -> Option<Scrollport> {
            PORT.with(|p| p.get())
        }
        fn subscribe_scrollport(&self, _: &dyn Any, cb: Box<dyn Fn(Scrollport)>) -> LayoutSubscription {
            DELIVER.with(|d| *d.borrow_mut() = Some(cb));
            LayoutSubscription::new(|| {
                UNSUBSCRIBED.with(|u| u.set(true));
                DELIVER.with(|d| *d.borrow_mut() = None);
            })
        }
    }
    static FAKE_OPS: FakeOps = FakeOps;

    /// A backend resizing the grid / showing a bar.
    fn backend_reports(s: Scrollport) {
        PORT.with(|p| p.set(Some(s)));
        DELIVER.with(|d| {
            if let Some(cb) = d.borrow().as_ref() {
                cb(s)
            }
        });
    }

    fn fake_handle() -> VirtualGridHandle {
        PORT.with(|p| p.set(None));
        DELIVER.with(|d| *d.borrow_mut() = None);
        UNSUBSCRIBED.with(|u| u.set(false));
        VirtualGridHandle::new(Rc::new(()), &FAKE_OPS)
    }

    /// Regression (CrewForge, "VirtualGridHandle exposes no scrollport
    /// client size or scrollbar thickness"): the handle answers with what
    /// the backend measured, bars included.
    #[test]
    fn regression_handle_reports_the_scrollport_and_scrollbar_thickness() {
        let h = fake_handle();
        assert_eq!(h.scrollport(), None, "not laid out yet");
        let classic = Scrollport { width: 385.0, height: 105.0, scrollbar_width: 15.0, scrollbar_height: 15.0 };
        PORT.with(|p| p.set(Some(classic)));
        assert_eq!(h.scrollport(), Some(classic));
    }

    /// `on_scrollport` delivers the current value at once, then only
    /// CHANGES — a backend re-firing at the same size (every layout pass,
    /// a ResizeObserver's initial observation) reaches the author once.
    #[test]
    fn on_scrollport_delivers_now_then_only_changes() {
        let h = fake_handle();
        PORT.with(|p| p.set(Some(Scrollport::overlay(400.0, 120.0))));
        let seen: Rc<RefCell<Vec<Scrollport>>> = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let sub = h.on_scrollport(move |p| s.borrow_mut().push(p));
        assert_eq!(*seen.borrow(), vec![Scrollport::overlay(400.0, 120.0)], "current value at subscribe");

        backend_reports(Scrollport::overlay(400.0, 120.0));
        assert_eq!(seen.borrow().len(), 1, "an unchanged re-fire is filtered");

        // A classic horizontal bar appears: same frame, smaller box.
        let barred = Scrollport { width: 400.0, height: 105.0, scrollbar_width: 0.0, scrollbar_height: 15.0 };
        backend_reports(barred);
        assert_eq!(seen.borrow().last(), Some(&barred));
        assert_eq!(seen.borrow().len(), 2);

        drop(sub);
        assert!(UNSUBSCRIBED.with(|u| u.get()), "dropping the subscription releases the backend's");
    }

    /// Subscribing before layout delivers nothing until the backend has
    /// a size — no zero-sized placeholder reaches the author.
    #[test]
    fn on_scrollport_before_layout_waits_for_the_first_measurement() {
        let h = fake_handle();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let _sub = h.on_scrollport(move |p| s.borrow_mut().push(p));
        assert!(seen.borrow().is_empty());
        backend_reports(Scrollport::overlay(300.0, 90.0));
        assert_eq!(*seen.borrow(), vec![Scrollport::overlay(300.0, 90.0)]);
    }

    /// A backend without a grid engine answers `None` and never fires.
    #[test]
    fn the_default_ops_report_no_scrollport() {
        struct Bare;
        impl VirtualGridOps for Bare {}
        static BARE: Bare = Bare;
        let h = VirtualGridHandle::new(Rc::new(()), &BARE);
        assert_eq!(h.scrollport(), None);
        let fired = Rc::new(std::cell::Cell::new(false));
        let f = fired.clone();
        let _sub = h.on_scrollport(move |_| f.set(true));
        assert!(!fired.get());
    }
}
