//! `LayoutTree::measure_main`: a detached root's natural main-axis size at
//! a given cross size — what a native virtualizer reports for an
//! `ItemSize::Measured` row (the row is a detached root; `compute` would
//! force it to whatever box it is handed, so its content height could
//! never be read).
//!
//! REGRESSION (CrewForge `TableCards` on iOS): cards estimated at 132pt
//! were 200pt tall; with no measurement the list spaced them by the
//! estimate and each card overlapped the next by 58pt.

use std::rc::Rc;

use runtime_layout::{AvailableSpace, LayoutTree, MeasureFn, Size};
use runtime_shared::{Length, StyleRules};

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

/// A text leaf `single_line` wide on one line, 20pt per line, wrapping
/// to the width it is given.
fn text(single_line: f32) -> MeasureFn {
    Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
        let w = known.width.unwrap_or(match avail.width {
            AvailableSpace::Definite(aw) => single_line.min(aw),
            _ => single_line,
        });
        let lines = (single_line / w.max(1.0)).ceil().max(1.0);
        Size { width: w, height: lines * 20.0 }
    })
}

/// A card: padding 10, a 50pt header, then a wrapping body.
fn card(t: &mut LayoutTree, body_single_line: f32) -> runtime_layout::LayoutNode {
    let root = t.new_node();
    t.set_style(root, &StyleRules { padding_top: px(10.0), padding_bottom: px(10.0), ..Default::default() });
    let header = t.new_node();
    t.set_style(header, &StyleRules { height: px(50.0), ..Default::default() });
    let body = t.new_node();
    t.set_style(body, &StyleRules::default());
    t.set_measure_fn(body, text(body_single_line));
    t.add_child(root, header);
    t.add_child(root, body);
    root
}

#[test]
fn a_rows_height_follows_its_content_at_the_lane_width() {
    let mut t = LayoutTree::new();
    let root = card(&mut t, 600.0);
    // 600 of text in a 300 lane wraps to two lines: 10 + 50 + 40 + 10.
    assert_eq!(t.measure_main(root, false, 300.0), 110.0);
    // Narrower lane, more lines: 600 / 200 = 3 lines: 10 + 50 + 60 + 10.
    assert_eq!(t.measure_main(root, false, 200.0), 130.0);
}

/// The box `compute` is handed afterwards wins again: measuring does not
/// leave the row at its natural size, and a re-measure after a `compute`
/// (whose box overwrote the auto height) still reads the content.
#[test]
fn measuring_and_computing_alternate_cleanly() {
    let mut t = LayoutTree::new();
    let root = card(&mut t, 600.0);
    let h = t.measure_main(root, false, 300.0);
    t.compute(root, 300.0, h);
    let f = t.frame_of(root);
    assert_eq!((f.width, f.height), (300.0, 110.0));
    // A stale, too-small cell box: compute forces it...
    t.compute(root, 300.0, 40.0);
    assert_eq!(t.frame_of(root).height, 40.0);
    // ...and the next measure still finds the content's 110.
    assert_eq!(t.measure_main(root, false, 300.0), 110.0);
}

/// An author's own main size is kept; a horizontal list measures width.
#[test]
fn an_explicit_size_wins_and_horizontal_lists_measure_width() {
    let mut t = LayoutTree::new();
    let fixed = t.new_node();
    t.set_style(fixed, &StyleRules { height: px(75.0), ..Default::default() });
    assert_eq!(t.measure_main(fixed, false, 300.0), 75.0);

    let chip = t.new_node();
    t.set_style(chip, &StyleRules { padding_left: px(8.0), padding_right: px(8.0), ..Default::default() });
    let label = t.new_node();
    t.set_measure_fn(label, text(64.0));
    t.add_child(chip, label);
    assert_eq!(t.measure_main(chip, true, 40.0), 80.0);
}
