//! The content of a horizontal `scroll_view` is as wide as its children
//! along the scroll axis — never stretched (or clamped) to the viewport.
//!
//! REGRESSION (robot-e2e demo tab strip on macOS): a scroll view's
//! children stack in a flex column, so its row of tabs was a CROSS-axis
//! child and the default `align-items: stretch` made it exactly the
//! scroller's width. The tabs past the first screenful were laid out
//! outside the row's frame. AppKit and UIKit hit-test a view only inside
//! its superview's bounds, so a click on any of those tabs went to the
//! scroller instead.

use runtime_layout::{LayoutNode, LayoutTree};
use runtime_shared::{FlexDirection, Length, StyleRules, Tokenized};

const VIEWPORT_W: f32 = 300.0;
const TAB_W: f32 = 80.0;
const TAB_H: f32 = 30.0;
const GAP: f32 = 12.0;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Length::Px(v).into())
}

struct Strip {
    tree: LayoutTree,
    scroller: LayoutNode,
    row: LayoutNode,
    tabs: Vec<LayoutNode>,
}

/// `root(viewport) > scroll_view(horizontal) > view(row) > tabs`, built
/// the way the native backends build it: the scroll node is marked with
/// `set_overflow_scroll` at create time, then children are attached.
/// `row_style` is the author's style on the row (flex_direction is added).
fn strip(tab_count: usize, row_style: StyleRules) -> Strip {
    let mut tree = LayoutTree::new();
    let root = tree.new_node();
    let scroller = tree.new_node();
    tree.set_overflow_scroll(scroller, true);
    tree.add_child(root, scroller);
    let row = tree.new_node();
    tree.set_style(
        row,
        &StyleRules {
            flex_direction: Some(FlexDirection::Row),
            gap: px(GAP),
            ..row_style
        },
    );
    tree.add_child(scroller, row);
    let tabs = (0..tab_count)
        .map(|_| {
            let tab = tree.new_node();
            tree.set_intrinsic_size(tab, TAB_W, TAB_H);
            tree.add_child(row, tab);
            tab
        })
        .collect();
    tree.compute(root, VIEWPORT_W, 400.0);
    Strip { tree, scroller, row, tabs }
}

fn row_width_for(n: usize) -> f32 {
    n as f32 * TAB_W + (n as f32 - 1.0) * GAP
}

#[test]
fn regression_hscroll_row_is_as_wide_as_its_tabs_not_the_viewport() {
    let n = 10;
    let s = strip(n, StyleRules::default());
    let row = s.tree.frame_of(s.row);
    assert_eq!(s.tree.frame_of(s.scroller).width, VIEWPORT_W, "the viewport fills its parent");
    assert_eq!(row.width, row_width_for(n), "the row is as wide as its tabs");
    // Every tab lies inside the row's frame — where AppKit/UIKit will
    // deliver a click to it.
    let last = s.tree.frame_of(*s.tabs.last().unwrap());
    assert!(
        last.x + last.width <= row.width,
        "last tab ends at {} past the row's {}",
        last.x + last.width,
        row.width
    );
    // The scrollable extent is the row.
    assert_eq!(s.tree.scroll_content_extent(s.scroller).0, row_width_for(n));
}

/// Content narrower than the viewport keeps its own width too — the same
/// rule on both sides of the edge — and an author who wants it to fill the
/// viewport says so with `min_width: 100%`, as on the web.
#[test]
fn hscroll_content_fills_the_viewport_only_when_the_author_asks() {
    let s = strip(2, StyleRules::default());
    assert_eq!(s.tree.frame_of(s.row).width, row_width_for(2));

    let fill = StyleRules { min_width: Some(Length::Percent(100.0).into()), ..Default::default() };
    let s = strip(2, fill.clone());
    assert_eq!(s.tree.frame_of(s.row).width, VIEWPORT_W, "short content floors at the viewport");
    let s = strip(10, fill);
    assert_eq!(s.tree.frame_of(s.row).width, row_width_for(10), "long content still overflows it");
}

/// An author `width` wins over the content width.
#[test]
fn hscroll_content_keeps_an_author_width() {
    let s = strip(10, StyleRules { width: px(500.0), ..Default::default() });
    assert_eq!(s.tree.frame_of(s.row).width, 500.0);
}

/// Moved out of the horizontal scroller, the node is an ordinary child
/// again: stretched to its new parent's width.
#[test]
fn content_moved_out_of_an_hscroll_stretches_again() {
    let mut s = strip(10, StyleRules::default());
    let root = s.tree.new_node();
    let column = s.tree.new_node();
    s.tree.add_child(root, column);
    s.tree.add_child(column, s.row);
    s.tree.compute(root, VIEWPORT_W, 400.0);
    assert_eq!(s.tree.frame_of(s.row).width, VIEWPORT_W);
}

/// A vertical scroller's content still fills the viewport's width.
#[test]
fn vscroll_content_still_stretches_across() {
    let mut tree = LayoutTree::new();
    let root = tree.new_node();
    let scroller = tree.new_node();
    tree.set_overflow_scroll(scroller, false);
    tree.add_child(root, scroller);
    let body = tree.new_node();
    tree.add_child(scroller, body);
    let item = tree.new_node();
    tree.set_intrinsic_size(item, TAB_W, TAB_H);
    tree.add_child(body, item);
    tree.compute(root, VIEWPORT_W, 400.0);
    assert_eq!(tree.frame_of(body).width, VIEWPORT_W);
}
