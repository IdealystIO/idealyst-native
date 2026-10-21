//! Scroll-view geometry shared by the paint walk and the click /
//! wheel hit walks: content extent, per-axis offset clamping, and
//! `Position::Sticky` pinning.
//!
//! Every walker computes a node's on-screen cell position the same
//! way (frame + translate, floored per level, minus the enclosing
//! scroll offset); sticky adds one more term to that position. The
//! pin arithmetic is `runtime_shared::sticky` — the same function
//! iOS / macOS / the GPU engine use — so a sticky header or frozen
//! column lands where it does on every other backend. This module
//! only supplies its inputs: the child's natural origin in the
//! scroller's content space (Taffy frames summed up to the scroll
//! node), the scroller's offset, and its viewport, all in layout px.

use runtime_layout::LayoutNode;
use runtime_shared::sticky::{translate, StickyInsets};

use crate::node::{NodeData, NodeKind};
use crate::TerminalBackend;

/// The nearest enclosing scroll view, threaded down each walk so a
/// sticky descendant can pin against it without an ancestor search.
/// A nested scroll view replaces it for its own subtree (a sticky
/// child pins against its NEAREST scroller, as in CSS).
#[derive(Clone, Copy, Debug)]
pub(crate) struct EnclosingScroll {
    /// The scroll view's Taffy node — where the natural-origin sum stops.
    layout: LayoutNode,
    /// Current `(scroll_x, scroll_y)`, in layout px.
    offset: (f32, f32),
    /// The scroll view's frame `(width, height)` (the scrollport), in
    /// layout px.
    viewport: (f32, f32),
}

impl TerminalBackend {
    /// The scroll context `data`'s children see: `data` itself when it
    /// is a scroll view, else the inherited one.
    pub(crate) fn child_scroll_context(
        &self,
        data: &NodeData,
        inherited: Option<EnclosingScroll>,
    ) -> Option<EnclosingScroll> {
        if !matches!(data.kind, NodeKind::ScrollView) {
            return inherited;
        }
        let (cw, ch) = self.cell_size;
        let frame = self.layout.frame_of(data.layout);
        Some(EnclosingScroll {
            layout: data.layout,
            offset: (data.scroll_x * cw, data.scroll_y * ch),
            viewport: (frame.width, frame.height),
        })
    }

    /// The `(dx, dy)` cell shift a `Position::Sticky` node gets from its
    /// enclosing scroll view — `(0, 0)` for any other node, and for a
    /// sticky node with no scroll ancestor (CSS: acts as relative).
    /// Added to the node's position by the paint walk and both hit
    /// walks alike, so a pinned header is clicked where it is painted.
    pub(crate) fn sticky_shift(&self, data: &NodeData, scroll: Option<EnclosingScroll>) -> (f32, f32) {
        let Some(scroll) = scroll else { return (0.0, 0.0) };
        let Some(style) = data.style.as_ref() else { return (0.0, 0.0) };
        if !matches!(style.position, Some(runtime_shared::Position::Sticky)) {
            return (0.0, 0.0);
        }
        // Natural origin from Taffy frames, never from the painted
        // position (which an ancestor's pin or translate may have moved).
        let mut natural = (0.0_f32, 0.0_f32);
        let mut cursor = data.layout;
        while cursor != scroll.layout {
            let f = self.layout.frame_of(cursor);
            natural.0 += f.x;
            natural.1 += f.y;
            match self.layout.parent_of(cursor) {
                Some(parent) => cursor = parent,
                None => return (0.0, 0.0),
            }
        }
        let frame = self.layout.frame_of(data.layout);
        let (dx, dy) = translate(
            StickyInsets::from_style(style),
            natural,
            (frame.width, frame.height),
            scroll.offset,
            scroll.viewport,
        );
        let (cw, ch) = self.cell_size;
        (dx / cw, dy / ch)
    }

    /// The scroll view's content `(width, height)` in cells — the
    /// shared [`LayoutTree::scroll_content_extent`](runtime_layout::LayoutTree::scroll_content_extent)
    /// (every descendant, unclipped on each scrolling axis, clipped to
    /// the frame on a non-scrolling one). Drives offset clamping and
    /// the scrollbar thumbs.
    pub(crate) fn scroll_content_cells(&self, data: &NodeData) -> (f32, f32) {
        let (cw, ch) = self.cell_size;
        let (w, h) = self.layout.scroll_content_extent(data.layout);
        (w / cw, h / ch)
    }
}

