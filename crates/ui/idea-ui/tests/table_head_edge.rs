//! `head_edge`: a table's pinned header shows when rows have scrolled
//! beneath it.
//!
//! Neither table form exposes its rows' scroll offset, so an app had no way
//! to draw an edge under the header only once the rows moved. The header
//! cells of the last header row now select `head_edge: hidden` at the top
//! and `scrolled` once the rows' scroller has left it, for
//! `Table(scroll_y = true)` and for the virtualized table alike. These tests
//! give `scrolled` a shadow through `TableStyles::restyle`, as an app does,
//! and scroll the table the way its backend reports a scroll.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::{
    ColumnId, RowId, Table, TableCell, TableColumn, TableHeadEdge, TableProps, TableRow, TableStyles, VirtualRows,
};
use runtime_core::{signal, text, ui, Color, Shadow, StyleRules, Tokenized};

/// Nodes whose last applied style carries a shadow.
type Shadowed = Rc<RefCell<BTreeSet<host_mock::Node>>>;

/// A harness recording which nodes are shadowed, with `head_edge:
/// scrolled` restyled to a shadow.
fn harness() -> (host_mock::Harness, Shadowed) {
    let h = host_mock::Harness::new();
    let shadowed: Shadowed = Rc::default();
    let s = shadowed.clone();
    h.set_style_line(move |node, rules| {
        if rules.shadow.is_some() {
            s.borrow_mut().insert(node);
        } else {
            s.borrow_mut().remove(&node);
        }
        format!("apply_style n{node}")
    });
    (h, shadowed)
}

fn restyle_head_edge() {
    runtime_core::component_styles::__reset::<TableStyles>();
    install_idea_theme(light_theme());
    TableStyles::restyle(|s| {
        s.head_cell().arm(TableHeadEdge::Scrolled, |_| StyleRules {
            shadow: Some(Shadow {
                x: 0.0,
                y: 2.0,
                blur: 4.0,
                color: Tokenized::Literal(Color("rgba(0,0,0,0.12)".into())),
            }),
            ..Default::default()
        });
    });
}

/// The repro: a `scroll_y` table, 300 px tall, 40 rows. Scrolling the
/// rows shadows exactly the header cells; back at the top the shadow goes.
#[test]
fn a_scroll_y_tables_header_shows_rows_scrolled_beneath_it() {
    let (h, shadowed) = harness();
    let tree = h.world.enter(|| {
        restyle_head_edge();
        let rows: Vec<String> = (0..40).map(|i| format!("Row {i}")).collect();
        ui! {
            Table(scroll_y = true, max_height = Some(300.0)) {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                    TableCell(header = true, text = Some("Role".to_string()))
                }
                for row in rows {
                    TableRow {
                        TableCell(text = Some(row.clone()))
                        TableCell(text = Some("crew".to_string()))
                    }
                }
            }
        }
    });
    let _realized = h.mount(tree);
    h.flush();
    assert!(shadowed.borrow().is_empty(), "at the top: no edge");

    let scroll = |y: f32| {
        for i in 0..h.scroll_view_count() {
            if let Some(f) = h.scroll_handler(i) {
                h.world.enter(|| f(0.0, y));
            }
        }
        h.flush();
    };
    scroll(120.0);
    assert_eq!(shadowed.borrow().len(), 2, "both header cells, and nothing else: {:?}", shadowed.borrow());
    h.take_log();
    scroll(400.0);
    assert!(
        !h.take_log().iter().any(|l| l.starts_with("apply_style")),
        "scrolling further restyles nothing — the arm only flips at the top"
    );
    scroll(0.0);
    assert!(shadowed.borrow().is_empty(), "back at the top: no edge");
    runtime_core::component_styles::__reset::<TableStyles>();
}

/// A table that doesn't scroll vertically never selects `scrolled`: its
/// header isn't pinned over anything.
#[test]
fn a_table_without_scroll_y_never_shows_the_edge() {
    let (h, shadowed) = harness();
    let tree = h.world.enter(|| {
        restyle_head_edge();
        ui! {
            Table(scroll_x = true) {
                TableRow { TableCell(header = true, text = Some("Name".to_string())) }
                TableRow { TableCell(text = Some("Ana".to_string())) }
            }
        }
    });
    let _realized = h.mount(tree);
    h.flush();
    for i in 0..h.scroll_view_count() {
        if let Some(f) = h.scroll_handler(i) {
            h.world.enter(|| f(80.0, 200.0));
        }
    }
    h.flush();
    assert!(shadowed.borrow().is_empty());
    runtime_core::component_styles::__reset::<TableStyles>();
}

/// The virtualized table: the grid's scroll report drives the same arm on
/// its frozen header row, selection column included.
#[test]
fn a_virtualized_tables_header_shows_rows_scrolled_beneath_it() {
    let (h, shadowed) = harness();
    let tree = h.world.enter(|| {
        restyle_head_edge();
        let rows = signal((0..1000).map(|i| format!("row {i}")).collect::<Vec<String>>());
        let props = TableProps {
            columns: vec![TableColumn::new("name").label("Name"), TableColumn::new("role").label("Role")],
            selection: Some(signal(BTreeSet::new())),
            virtualized: Some(
                VirtualRows::new(
                    move || rows.with(|r| r.len()),
                    move |i| RowId::from(rows.with(|r| r[i].clone())),
                    |i, _: &ColumnId| {
                        let s = format!("{i}");
                        ui! { text { s } }
                    },
                )
                .row_height(40.0),
            ),
            ..Default::default()
        };
        Table(props)
    });
    let _realized = h.mount(tree);
    h.flush();
    let grid = h.virtual_grid(0);
    let mount = |c, r| {
        let (node, _) = h.world.enter(|| (grid.mount_cell)(c, r));
        h.flush();
        node
    };
    let heads: BTreeSet<host_mock::Node> = (0..3).map(|c| mount(c, 0)).collect();
    let body = mount(1, 5);
    assert!(shadowed.borrow().is_empty(), "at the top: no edge");

    let scroll = grid.on_scroll.clone().expect("the table observes the grid's scroll");
    h.world.enter(|| scroll(0.0, 200.0));
    h.flush();
    assert_eq!(*shadowed.borrow(), heads, "every header cell, no body cell");
    assert!(!shadowed.borrow().contains(&body));
    h.world.enter(|| scroll(0.0, 0.0));
    h.flush();
    assert!(shadowed.borrow().is_empty(), "back at the top: no edge");
    runtime_core::component_styles::__reset::<TableStyles>();
}
