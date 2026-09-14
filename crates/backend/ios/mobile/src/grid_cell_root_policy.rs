//! Pure decision logic for how the layout pass treats a Taffy root that
//! is a mounted `virtual_grid` cell.
//!
//! Un-gated (compiles on any host) so the regression test runs from any
//! platform — the `splice_policy` / `portal_policy` /
//! `layout_drain_policy` pattern. The UIKit half — recording a cell's
//! box in `virtual_grid::CELL_BOXES` before the view attaches, and the
//! pass consulting it — cannot run in a host test binary (no UIKit
//! classes), and is verified on the simulator: a 5×4 lattice of 40×44
//! cells measures 40×44 at its content-space origins.
//!
//! # Why this exists
//!
//! A mounted cell's root view has no Taffy PARENT — the grid engine
//! places it in the scroller's content space, not in the layout tree —
//! so it is a root, and the pass did two things to a root that are
//! exactly wrong for a cell: it computed it against the viewport, and
//! then wrote the result over the frame the engine had just set. A
//! 40×44 cell came back 40×956 at the origin, every cell of the grid
//! stacked in one column. The cell's children are ordinary entries in
//! the apply loop and keep their frames, which are relative to the cell.

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

/// What the pass does with one root.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RootPass {
    /// The box Taffy computes the root against.
    pub compute_against: (f32, f32),
    /// `true` when the grid engine owns the root's frame and the apply
    /// loop must leave it alone.
    pub engine_owns_frame: bool,
}

/// `cell_box` is `virtual_grid::cell_box(view_key)` — `Some` only for a
/// mounted grid cell's root view; `viewport` is the host bounds every
/// other root is computed against.
pub(crate) fn root_pass(cell_box: Option<(f32, f32)>, viewport: (f32, f32)) -> RootPass {
    match cell_box {
        Some(cell) => RootPass {
            compute_against: cell,
            engine_owns_frame: true,
        },
        None => RootPass {
            compute_against: viewport,
            engine_owns_frame: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a cell root computed against the viewport came back
    /// full-screen-tall, and the apply loop then moved it to the origin.
    #[test]
    fn regression_grid_cell_root_is_computed_against_its_box_and_left_where_the_engine_put_it() {
        let pass = root_pass(Some((40.0, 44.0)), (402.0, 956.0));
        assert_eq!(pass.compute_against, (40.0, 44.0), "computed against the screen");
        assert!(pass.engine_owns_frame, "the pass would write (0, 0, w, h) over the cell");
    }

    /// The control: every other root — the framework root, a screen
    /// mounted by `mount_screen_in_vc` — still fills the viewport and
    /// takes the frame Taffy computed.
    #[test]
    fn an_ordinary_root_fills_the_viewport_and_is_framed_by_the_pass() {
        let pass = root_pass(None, (402.0, 956.0));
        assert_eq!(pass.compute_against, (402.0, 956.0));
        assert!(!pass.engine_owns_frame);
    }
}
