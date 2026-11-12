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
//! ## Frozen panes
//!
//! [`FrozenPanes`] holds rows and columns still at the grid's edges while
//! the rest scrolls beneath them: `top` header rows, `bottom` footer rows,
//! `left` / `right` pinned columns. A frozen row still scrolls sideways
//! with the columns (it is a header, not an overlay), a frozen column
//! still scrolls vertically with the rows, and a cell frozen on both axes
//! (a corner) holds still.
//!
//! The geometry is CSS `position: sticky` against the scrollport, the same
//! on every backend: a frozen cell sits where it would sit unscrolled, and
//! is held at the viewport edge when scrolling would carry it past that
//! edge. So a `right` column of a grid narrower than its viewport stays
//! beside its neighbours instead of jumping to the far edge.
//! [`GridMetrics::display_origin`] is that rule; [`GridMetrics::pane_window`]
//! is the mounted set — the scrolling window plus every frozen cell on the
//! window's rows and columns, so a frozen header always has its cells
//! however far the rows have scrolled. Frozen cells draw above scrolling
//! ones and corners above both (which [`Band`] a cell is in says which).
//!
//! Frozen bands hold the indices at each end of an axis: `top = 1` is row
//! `0`, `bottom = 1` the last row. Bands that would overlap (more frozen
//! rows than rows) are clipped so no index is in two bands.
//!
//! ## Cell reuse
//!
//! A grid built with a reusable renderer (`virtual_grid_recycled`) can hand
//! a cell that left the window to a position that entered it, instead of
//! destroying one cell and building another: the backend asks
//! [`GridRecycling::reuse_class`] which cells may stand in for each other,
//! moves a parked cell of the right class to the new position, and calls
//! [`GridRecycling::rebind_cell`], which updates the cell's reactive
//! position — its content follows, with its native views kept. Backends
//! flush that update before the frame draws, so a reused cell never shows
//! its old row in its new place. A backend that does not reuse cells mounts
//! fresh ones, which renders the same thing.
//!
//! ## Keeping a row mounted
//!
//! [`VirtualGridHandle::keep_row_mounted`] keeps one row's cells mounted
//! however far the grid scrolls from it, at their own position. A cell that
//! leaves the window is otherwise parked, reused for another row or
//! released — and with it goes anything the cell owns, including a gesture
//! in progress: a touch belongs to the view that took the press, and a
//! released view stops reporting it (on the web, even moving a reused cell
//! in the DOM drops its pointer capture). A drag that starts in a cell —
//! a reorder grip — keeps its row mounted until the drag ends, so the
//! gesture survives the grid scrolling under it. [`MountedWindow`] — a
//! [`PaneWindow`] plus the kept row ([`PaneWindow::keeping_row`]) — is the
//! set every backend's window diff mounts.
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

    /// The cells to mount with `frozen` panes: [`visible_window`]
    /// clipped to the scrolling band of each axis, plus every frozen row
    /// and column (see [`PaneWindow`]).
    ///
    /// [`visible_window`]: Self::visible_window
    pub fn pane_window(
        &self,
        scroll: (f32, f32),
        viewport: (f32, f32),
        overscan: f32,
        frozen: FrozenPanes,
    ) -> PaneWindow {
        let (cols, rows) = self.counts();
        let rows_frozen = frozen.row_bands(rows);
        let cols_frozen = frozen.col_bands(cols);
        let w = self.visible_window(scroll, viewport, overscan);
        let clip = |s: usize, e: usize, (lead, trail): (usize, usize), n: usize| {
            let (lo, hi) = (lead, n - trail);
            // An empty scrolling band (everything frozen) reads as 1..=0.
            if lo >= hi {
                return (1, 0);
            }
            let (s, e) = (s.max(lo), e.min(hi - 1));
            if s > e { (1, 0) } else { (s, e) }
        };
        let scrolling = if w.is_empty() {
            GridWindow::EMPTY
        } else {
            let (c0, c1) = clip(w.col_start, w.col_end, cols_frozen, cols);
            let (r0, r1) = clip(w.row_start, w.row_end, rows_frozen, rows);
            GridWindow { col_start: c0, col_end: c1, row_start: r0, row_end: r1 }
        };
        PaneWindow { scrolling, rows_frozen, cols_frozen, counts: (cols, rows) }
    }

    /// Where cell `(col, row)` is drawn, in content space, at `scroll`
    /// with a `viewport`-sized scrollport — its [`cell_origin`] for a
    /// scrolling cell, and for a frozen one the CSS `position: sticky`
    /// rule: held at the viewport edge it would otherwise scroll past.
    /// A backend that positions cells itself (no sticky layout of its own)
    /// moves each frozen cell here on every scroll.
    ///
    /// [`cell_origin`]: Self::cell_origin
    pub fn display_origin(
        &self,
        col: usize,
        row: usize,
        scroll: (f32, f32),
        viewport: (f32, f32),
        frozen: FrozenPanes,
    ) -> (f32, f32) {
        let (cols, rows) = self.counts();
        let (x, y) = self.cell_origin(col, row);
        let (cw, ch) = self.content_size();
        let (left, right) = frozen.col_bands(cols);
        let (top, bottom) = frozen.row_bands(rows);
        let x = sticky(x, Band::of(col, left, right, cols), scroll.0, viewport.0, cw);
        let y = sticky(y, Band::of(row, top, bottom, rows), scroll.1, viewport.1, ch);
        (x, y)
    }

    /// The scroll offset that brings cell `(col, row)` into view at the
    /// scrolling area's leading corner — just past the frozen top rows
    /// and left columns, so it doesn't land under them. An axis on which
    /// the cell is frozen keeps `current`: a frozen cell is always in view
    /// on that axis. Clamped to the scrollable range at `viewport`.
    pub fn scroll_target(
        &self,
        col: usize,
        row: usize,
        current: (f32, f32),
        viewport: (f32, f32),
        frozen: FrozenPanes,
    ) -> (f32, f32) {
        let (cols, rows) = self.counts();
        let (x, y) = self.cell_origin(col, row);
        let (cw, ch) = self.content_size();
        let (left, right) = frozen.col_bands(cols);
        let (top, bottom) = frozen.row_bands(rows);
        let axis = |at: f32, band: Band, lead_extent: f32, cur: f32, content: f32, vp: f32| {
            if band != Band::Scrolling {
                return cur;
            }
            (at - lead_extent).clamp(0.0, (content - vp).max(0.0))
        };
        (
            axis(x, Band::of(col, left, right, cols), at_or_last(&self.col_offsets, left), current.0, cw, viewport.0),
            axis(y, Band::of(row, top, bottom, rows), at_or_last(&self.row_offsets, top), current.1, ch, viewport.1),
        )
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

/// How a backend reuses cells — see the module docs, "Cell reuse". Handed
/// to `GridOps::set_virtual_grid_recycling` once, after create.
#[derive(Clone)]
pub struct GridRecycling {
    /// The class of the cell at `(col, row)`: a parked cell may only be
    /// reused for a position of the same class (a table column's cells, say,
    /// share one structure; a header cell has its own).
    pub reuse_class: Rc<dyn Fn(usize, usize) -> u64>,
    /// Point the mounted cell with this scope id at `(col, row)`. The
    /// backend flushes the update before drawing.
    pub rebind_cell: Rc<dyn Fn(u64, usize, usize)>,
}

/// Rows and columns held still at the grid's edges — see the module
/// docs, "Frozen panes". Counts of indices at each end of an axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FrozenPanes {
    /// Leading rows held at the top edge (a header).
    pub top: usize,
    /// Trailing rows held at the bottom edge (a footer).
    pub bottom: usize,
    /// Leading columns held at the left edge.
    pub left: usize,
    /// Trailing columns held at the right edge.
    pub right: usize,
}