/// `offset + delta` on one axis, clamped to `[0, max]` — or held at 0
/// when `axis` doesn't scroll that way (a vertical scroller with an
/// over-wide row must never slide sideways).
pub(crate) fn step_offset(scrolls: bool, offset: f32, delta: f32, max: f32) -> f32 {
    if scrolls {
        (offset + delta).clamp(0.0, max)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClickOutcome, TermNode};
    use runtime_scene::Host;
    use runtime_shared::color::Rgba;
    use runtime_shared::{accessibility::AccessibilityProps, Length, ScrollAxis, StyleRules, Tokenized};
    use runtime_vocabulary::caps::{LifecycleOps, PressableOps, ScrollOps, StyleOps, ViewOps};
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<(f32, f32)>>>;
    const RED: Rgba = Rgba { r: 255, g: 0, b: 0, a: 255 };

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }

    fn sized(w: f32, h: f32) -> Rc<StyleRules> {
        Rc::new(StyleRules { width: px(w), height: px(h), ..Default::default() })
    }

    /// A 10×6-cell root holding a `scroll_view(axis)` that fills it,
    /// whose 30×20 content view holds — when `sticky` — a 3×2 red
    /// pressable at (6, 4) with `position: Sticky, top: 0, left: 0`
    /// (its clicks push `(-1, -1)` onto the log). Content overflows the
    /// viewport by (20, 14) on every axis the scroller scrolls.
    fn scroller(axis: ScrollAxis, sticky: bool) -> (TerminalBackend, TermNode, Log) {
        let mut be = TerminalBackend::new();
        be.set_viewport(10, 6);
        let a11y = AccessibilityProps::default();
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut root = be.create_view(&a11y);
        be.apply_style(&root, &sized(10.0, 6.0));
        let l = log.clone();
        let mut sv = be.create_scroll_view(
            axis,
            Some(Rc::new(move |x, y| l.borrow_mut().push((x, y)))),
            &a11y,
        );
        let mut content = be.create_view(&a11y);
        // `flex_shrink: 0`: a declared height is not a floor (only width
        // is, see `runtime_layout`), so the column would otherwise shrink
        // the content to the 6-row viewport.
        be.apply_style(
            &content,
            &Rc::new(StyleRules {
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..(*sized(30.0, 20.0)).clone()
            }),
        );
        if sticky {
            let l = log.clone();
            let pin = be.create_pressable(Rc::new(move || l.borrow_mut().push((-1.0, -1.0))), &a11y);
            be.apply_style(
                &pin,
                &Rc::new(StyleRules {
                    position: Some(runtime_shared::Position::Sticky),
                    top: px(0.0),
                    left: px(0.0),
                    margin_top: px(4.0),
                    margin_left: px(6.0),
                    width: px(3.0),
                    height: px(2.0),
                    background: Some(Tokenized::Literal("rgb(255, 0, 0)".into())),
                    ..Default::default()
                }),
            );
            be.insert(&mut content, pin);
        }
        be.insert(&mut sv, content);
        be.insert(&mut root, sv);
        be.finish(root);
        be.render_to_grid();
        (be, sv, log)
    }

    fn offsets(be: &TerminalBackend, sv: TermNode) -> (f32, f32) {
        let d = &be.nodes[&sv.id];
        (d.scroll_x, d.scroll_y)
    }

    fn row_text(grid: &crate::render::Grid, row: u16) -> String {
        (0..grid.cols).map(|c| grid.cell(c, row).map_or(' ', |cell| cell.glyph)).collect()
    }

    fn col_text(grid: &crate::render::Grid, col: u16) -> String {
        (0..grid.rows).map(|r| grid.cell(col, r).map_or(' ', |cell| cell.glyph)).collect()
    }

    /// `ScrollAxis::Both`: each wheel delta drives its own offset, each
    /// clamped to its own `[0, content - viewport]`, and every change
    /// reports both offsets through `on_scroll`.
    #[test]
    fn two_axis_wheel_scrolls_and_clamps_both_offsets() {
        let (mut be, sv, log) = scroller(ScrollAxis::Both, false);
        assert!(be.dispatch_scroll(2, 2, 5.0, 3.0));
        assert_eq!(offsets(&be, sv), (5.0, 3.0));
        be.dispatch_scroll(2, 2, 100.0, 100.0);
        assert_eq!(offsets(&be, sv), (20.0, 14.0), "each axis clamps at content - viewport");
        be.dispatch_scroll(2, 2, -100.0, 0.0);
        assert_eq!(offsets(&be, sv), (0.0, 14.0), "one axis clamps at 0 while the other holds");
        assert_eq!(log.borrow().as_slice(), &[(5.0, 3.0), (20.0, 14.0), (0.0, 14.0)]);
    }

    /// A single-axis scroller never moves its cross-axis offset — even
    /// when its content overflows on that axis (the 30-wide content in a
    /// vertical scroller must not let the wheel slide it sideways).
    #[test]
    fn single_axis_wheel_never_moves_the_cross_offset() {
        let (mut be, sv, _) = scroller(ScrollAxis::Vertical, false);
        be.dispatch_scroll(2, 2, 5.0, 3.0);
        assert_eq!(offsets(&be, sv), (0.0, 3.0));

        let (mut be, sv, _) = scroller(ScrollAxis::Horizontal, false);
        be.dispatch_scroll(2, 2, 5.0, 3.0);
        assert_eq!(offsets(&be, sv), (5.0, 0.0));
    }

    /// A two-axis scroller overflowing on both axes paints a vertical
    /// scrollbar in its trailing column and a horizontal one in its
    /// bottom row; a vertical scroller paints only the vertical one.
    #[test]
    fn two_axis_scroller_paints_a_scrollbar_per_axis() {
        let is_bar = |s: &str, track: char, thumb: char| s.contains(thumb) && s.chars().all(|c| c == track || c == thumb || c == ' ');
        let (mut be, _, _) = scroller(ScrollAxis::Both, false);
        let grid = be.render_to_grid();
        assert!(is_bar(&col_text(&grid, 9), '│', '┃'), "vertical bar: {:?}", col_text(&grid, 9));
        let bottom = row_text(&grid, 5);
        assert!(bottom.contains('━') && bottom.contains('─'), "horizontal bar: {bottom:?}");

        let (mut be, _, _) = scroller(ScrollAxis::Vertical, false);
        let grid = be.render_to_grid();
        assert!(col_text(&grid, 9).contains('┃'), "vertical bar");
        let bottom = row_text(&grid, 5);
        assert!(!bottom.contains('━') && !bottom.contains('─'), "no horizontal bar: {bottom:?}");
    }

    /// `position: Sticky` with `top` + `left` in a two-axis scroller pins
    /// on BOTH axes, where it paints AND where it takes clicks: scrolled
    /// past it on both, it parks at the scrollport's top-left corner;
    /// scrolled on X only, at the left edge on its laid-out row.
    #[test]
    fn sticky_top_left_pins_paint_and_hit_on_both_axes() {
        let (mut be, _, log) = scroller(ScrollAxis::Both, true);
        let bg = |be: &mut TerminalBackend, col, row| be.render_to_grid().cell(col, row).and_then(|c| c.bg);
        let click = |be: &mut TerminalBackend, col, row| match be.dispatch_click(col, row) {
            ClickOutcome::HandlerFired(h) => {
                h();
                true
            }
            _ => false,
        };
        assert_eq!(bg(&mut be, 6, 4), Some(RED), "laid out at (6, 4)");

        be.dispatch_scroll(2, 2, 10.0, 8.0);
        assert_eq!(bg(&mut be, 0, 0), Some(RED), "pinned to the top-left corner");
        assert_eq!(bg(&mut be, 6, 4), None, "no longer at its laid-out cell");
        assert!(click(&mut be, 1, 1), "the pinned box takes the click where it paints");
        assert_eq!(log.borrow().last(), Some(&(-1.0, -1.0)));

        be.dispatch_scroll(2, 2, 0.0, -8.0);
        assert_eq!(bg(&mut be, 0, 4), Some(RED), "left pin keeps the laid-out row");
        assert_eq!(bg(&mut be, 0, 0), None, "no vertical pin without vertical scroll");
    }
}
