//! Two table wants pinned as geometry and resolved styles on the real mount.
//!
//! - want_aacf8013: an app must be able to take the header labels out of
//!   uppercase. `TableStyles::restyle` on `head_text` reaches every header shape:
//!   plain, column-model and sortable.
//! - want_6ba2f779: a `scroll_y` Plain table in a bounded parent must scroll its
//!   rows rather than clip them: `edge_watched_scroller`'s measuring holder must
//!   shrink. Laid out in Taffy from the native lowering's styles; the holder's
//!   rules are the same on web, which isn't measured here.

mod common;

use std::rc::Rc;

use common::{find, lay_out};
use idea_ui::{SortState, Table, TableCell, TableColumn, TableRow, TableStyles, TableVariant};
use runtime_core::{ui, Element, FontWeight, Length, StyleRules, TextTransform, Tokenized};

const PLAIN_NAME: &str = "text \"Name";

fn sentence_case_heads() {
    TableStyles::restyle(|s| {
        s.head_text().base(|_| StyleRules {
            text_transform: Some(TextTransform::None),
            letter_spacing: Some(Tokenized::Literal(0.0)),
            font_weight: Some(FontWeight::Medium),
            ..Default::default()
        });
    });
}

fn assert_sentence_case(case: &str, build: impl FnOnce() -> Element) {
    let laid = lay_out(800.0, 600.0, || {
        sentence_case_heads();
        build()
    });
    let node = find(&laid.h, laid.root, PLAIN_NAME)
        .unwrap_or_else(|| panic!("{case}: no header text:\n{}", laid.h.tree(laid.root)));
    let rules = laid.styles.get(&node).unwrap_or_else(|| panic!("{case}: the header text is unstyled"));
    assert_eq!(rules.text_transform, Some(TextTransform::None), "{case}: not uppercase");
    assert_eq!(rules.font_weight, Some(FontWeight::Medium), "{case}: the restyle's weight");
}

#[test]
fn restyled_head_text_reaches_a_plain_header() {
    assert_sentence_case("plain", || {
        ui! {
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                }
            }
        }
    });
}

#[test]
fn restyled_head_text_reaches_a_column_model_header() {
    assert_sentence_case("column model", || {
        let columns = vec![TableColumn::new("name").label("Name").width(200.0).resizable()];
        ui! {
            Table(columns = columns) {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                }
            }
        }
    });
}

#[test]
fn restyled_head_text_reaches_a_sortable_header() {
    assert_sentence_case("sortable", || {
        let on_sort: Rc<dyn Fn(SortState)> = Rc::new(|_| {});
        ui! {
            Table {
                TableRow {
                    TableCell(
                        header = true,
                        text = Some("Name".to_string()),
                        sort = Some(SortState::Asc),
                        on_sort = Some(on_sort),
                    )
                }
            }
        }
    });
}

fn bounded_table(scroll_x: bool, head: bool) -> Element {
    let bound = StyleRules { height: Some(Tokenized::Literal(Length::Px(300.0))), ..Default::default() };
    let rows: Vec<String> = (0..60).map(|i| format!("Row {i}")).collect();
    ui! {
        view(style = bound) {
            Table(scroll_x = scroll_x, scroll_y = true, variant = TableVariant::Plain) {
                if head {
                    TableRow {
                        TableCell(header = true, text = Some("Name".to_string()))
                    }
                }
                for row in rows {
                    TableRow {
                        TableCell(text = Some(row.clone()))
                    }
                }
            }
        }
    }
}

/// The laid-out scroller's height with its node built as a native backend builds
/// it: an overflow-scroll leaf that measures its content. `tests/common` mirrors
/// a scroll view as a plain node, so its rows' height would hold it open. Styles
/// are the ones the table really applied.
fn scroller_height(laid: &common::Laid, content_height: f32) -> f32 {
    const HEAD_HEIGHT: f32 = 44.0;
    use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, MeasureFn, Size};
    fn mirror(
        laid: &common::Laid,
        layout: &mut LayoutTree,
        node: u32,
        content_height: f32,
        scroller: &mut Option<LayoutNode>,
    ) -> LayoutNode {
        let ln = layout.new_node();
        let is_scroller = laid.h.kind_of(node).as_deref() == Some("scroll_view");
        // Before the style, as a backend does at creation: marking seeds a fill
        // that the node's own style then overrides.
        if is_scroller {
            layout.set_overflow_scroll_axis(ln, laid.h.shared.scroll_axes.borrow()[&node]);
        }
        layout.set_style(ln, &laid.styles.get(&node).cloned().unwrap_or_default());
        if is_scroller {
            // A split head's strip is a scroller too, and the one that never
            // shrinks: it holds one row. The rows' scroller is the other.
            let is_strip = laid.styles.get(&node).is_some_and(|r| r.flex_shrink == Some(Tokenized::Literal(0.0)));
            let height = if is_strip { HEAD_HEIGHT } else { content_height };
            let measure: MeasureFn = Rc::new(move |known: Size<Option<f32>>, _: Size<AvailableSpace>| Size {
                width: known.width.unwrap_or(800.0),
                height: known.height.unwrap_or(height),
            });
            layout.set_measure_fn(ln, measure);
            if !is_strip {
                *scroller = Some(ln);
            }
            return ln;
        }
        for child in laid.h.children_of(node) {
            let c = mirror(laid, layout, child, content_height, scroller);
            layout.add_child(ln, c);
        }
        ln
    }
    let mut layout = LayoutTree::new();
    let mut scroller = None;
    let root = mirror(laid, &mut layout, laid.root, content_height, &mut scroller);
    layout.compute(root, 800.0, 600.0);
    layout.frame_of(scroller.expect("the table has a scroller")).height
}

/// The scroller stays inside the bound while its rows run far past it, so what
/// doesn't fit scrolls. With `scroll_x` the edge-watch holder sits in between,
/// which is the shape that clipped. A head row splits the table into a pinned
/// strip over the rows' scroller; without one a single scroller holds all rows.
#[test]
fn regression_scroll_y_plain_table_scrolls_in_a_bounded_parent() {
    const ROWS_HEIGHT: f32 = 2640.0;
    for (scroll_x, head) in [(false, true), (true, true), (false, false), (true, false)] {
        let laid = lay_out(800.0, 600.0, || bounded_table(scroll_x, head));
        let height = scroller_height(&laid, ROWS_HEIGHT);
        assert!(
            height > 0.0 && height <= 300.0,
            "scroll_x={scroll_x} head={head}: the scroller keeps to the 300 bound, got {height}\n{}",
            laid.h.tree(laid.root)
        );
    }
}