impl FrozenPanes {
    /// No frozen rows or columns: every cell scrolls.
    pub const NONE: Self = Self { top: 0, bottom: 0, left: 0, right: 0 };

    /// Whether anything is frozen.
    pub fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// The row bands for `rows` rows: `(top, bottom)` clipped so they
    /// never overlap.
    fn row_bands(self, rows: usize) -> (usize, usize) {
        clip_bands(self.top, self.bottom, rows)
    }

    /// The column bands for `cols` columns, clipped the same way.
    fn col_bands(self, cols: usize) -> (usize, usize) {
        clip_bands(self.left, self.right, cols)
    }
}

fn clip_bands(lead: usize, trail: usize, count: usize) -> (usize, usize) {
    let lead = lead.min(count);
    (lead, trail.min(count - lead))
}

/// Which band of one axis an index is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Band {
    /// Frozen at the leading edge (top / left).
    Leading,
    /// Scrolls.
    Scrolling,
    /// Frozen at the trailing edge (bottom / right).
    Trailing,
}

impl Band {
    fn of(i: usize, lead: usize, trail: usize, count: usize) -> Self {
        if i < lead {
            Band::Leading
        } else if i >= count - trail {
            Band::Trailing
        } else {
            Band::Scrolling
        }
    }
}

/// The cells a grid with frozen panes mounts: the scrolling window's
/// rows and columns, plus every frozen row and column. The mounted set is
/// the product of the two index sets, so a frozen header row has a cell in
/// every visible column, and a frozen column a cell in every visible row.
/// Built by [`GridMetrics::pane_window`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneWindow {
    /// The scrolling rows and columns in view, clipped to the scrolling
    /// band of each axis (frozen indices are never in it).
    pub scrolling: GridWindow,
    /// `(top, bottom)` frozen rows, clipped (see [`FrozenPanes`]).
    pub rows_frozen: (usize, usize),
    /// `(left, right)` frozen columns, clipped.
    pub cols_frozen: (usize, usize),
    /// `(col_count, row_count)` of the grid.
    pub counts: (usize, usize),
}

