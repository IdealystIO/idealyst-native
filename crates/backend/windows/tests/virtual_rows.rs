//! A virtualized list's row geometry on Windows: `ItemSize::Measured`
//! rows are laid out — and the rows after them offset — by their rendered
//! size, not the author's estimate.
//!
//! REGRESSION (idea-ui `TableCards`): cards estimated at 132pt render
//! 200pt tall; with the estimate used as the row size every card overlaps
//! the next by 58pt.
//!
//! The crate body is `cfg(target_os = "windows")`; this compiles the pure
//! module directly (`#[path]`) so it runs on every host. The live list
//! (resync → layout pass → placement) is covered by `virtualizer::tests`
//! (Windows target / Wine).

#[path = "../src/virtual_rows.rs"]
mod virtual_rows;

use std::cell::RefCell;
use std::rc::Rc;

use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};
use runtime_shared::{Length, StyleRules};
use virtual_rows::*;

const ESTIMATE: f32 = 132.0;

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

/// A card row: padding 10, a `header`-tall block, then a body of text
/// `text_len` wide on one line, 20 per line, wrapping to the row's width.
/// Natural height at width `w`: 20 + header + 20 * ceil(text_len / w).
fn card(t: &mut LayoutTree, header: f32, text_len: f32) -> LayoutNode {
    let root = t.new_node();
    t.set_style(root, &StyleRules { padding_top: px(10.0), padding_bottom: px(10.0), ..Default::default() });
    let head = t.new_node();
    t.set_style(head, &StyleRules { height: px(header), ..Default::default() });
    let body = t.new_node();
    t.set_style(body, &StyleRules::default());
    t.set_measure_fn(
        body,
        Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
            let w = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(aw) => text_len.min(aw),
                _ => text_len,
            });
            Size { width: w, height: (text_len / w.max(1.0)).ceil().max(1.0) * 20.0 }
        }),
    );
    t.add_child(root, head);
    t.add_child(root, body);
    root
}

fn rows_of(roots: &[LayoutNode]) -> Vec<RowLayout> {
    roots.iter().enumerate().map(|(idx, &root)| RowLayout { idx, root, scope: 100 + idx as u64 }).collect()
}

#[test]
fn regression_measured_rows_are_spaced_by_their_rendered_size() {
    let mut t = LayoutTree::new();
    // At width 300: 200, 200, 100 (shorter than the estimate), 200.
    let roots = [card(&mut t, 160.0, 300.0), card(&mut t, 140.0, 600.0), card(&mut t, 60.0, 100.0), card(&mut t, 160.0, 250.0)];
    let rows = rows_of(&roots);
    let mut sizes = vec![ESTIMATE; 4];
    let reports: Rc<RefCell<Vec<(u64, f32)>>> = Rc::default();
    let r = reports.clone();
    let report = move |scope, size| r.borrow_mut().push((scope, size));

    assert!(lay_out_rows(&mut t, &rows, &mut sizes, false, 300.0, true, &report));
    assert_eq!(*reports.borrow(), vec![(100, 200.0), (101, 200.0), (102, 100.0), (103, 200.0)]);
    assert_eq!(sizes, vec![200.0, 200.0, 100.0, 200.0]);
    // Each row is laid out at its rendered size, across the list.
    for (root, h) in roots.iter().zip([200.0, 200.0, 100.0, 200.0]) {
        let f = t.frame_of(*root);
        assert_eq!((f.width, f.height), (300.0, h));
    }
    // Following rows are offset by the real sizes (not 132 apart), and
    // the scroll extent is their sum.
    let (offsets, total) = cumulative(&sizes, 8.0);
    assert_eq!(offsets, vec![0.0, 208.0, 416.0, 524.0]);
    assert_eq!(total, 724.0);
    // The window follows too: a 400 viewport holds rows 0..2 (by the
    // estimate it would have reached row 3).
    assert_eq!(visible_range(&offsets, &sizes, 0.0, 400.0, 0.0), (0, 2));

    // Converges: the next pass measures the same sizes and reports nothing.
    reports.borrow_mut().clear();
    assert!(!lay_out_rows(&mut t, &rows, &mut sizes, false, 300.0, true, &report));
    assert!(reports.borrow().is_empty());
    assert_eq!(t.frame_of(roots[2]).height, 100.0);
}

