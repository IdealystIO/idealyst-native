//! Virtual-grid builder: `virtual_grid()` — the two-axis virtualized
//! collection.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtual_grid::{CellKey, VirtualGridHandle};
use runtime_scene::{item, Element};

use crate::prims::{PrimCell, VirtualGridPrim};
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
/// .overscan(1.0)
/// .on_scroll(move |x, y| header_offset.set(x))
/// ```
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
        },
    }
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

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
