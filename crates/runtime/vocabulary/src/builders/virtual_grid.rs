//! Virtual-grid builder: `virtual_grid()` — the two-axis virtualized
//! collection.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtual_grid::{CellKey, FrozenPanes, VirtualGridHandle};
use runtime_scene::{item, Element};

use crate::prims::{PrimCell, VirtualGridPrim, VirtualGridRecycle};
use crate::style_attach::IntoStyleProp;

/// Start a `virtual_grid` — a grid of `col_count × row_count` cells
/// that scrolls on **both** axes and mounts only the cells overlapping
/// the viewport.
///
/// The five required inputs are positional because none of them has a
/// sensible default: the counts, the two size functions, and the cell
/// renderer are the grid. Everything else chains.
///
/// ```ignore
/// virtual_grid(
///     move || days.get().len(),        // columns
///     move || crew.get().len(),        // rows
///     |_col| 120.0,                    // every column 120 wide
///     |_row| 44.0,                     // every row 44 tall
///     move |c, r| cell_id(c, r),       // stable identity
///     move |c, r| ui! { ScheduleCell(day = c, crew = r) },
/// )
/// .frozen(FrozenPanes { top: 1, left: 1, ..FrozenPanes::NONE })
/// .overscan(1.0)
/// .on_scroll(move |x, y| header_offset.set(x))
/// ```
///
/// # Sizing
///
/// By default the grid **fills** its parent, like every scroll viewport,
/// and scrolls whatever does not fit. To make it **fit its content**,
/// for example exactly as tall as its rows while it scrolls sideways,
/// style it `flex_grow: 0` + `flex_basis: auto`. The grid then
/// reports its content size, plus the scrollbar the other axis needs
/// where scrollbars take space (a browser with classic scrollbars). The
/// last row is never covered by the horizontal bar, and the grid does
/// not scroll vertically. Native scrollers overlay their content, so
/// there the fit is exactly the rows. Add `max_height` to cap the fit.
/// Don't pin a parent to `rows × row_height`: that leaves no room for
/// the scrollbar.
///
/// The size functions are read reactively: a width that follows a
/// signal (columns sharing the visible width) re-lays the grid out when
/// it changes, as a count does.
///
/// Reach for [`virtualizer`](super::virtualizer) instead when only one
/// axis scrolls: its lanes wrap items across a fixed viewport, which
/// is a different (and cheaper) model than scrollable columns.
pub fn virtual_grid(
    col_count: impl Fn() -> usize + 'static,
    row_count: impl Fn() -> usize + 'static,
    col_width: impl Fn(usize) -> f32 + 'static,
    row_height: impl Fn(usize) -> f32 + 'static,
    cell_key: impl Fn(usize, usize) -> CellKey + 'static,
    render_cell: impl Fn(usize, usize) -> Element + 'static,
) -> VirtualGridBuilder {
    VirtualGridBuilder {
        prim: VirtualGridPrim {
            col_count: Box::new(col_count),
            row_count: Box::new(row_count),
            col_width: Rc::new(col_width),
            row_height: Rc::new(row_height),
            cell_key: Rc::new(cell_key),
            render_cell: Rc::new(render_cell),
            overscan: 1.0,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
            on_scroll: None,
            frozen: FrozenPanes::NONE,
            recycle: None,
        },
    }
}

/// [`virtual_grid`] with REUSABLE cells: `render` reads the cell's position
/// from a signal, so a cell that scrolls out can be pointed at a position
/// that scrolls in, keeping its native views — only what depends on the
/// position updates. `reuse_class(col, row)` says which cells may stand in
/// for each other: cells built the same way (a table column's body cells)
/// share a class, a cell built differently (its header) gets its own. A
/// cell's content must read everything that varies by position through the
/// signal, and react to data changes on its own: a reused cell is never
/// rebuilt, and `cell_key` no longer forces a rebuild. Backends that don't
/// reuse cells mount fresh ones with the same renderer, so the result is
/// the same everywhere — see the primitive's module docs, "Cell reuse".
///
/// ```ignore
/// virtual_grid_recycled(
///     move || cols.len(),
///     move || rows.get().len(),
///     |_| 120.0,
///     |_| 44.0,
///     |c, r| (c * 1_000_003 + r) as u64,
///     |c, _| c as u64,                       // every cell of a column alike
///     move |at| ui! { text(move || label(at.get())) },
/// )
/// ```
pub fn virtual_grid_recycled(
    col_count: impl Fn() -> usize + 'static,
    row_count: impl Fn() -> usize + 'static,
    col_width: impl Fn(usize) -> f32 + 'static,
    row_height: impl Fn(usize) -> f32 + 'static,
    cell_key: impl Fn(usize, usize) -> CellKey + 'static,
    reuse_class: impl Fn(usize, usize) -> u64 + 'static,
    render: impl Fn(runtime_world::ReadSignal<(usize, usize)>) -> Element + 'static,
) -> VirtualGridBuilder {
    let render: Rc<dyn Fn(runtime_world::ReadSignal<(usize, usize)>) -> Element> = Rc::new(render);
    // The fixed-position form, for hosts that don't reuse: the same
    // renderer over a position that never moves (created in the cell's
    // own scope by the handler, like everything the renderer builds).
    let fixed = {
        let render = render.clone();
        move |c: usize, r: usize| render(runtime_world::signal((c, r)).read_only())
    };
    let mut b = virtual_grid(col_count, row_count, col_width, row_height, cell_key, fixed);
    b.prim.recycle = Some(VirtualGridRecycle { reuse_class: Rc::new(reuse_class), render });
    b
}

pub struct VirtualGridBuilder {
    prim: VirtualGridPrim,
}

impl VirtualGridBuilder {
    /// Buffer outside the visible window, in viewport extents, applied
    /// to both axes. Default `1.0`.
    ///
    /// Two-axis overscan costs more than the 1-D kind: widening by one
    /// viewport on each axis roughly *nines* the mounted set (3× on
    /// each axis), where a list only triples it. Start lower here than
    /// you would for a list.
    pub fn overscan(mut self, factor: f32) -> Self {
        self.prim.overscan = factor;
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(VirtualGridHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    /// Observe the grid's scroll offset. Both components are
    /// meaningful (unlike the 1-D primitives, where the off-axis one
    /// is always `0.0`) — this is what a frozen header or a synced
    /// pane reads. Fires at scroll frequency; keep it cheap.
    pub fn on_scroll(mut self, handler: impl Fn(f32, f32) + 'static) -> Self {
        self.prim.on_scroll = Some(Rc::new(handler));
        self
    }

    /// Hold rows and columns still at the grid's edges while the rest
    /// scrolls beneath them — a header row, pinned columns:
    /// `.frozen(FrozenPanes { top: 1, left: 1, ..FrozenPanes::NONE })`.
    /// A frozen row still scrolls sideways with the columns and a frozen
    /// column vertically with the rows; a cell frozen on both holds still.
    /// Frozen cells draw above scrolling ones, so give them a background.
    /// Same sticky geometry on every backend — see the primitive's module
    /// docs, "Frozen panes".
    pub fn frozen(mut self, frozen: FrozenPanes) -> Self {
        self.prim.frozen = frozen;
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