/// The measurement is taken at the list's cross size: a narrower list
/// wraps the body to more lines, and the row grows to match.
#[test]
fn measured_rows_remeasure_when_the_list_narrows() {
    let mut t = LayoutTree::new();
    let roots = [card(&mut t, 50.0, 600.0)];
    let rows = rows_of(&roots);
    let mut sizes = vec![ESTIMATE];
    let reports: Rc<RefCell<Vec<(u64, f32)>>> = Rc::default();
    let r = reports.clone();
    let report = move |scope, size| r.borrow_mut().push((scope, size));

    // 600 / 300 = 2 lines: 10 + 50 + 40 + 10.
    assert!(lay_out_rows(&mut t, &rows, &mut sizes, false, 300.0, true, &report));
    assert_eq!(sizes, vec![110.0]);
    // 600 / 200 = 3 lines.
    assert!(lay_out_rows(&mut t, &rows, &mut sizes, false, 200.0, true, &report));
    assert_eq!(sizes, vec![130.0]);
    assert_eq!(*reports.borrow(), vec![(100, 110.0), (100, 130.0)]);
}

/// A horizontal list measures the row's natural WIDTH at the list's height.
#[test]
fn horizontal_measured_rows_follow_their_rendered_width() {
    let mut t = LayoutTree::new();
    let root = t.new_node();
    t.set_style(root, &StyleRules { padding_left: px(4.0), padding_right: px(4.0), ..Default::default() });
    let child = t.new_node();
    t.set_style(child, &StyleRules { width: px(172.0), ..Default::default() });
    t.add_child(root, child);
    let mut sizes = vec![ESTIMATE];
    let reported = Rc::new(RefCell::new(None));
    let r = reported.clone();
    assert!(lay_out_rows(&mut t, &rows_of(&[root]), &mut sizes, true, 80.0, true, &move |s, v| *r.borrow_mut() = Some((s, v))));
    assert_eq!(*reported.borrow(), Some((100, 180.0)));
    let f = t.frame_of(root);
    assert_eq!((f.width, f.height), (180.0, 80.0));
}

/// A `Known` list's sizes are authoritative: rows are laid out at the
/// given size even where their content differs, and nothing is measured
/// or reported.
#[test]
fn known_lists_never_report_a_measured_size() {
    let mut t = LayoutTree::new();
    let roots = [card(&mut t, 160.0, 300.0), card(&mut t, 60.0, 100.0)];
    let mut sizes = vec![ESTIMATE; 2];
    let calls = Rc::new(std::cell::Cell::new(0));
    let c = calls.clone();
    let changed = lay_out_rows(&mut t, &rows_of(&roots), &mut sizes, false, 300.0, false, &move |_, _| c.set(c.get() + 1));
    assert!(!changed);
    assert_eq!(calls.get(), 0, "set_measured_size must not be called for a Known list");
    assert_eq!(sizes, vec![ESTIMATE; 2]);
    for root in roots {
        assert_eq!(t.frame_of(root).height, ESTIMATE);
    }
}

#[test]
fn cumulative_offsets_include_gaps() {
    let (offs, total) = cumulative(&[10.0, 20.0, 30.0], 5.0);
    assert_eq!(offs, vec![0.0, 15.0, 40.0]);
    assert_eq!(total, 70.0);
}

#[test]
fn visible_range_selects_intersecting_items_with_overscan() {
    let sizes = vec![40.0; 100];
    let (offs, _) = cumulative(&sizes, 0.0);
    assert_eq!(visible_range(&offs, &sizes, 0.0, 200.0, 0.0), (0, 5));
    assert_eq!(visible_range(&offs, &sizes, 400.0, 200.0, 0.0), (10, 15));
    assert_eq!(visible_range(&offs, &sizes, 400.0, 200.0, 1.0), (5, 20));
    assert_eq!(visible_range(&[], &[], 0.0, 200.0, 0.0), (0, 0));
    assert_eq!(visible_range(&offs, &sizes, 0.0, 0.0, 0.0), (0, 0), "no viewport yet");
}
