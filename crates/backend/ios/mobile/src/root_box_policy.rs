//! Pure decision logic for the box the layout pass computes a Taffy root
//! against, and whether the pass frames that root.
//!
//! Un-gated (compiles on any host) so the regression tests run from any
//! platform — the `splice_policy` / `portal_policy` /
//! `layout_drain_policy` pattern. The UIKit half — reading a cell's box
//! from `virtual_grid::CELL_BOXES`, and a screen's box from the view
//! controller view it is pinned into (`imp::vc_container_box`) — cannot
//! run in a host test binary (no UIKit classes), and is verified on the
//! simulator.
//!
//! # Why a root is not always computed against the viewport
//!
//! A root is a view with no Taffy PARENT. Three kinds exist:
//!
//! - **The framework root**, pinned to the host view. Its box is the
//!   viewport.
//! - **A mounted `virtual_grid` cell.** The grid engine places it in the
//!   scroller's content space. Computed against the viewport, a 40×44
//!   cell came back 40×956, and the apply loop then wrote that over the
//!   frame the engine had set — every cell stacked in one column at the
//!   origin. The cell's children are ordinary entries in the apply loop
//!   and keep their frames, which are relative to the cell.
//! - **A screen mounted in a `UIViewController`** (`mount_screen_in_vc`:
//!   stack pushes, tab and drawer screens, modals). Auto Layout pins it
//!   to the controller's view, which UIKit sizes to the navigator's
//!   container — not the window. Computed against the viewport, the
//!   screen's subtree was laid out taller than the box UIKit held it in:
//!   inside an app-level `safe_area` the bottom of every stack screen ran
//!   past its container by the inset total and was clipped (49 pt on a
//!   12.9" iPad).

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
/// mounted grid cell's root view. `container_box` is the bounds of the
/// view controller view the root is pinned into — `Some` only for a
/// VC-mounted screen. `viewport` is the host bounds every other root is
/// computed against.
///
/// An empty container box (a controller view UIKit has not sized yet)
/// falls back to the viewport: Taffy against `0×0` would collapse the
/// screen, and the controller view's first real size re-runs the pass.
pub(crate) fn root_pass(
    cell_box: Option<(f32, f32)>,
    container_box: Option<(f32, f32)>,
    viewport: (f32, f32),
) -> RootPass {
    if let Some(cell) = cell_box {
        return RootPass {
            compute_against: cell,
            engine_owns_frame: true,
        };
    }
    let compute_against = match container_box {
        Some((w, h)) if w > 0.0 && h > 0.0 => (w, h),
        _ => viewport,
    };
    RootPass {
        compute_against,
        engine_owns_frame: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a cell root computed against the viewport came back
    /// full-screen-tall, and the apply loop then moved it to the origin.
    #[test]
    fn regression_grid_cell_root_is_computed_against_its_box_and_left_where_the_engine_put_it() {
        let pass = root_pass(Some((40.0, 44.0)), None, (402.0, 956.0));
        assert_eq!(pass.compute_against, (40.0, 44.0), "computed against the screen");
        assert!(pass.engine_owns_frame, "the pass would write (0, 0, w, h) over the cell");
    }

    /// Regression: a stack screen inside an app-level `safe_area` sits in
    /// a controller view 1317 pt tall on a 1366 pt iPad window. Computed
    /// against the window, its content overflowed by 49 pt and the
    /// footer was clipped.
    #[test]
    fn regression_vc_mounted_screen_is_computed_against_its_controller_view_not_the_window() {
        let pass = root_pass(None, Some((1024.0, 1317.0)), (1024.0, 1366.0));
        assert_eq!(pass.compute_against, (1024.0, 1317.0), "computed against the window");
        assert!(!pass.engine_owns_frame, "a screen root still takes the frame Taffy computed");
    }

    /// A controller view UIKit has not sized yet must not collapse the
    /// screen to nothing; the viewport stands in until its real size
    /// lands and re-runs the pass.
    #[test]
    fn an_unsized_controller_view_falls_back_to_the_viewport() {
        for empty in [(0.0, 0.0), (1024.0, 0.0), (0.0, 1317.0)] {
            let pass = root_pass(None, Some(empty), (1024.0, 1366.0));
            assert_eq!(pass.compute_against, (1024.0, 1366.0), "empty box {empty:?}");
        }
    }

    /// The framework root — no cell, no controller view — fills the
    /// viewport and takes the frame Taffy computed.
    #[test]
    fn the_framework_root_fills_the_viewport_and_is_framed_by_the_pass() {
        let pass = root_pass(None, None, (402.0, 956.0));
        assert_eq!(pass.compute_against, (402.0, 956.0));
        assert!(!pass.engine_owns_frame);
    }
}