impl PaneWindow {
    /// This window, also mounting row `row` (see the module docs, "Keeping
    /// a row mounted") across the window's columns. A row past the end, or
    /// one the window mounts anyway, adds nothing.
    pub fn keeping_row(self, row: Option<usize>) -> MountedWindow {
        let kept_row = row.filter(|&r| r < self.counts.1 && !self.mounts_row(r));
        MountedWindow { pane: self, kept_row }
    }

    /// Whether column `col` is in the frozen bands or the scrolling window.
    fn mounts_col(self, col: usize) -> bool {
        let (left, right) = self.cols_frozen;
        let n = self.counts.0;
        col < left
            || col >= n - right
            || (!self.scrolling.is_empty() && col >= self.scrolling.col_start && col <= self.scrolling.col_end)
    }

    /// Whether row `row` is in the frozen bands or the scrolling window.
    fn mounts_row(self, row: usize) -> bool {
        let (top, bottom) = self.rows_frozen;
        let n = self.counts.1;
        row < top
            || row >= n - bottom
            || (!self.scrolling.is_empty() && row >= self.scrolling.row_start && row <= self.scrolling.row_end)
    }

    /// The rows mounted, ascending, each once.
    pub fn rows(self) -> impl Iterator<Item = usize> {
        let (top, bottom) = self.rows_frozen;
        let n = self.counts.1;
        let (s, e) = (self.scrolling.row_start, self.scrolling.row_end);
        let scroll = if self.scrolling.is_empty() { 1..=0 } else { s..=e };
        (0..top).chain(scroll).chain(n - bottom..n)
    }

    /// The columns mounted, ascending, each once.
    pub fn cols(self) -> impl Iterator<Item = usize> {
        let (left, right) = self.cols_frozen;
        let n = self.counts.0;
        let (s, e) = (self.scrolling.col_start, self.scrolling.col_end);
        let scroll = if self.scrolling.is_empty() { 1..=0 } else { s..=e };
        (0..left).chain(scroll).chain(n - right..n)
    }

