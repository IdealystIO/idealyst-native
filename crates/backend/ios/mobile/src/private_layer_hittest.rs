//! Pure geometry for the screen_recorder `PrivateLayer` passthrough hit-test,
//! kept un-gated (no `target_os`) so its regression coverage runs from any
//! host. The objc traversal in `imp::callbacks::private_layer_blocks_touch`
//! builds a [`HitNode`] tree from the live `UIView` subtree and delegates here.
//! The UIKit reads (frame / installed-handler / backgroundColor) are not
//! host-testable, but the recursion + per-level coordinate conversion — where
//! the "can't draw at all" bug actually hid — is.
//!
//! Background: the PrivateLayer overlay lives in its own `UIWindow` above the
//! app window. Its content is a viewport-spanning TRANSPARENT flex container
//! with the toolbar / recording-preview nested sparsely inside. A non-recursive
//! "is the point inside any DIRECT child's frame" check (the portal
//! `OverlayPassthroughView`) therefore reported a hit for EVERY point and
//! swallowed all canvas-area touches. The fix descends the subtree and reports
//! a hit only where the touch lands on a view that actually wants it — a
//! control (touch handler installed) or visible content (non-clear background)
//! — passing transparent containers through.

/// A view in the passthrough subtree: its `frame` in the PARENT's coordinate
/// space (`x`/`y`/`w`/`h`), whether the view ITSELF captures touches
/// (`captures` — a touch handler is installed or it paints a non-clear
/// background), and its children.
// Consumed by `imp::callbacks` (ios-only) and the tests below; on a host
// non-test lib build neither references it, hence the allow.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HitNode {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub captures: bool,
    pub children: Vec<HitNode>,
}

#[cfg(test)]
impl HitNode {
    fn leaf(x: f64, y: f64, w: f64, h: f64, captures: bool) -> Self {
        HitNode { x, y, w, h, captures, children: Vec::new() }
    }
    fn container(x: f64, y: f64, w: f64, h: f64, children: Vec<HitNode>) -> Self {
        HitNode { x, y, w, h, captures: false, children }
    }
}

/// Does `point` (`px`, `py`) — in the coordinate space of the parent that holds
/// `nodes` — land on a node that should CAPTURE the touch?
///
/// For each node containing the point: descend into its children (converting
/// the point into the child's coordinate space by subtracting the child's frame
/// origin); if any descendant captures, this node does. Otherwise the node
/// captures iff its own `captures` flag is set. A transparent container
/// (`captures == false`) with no capturing descendant under the point returns
/// false, so the touch falls through — the property that makes the empty canvas
/// area drawable while the toolbar still blocks.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn region_blocks_touch(nodes: &[HitNode], px: f64, py: f64) -> bool {
    for n in nodes {
        let inside =
            px >= n.x && px < n.x + n.w && py >= n.y && py < n.y + n.h;
        if !inside {
            continue;
        }
        let (lx, ly) = (px - n.x, py - n.y);
        if region_blocks_touch(&n.children, lx, ly) {
            return true;
        }
        if n.captures {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models the actual bug shape: a full-screen TRANSPARENT container (the
    /// PrivateLayer content root) holding a toolbar pinned to the bottom. A tap
    /// in the middle (the canvas area) must fall THROUGH; a tap on the toolbar
    /// must be captured. Before the fix the container reported a hit everywhere
    /// and the middle tap was swallowed.
    #[test]
    fn regression_canvas_area_falls_through_full_screen_transparent_container() {
        // 400x800 screen. Toolbar: a capturing bar 360 wide, 60 tall, near the
        // bottom (y = 720), inside a full-screen transparent container.
        let tree = vec![HitNode::container(
            0.0,
            0.0,
            400.0,
            800.0,
            vec![HitNode::leaf(20.0, 720.0, 360.0, 60.0, true)],
        )];
        // Canvas area (middle) → falls through (the regression).
        assert!(!region_blocks_touch(&tree, 200.0, 390.0));
        assert!(!region_blocks_touch(&tree, 60.0, 120.0));
        // On the toolbar → captured.
        assert!(region_blocks_touch(&tree, 200.0, 740.0));
    }

    /// A bare capturing leaf blocks inside its frame and passes outside it.
    #[test]
    fn capturing_leaf_blocks_inside_only() {
        let tree = vec![HitNode::leaf(10.0, 10.0, 100.0, 100.0, true)];
        assert!(region_blocks_touch(&tree, 50.0, 50.0));
        assert!(!region_blocks_touch(&tree, 5.0, 5.0));
        assert!(!region_blocks_touch(&tree, 200.0, 200.0));
    }

    /// Coordinate conversion is applied at each level: a deeply-nested
    /// capturing grandchild is hit at the right ABSOLUTE point only after both
    /// ancestor frame origins are subtracted.
    #[test]
    fn nested_coordinate_conversion() {
        // container at (100,100) → child container at (10,20) → leaf at (5,5)
        // size 30x30. Absolute leaf rect origin = (100+10+5, 100+20+5) =
        // (115, 125), size 30x30.
        let tree = vec![HitNode::container(
            100.0,
            100.0,
            300.0,
            300.0,
            vec![HitNode::container(
                10.0,
                20.0,
                200.0,
                200.0,
                vec![HitNode::leaf(5.0, 5.0, 30.0, 30.0, true)],
            )],
        )];
        assert!(region_blocks_touch(&tree, 120.0, 130.0)); // inside the leaf
        assert!(!region_blocks_touch(&tree, 105.0, 105.0)); // in containers only
    }

    /// A transparent container with a transparent child captures nothing.
    #[test]
    fn fully_transparent_subtree_never_blocks() {
        let tree = vec![HitNode::container(
            0.0,
            0.0,
            400.0,
            800.0,
            vec![HitNode::leaf(0.0, 0.0, 400.0, 800.0, false)],
        )];
        assert!(!region_blocks_touch(&tree, 200.0, 400.0));
    }

    #[test]
    fn empty_tree_never_blocks() {
        assert!(!region_blocks_touch(&[], 0.0, 0.0));
    }
}
