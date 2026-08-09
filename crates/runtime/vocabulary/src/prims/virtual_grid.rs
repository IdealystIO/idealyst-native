//! Virtual-grid payload: the two-axis `virtual_grid` primitive.
//!
//! Sibling of [`VirtualizerPrim`](super::VirtualizerPrim), not a mode
//! of it — see `runtime_shared::primitives::virtual_grid` for why the
//! lane model and the column model can't be the same thing.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::virtual_grid::{CellKey, VirtualGridHandle};
use runtime_scene::Element;

use crate::style_attach::StyleProp;

/// The `virtual_grid` primitive.
///
/// Counts are plain closures so the handler's data effect subscribes
/// to whatever signals they read — the same reactive contract
/// `VirtualizerPrim::item_count` has. Sizes are queried per index when
/// metrics rebuild, so they must be cheap.
pub struct VirtualGridPrim {
    /// Reactive column count — reads inside subscribe the data effect.
    pub col_count: Box<dyn Fn() -> usize>,
    /// Reactive row count — same.
    pub row_count: Box<dyn Fn() -> usize>,
    /// Width of column `c`, in CSS px / native points.
    pub col_width: Rc<dyn Fn(usize) -> f32>,
    /// Height of row `r`.
    pub row_height: Rc<dyn Fn(usize) -> f32>,
    /// Stable identity for a cell; the mounted-cell cache keys off it
    /// so cell state survives a data change that moved the cell.
    pub cell_key: Rc<dyn Fn(usize, usize) -> CellKey>,
    /// Materialize the cell at `(col, row)`. Must be single-root, same
    /// contract as `VirtualizerPrim::render_item`.
    pub render_cell: Rc<dyn Fn(usize, usize) -> Element>,
    /// Buffer factor outside the visible window, in viewport extents,
    /// applied to BOTH axes.
    pub overscan: f32,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(VirtualGridHandle)>>,
    /// Scroll observer for the grid's own scroller.
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}