    /// Every mounted `(col, row)`, row-major.
    pub fn cells(self) -> impl Iterator<Item = (usize, usize)> {
        self.rows().flat_map(move |r| self.cols().map(move |c| (c, r)))
    }

    /// Whether `(col, row)` is mounted.
    pub fn contains(self, col: usize, row: usize) -> bool {
        let (cols, rows) = self.counts;
        if col >= cols || row >= rows {
            return false;
        }
        let in_axis = |i: usize, (lead, trail): (usize, usize), n: usize, s: usize, e: usize| {
            i < lead || i >= n - trail || (!self.scrolling.is_empty() && i >= s && i <= e)
        };
        in_axis(col, self.cols_frozen, cols, self.scrolling.col_start, self.scrolling.col_end)
            && in_axis(row, self.rows_frozen, rows, self.scrolling.row_start, self.scrolling.row_end)
    }

    /// `(column band, row band)` of a cell: whether it scrolls on each
    /// axis. A backend draws frozen cells above scrolling ones, and a cell
    /// frozen on both axes (a corner) above both.
    pub fn bands(self, col: usize, row: usize) -> (Band, Band) {
        let (cols, rows) = self.counts;
        (
            Band::of(col, self.cols_frozen.0, self.cols_frozen.1, cols),
            Band::of(row, self.rows_frozen.0, self.rows_frozen.1, rows),
        )
    }

    /// Number of mounted cells.
    pub fn len(self) -> usize {
        self.rows().count() * self.cols().count()
    }

    /// Whether nothing is mounted.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// The cells a grid mounts: its [`PaneWindow`] plus a row kept mounted
/// outside it — see the module docs, "Keeping a row mounted". Built by
/// [`PaneWindow::keeping_row`]; the methods mirror [`PaneWindow`]'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MountedWindow {
    pub pane: PaneWindow,
    /// The kept row — only while it is in range and outside the pane
    /// window, so two windows that mount the same cells compare equal.
    pub kept_row: Option<usize>,
}

impl MountedWindow {
    /// The rows mounted, ascending, each once.
    pub fn rows(self) -> impl Iterator<Item = usize> {
        let p = self.pane;
        let (top, bottom) = p.rows_frozen;
        let n = p.counts.1;
        let (s, e) = (p.scrolling.row_start, p.scrolling.row_end);
        let scroll = if p.scrolling.is_empty() { 1..=0 } else { s..=e };
        // The kept row is in the scrolling band but not the window: before
        // the window's rows or after them (either, when the window is empty).
        let kept = self.kept_row;
        let before = kept.filter(|&k| p.scrolling.is_empty() || k < s);
        let after = kept.filter(|&k| !p.scrolling.is_empty() && k > e);
        (0..top).chain(before).chain(scroll).chain(after).chain(n - bottom..n)
    }

    /// The columns mounted, ascending, each once.
    pub fn cols(self) -> impl Iterator<Item = usize> {
        self.pane.cols()
    }

    /// Every mounted `(col, row)`, row-major.
    pub fn cells(self) -> impl Iterator<Item = (usize, usize)> {
        self.rows().flat_map(move |r| self.cols().map(move |c| (c, r)))
    }

    /// Whether `(col, row)` is mounted.
    pub fn contains(self, col: usize, row: usize) -> bool {
        if self.kept_row == Some(row) {
            col < self.pane.counts.0 && self.pane.mounts_col(col)
        } else {
            self.pane.contains(col, row)
        }
    }

    /// `(column band, row band)` of a cell — [`PaneWindow::bands`].
    pub fn bands(self, col: usize, row: usize) -> (Band, Band) {
        self.pane.bands(col, row)
    }

    /// Number of mounted cells.
    pub fn len(self) -> usize {
        self.rows().count() * self.cols().count()
    }

    /// Whether nothing is mounted.
    pub fn is_empty(self) -> bool {
        self.len() == 0
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

/// CSS `position: sticky` on one axis, in content space: a leading cell
/// is held at the scrollport's leading edge (`top: 0` / `left: 0`), a
/// trailing one at its trailing edge (`bottom: 0` / `right: 0`).
fn sticky(at: f32, band: Band, scroll: f32, viewport: f32, content: f32) -> f32 {
    match band {
        Band::Scrolling => at,
        Band::Leading => at + scroll.max(0.0),
        Band::Trailing => at + (scroll + viewport - content).min(0.0),
    }
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

    /// Keep row `row`'s cells mounted however far the grid scrolls from it
    /// (`None` lets the window decide again). One row at a time; a new call
    /// replaces the last. For a gesture that starts in a cell and must
    /// outlive the cell scrolling away — see the module docs, "Keeping a row
    /// mounted".
    pub fn keep_row_mounted(&self, row: Option<usize>) {
        self.ops.keep_row_mounted(&*self.node, row);
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

    /// Mount row `row` beside the window from the next sync on (see
    /// [`PaneWindow::keeping_row`]); `None` stops. Re-window at once, so a
    /// row the window just left is not released first.
    #[allow(unused_variables)]
    fn keep_row_mounted(&self, node: &dyn Any, row: Option<usize>) {}

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

    runtime_tls::thread_local! {
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

    // ---------------------------------------------------------------
    // Frozen panes
    // ---------------------------------------------------------------

    fn frozen(top: usize, bottom: usize, left: usize, right: usize) -> FrozenPanes {
        FrozenPanes { top, bottom, left, right }
    }

    /// A 20-column x 1,000-row table: columns 100 wide, rows 40 tall,
    /// one header row, one pinned column each side.
    fn table() -> GridMetrics {
        GridMetrics::build(20, 1000, &|_| 100.0, &|_| 40.0)
    }

    /// The header row and the pinned columns must stay mounted however far
    /// the grid scrolls — a plain window would drop row 0 as soon as it
    /// scrolled out, and the header would vanish.
    #[test]
    fn frozen_rows_and_columns_stay_mounted_far_from_the_origin() {
        let m = table();
        let f = frozen(1, 0, 1, 1);
        let w = m.pane_window((800.0, 20_000.0), (400.0, 200.0), 0.0, f);
        let rows: Vec<usize> = w.rows().collect();
        let cols: Vec<usize> = w.cols().collect();
        assert_eq!(rows.first(), Some(&0), "header row mounted: {rows:?}");
        assert!(rows[1..].iter().all(|r| (500..=505).contains(r)), "{rows:?}");
        assert_eq!(cols.first(), Some(&0), "left pin mounted: {cols:?}");
        assert_eq!(cols.last(), Some(&19), "right pin mounted: {cols:?}");
        assert!(w.contains(0, 0) && w.contains(19, 0) && w.contains(10, 502));
        assert!(!w.contains(10, 1), "a scrolled-away body row is not mounted");
        // The product of the two sets, each cell once.
        assert_eq!(w.cells().count(), rows.len() * cols.len());
        assert_eq!(w.len(), rows.len() * cols.len());
    }

    /// At the origin the scrolling window starts at row 0 / column 0,
    /// which are frozen: they must be mounted ONCE, not once per band.
    #[test]
    fn frozen_indices_are_never_in_the_scrolling_window() {
        let m = table();
        let w = m.pane_window((0.0, 0.0), (400.0, 200.0), 0.0, frozen(1, 0, 1, 1));
        assert_eq!((w.scrolling.row_start, w.scrolling.col_start), (1, 1));
        let rows: Vec<usize> = w.rows().collect();
        let mut dedup = rows.clone();
        dedup.dedup();
        assert_eq!(rows, dedup);
        let cells: Vec<(usize, usize)> = w.cells().collect();
        let set: std::collections::BTreeSet<_> = cells.iter().copied().collect();
        assert_eq!(set.len(), cells.len(), "a cell mounted twice");
    }

    #[test]
    fn bands_say_which_cells_hold_still() {
        let m = table();
        let w = m.pane_window((0.0, 0.0), (400.0, 200.0), 0.0, frozen(1, 1, 1, 1));
        assert_eq!(w.bands(0, 0), (Band::Leading, Band::Leading));
        assert_eq!(w.bands(5, 0), (Band::Scrolling, Band::Leading));
        assert_eq!(w.bands(19, 3), (Band::Trailing, Band::Scrolling));
        assert_eq!(w.bands(5, 999), (Band::Scrolling, Band::Trailing));
        assert_eq!(w.bands(5, 3), (Band::Scrolling, Band::Scrolling));
    }

    /// The sticky rule, per band: a leading cell rides the scroll offset
    /// (held at the top/left edge), a trailing one is held at the bottom/
    /// right edge, a scrolling one stays at its origin.
    #[test]
    fn display_origin_holds_frozen_cells_at_the_viewport_edges() {
        let m = table(); // content 2000 x 40000
        let f = frozen(1, 1, 1, 1);
        let (scroll, vp) = ((300.0, 1000.0), (500.0, 400.0));
        assert_eq!(m.display_origin(0, 0, scroll, vp, f), (300.0, 1000.0), "corner");
        assert_eq!(m.display_origin(5, 0, scroll, vp, f), (500.0, 1000.0), "header scrolls sideways");
        assert_eq!(m.display_origin(0, 30, scroll, vp, f), (300.0, 1200.0), "left pin scrolls down");
        // Right pin: origin x 1900, held so its right edge meets the
        // viewport's (300 + 500 = 800): 800 - 100 = 700.
        assert_eq!(m.display_origin(19, 30, scroll, vp, f).0, 700.0);
        // Bottom row: origin y 39960, held at 1000 + 400 - 40 = 1360.
        assert_eq!(m.display_origin(5, 999, scroll, vp, f).1, 1360.0);
        assert_eq!(m.display_origin(5, 30, scroll, vp, f), (500.0, 1200.0), "body cell");
    }

    /// CSS sticky: a trailing column of a grid NARROWER than its viewport
    /// sits beside its neighbours, not at the viewport's far edge.
    #[test]
    fn trailing_frozen_column_of_a_narrow_grid_stays_in_place() {
        let m = GridMetrics::build(3, 10, &|_| 100.0, &|_| 40.0); // 300 wide
        let f = frozen(0, 0, 0, 1);
        assert_eq!(m.display_origin(2, 0, (0.0, 0.0), (900.0, 200.0), f).0, 200.0);
    }

    /// Scrolling to a row lands it just under the frozen header, not
    /// beneath it; an axis the cell is frozen on keeps its offset.
    #[test]
    fn scroll_target_clears_the_frozen_header_and_columns() {
        let m = table();
        let f = frozen(1, 0, 2, 0);
        let vp = (500.0, 400.0);
        assert_eq!(m.scroll_target(10, 100, (0.0, 0.0), vp, f), (800.0, 3960.0));
        assert_eq!(m.scroll_target(0, 100, (123.0, 0.0), vp, f), (123.0, 3960.0), "frozen column: x kept");
        assert_eq!(m.scroll_target(10, 0, (0.0, 77.0), vp, f), (800.0, 77.0), "header row: y kept");
        // Clamped at the end of the content.
        assert_eq!(m.scroll_target(19, 999, (0.0, 0.0), vp, f), (1500.0, 39600.0));
    }

    /// More frozen rows than rows: the bands are clipped, nothing is
    /// mounted twice, and every row is frozen.
    #[test]
    fn overlapping_frozen_bands_are_clipped() {
        let m = GridMetrics::build(2, 3, &|_| 10.0, &|_| 10.0);
        let w = m.pane_window((0.0, 0.0), (100.0, 100.0), 0.0, frozen(2, 5, 0, 0));
        assert_eq!(w.rows_frozen, (2, 1));
        assert_eq!(w.rows().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(w.scrolling.is_empty());
    }

    #[test]
    fn no_frozen_panes_is_the_plain_window() {
        let m = table();
        let (s, v) = ((350.0, 900.0), (400.0, 200.0));
        let plain = m.visible_window(s, v, 0.5);
        let w = m.pane_window(s, v, 0.5, FrozenPanes::NONE);
        assert_eq!(w.scrolling, plain);
        assert_eq!(w.cells().collect::<Vec<_>>(), plain.cells().collect::<Vec<_>>());
        assert_eq!(m.display_origin(7, 30, s, v, FrozenPanes::NONE), m.cell_origin(7, 30));
    }

    /// A kept row stays mounted, across the window's columns, however far
    /// the window scrolls from it — above it or below it — and is listed in
    /// row order, once.
    #[test]
    fn a_kept_row_stays_mounted_outside_the_window() {
        let m = table();
        let f = frozen(1, 0, 1, 0);
        // Window around rows 500..=505; row 3 kept from far above.
        let w = m.pane_window((800.0, 20_000.0), (400.0, 200.0), 0.0, f).keeping_row(Some(3));
        assert_eq!(w.kept_row, Some(3));
        let rows: Vec<usize> = w.rows().collect();
        assert_eq!(&rows[..2], &[0, 3], "header, then the kept row, then the window: {rows:?}");
        assert!(rows[2..].iter().all(|r| (500..=505).contains(r)), "{rows:?}");
        assert!(w.contains(0, 3) && w.contains(10, 3), "frozen and scrolling columns of the kept row");
        assert!(!w.contains(1, 3), "only the window's columns");
        assert_eq!(w.cells().count(), rows.len() * w.cols().count());

        // Kept from far below.
        let w = m.pane_window((0.0, 0.0), (400.0, 200.0), 0.0, f).keeping_row(Some(900));
        let rows: Vec<usize> = w.rows().collect();
        assert_eq!(rows.last(), Some(&900), "{rows:?}");
        let mut sorted = rows.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(rows, sorted, "ascending, each once");
    }

    /// Keeping a row the window mounts anyway, or one past the end, changes
    /// nothing — so the windows compare equal and a backend skips the diff.
    #[test]
    fn keeping_a_mounted_or_missing_row_adds_nothing() {
        let m = table();
        let w = m.pane_window((0.0, 0.0), (400.0, 200.0), 0.0, frozen(1, 0, 0, 0));
        let plain = w.keeping_row(None);
        assert_eq!(plain.kept_row, None);
        assert_eq!(w.keeping_row(Some(2)), plain, "in the window");
        assert_eq!(w.keeping_row(Some(0)), plain, "frozen");
        assert_eq!(w.keeping_row(Some(1000)), plain, "past the end");
        assert_ne!(w.keeping_row(Some(700)), plain, "a row outside the window is a different mounted set");
        // Without a kept row the mounted set is the pane window's.
        assert_eq!(plain.cells().collect::<Vec<_>>(), w.cells().collect::<Vec<_>>());
        assert!((0..20).all(|c| (0..10).all(|r| plain.contains(c, r) == w.contains(c, r))));
    }

    /// With nothing in the scrolling window, the kept row is mounted
    /// between the frozen bands, in the frozen columns.
    #[test]
    fn a_kept_row_is_mounted_beside_an_empty_window() {
        let m = GridMetrics::build(2, 10, &|_| 10.0, &|_| 10.0);
        let mut w = m.pane_window((0.0, 0.0), (20.0, 100.0), 0.0, frozen(1, 1, 1, 0));
        w.scrolling = GridWindow::EMPTY;
        let w = w.keeping_row(Some(4));
        assert_eq!(w.rows().collect::<Vec<_>>(), vec![0, 4, 9]);
        assert!(w.contains(0, 4));
        assert_eq!(w.cells().count(), 3, "the frozen column on each mounted row");
    }

    #[test]
    fn empty_grid_with_frozen_panes_mounts_nothing() {
        let m = GridMetrics::build(0, 0, &|_| 10.0, &|_| 10.0);
        let w = m.pane_window((0.0, 0.0), (100.0, 100.0), 1.0, frozen(1, 1, 1, 1));
        assert!(w.is_empty());
        assert_eq!(w.cells().count(), 0);
    }
}
