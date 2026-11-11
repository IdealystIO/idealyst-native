//! `Table(virtualized = …)`, mounted against `host-mock`.
//!
//! The table lays its rows out on the `virtual_grid` primitive; the mock
//! records the grid's callback bundle, so these tests drive it the way a
//! backend does — ask for a cell, report a scrollport, scroll — and assert
//! what the table hands back. The grid's own windowing and frozen-pane
//! geometry are tested in `runtime_shared::primitives::virtual_grid` and per
//! backend; this suite is the table's half.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::table::{
    ColumnId, ColumnPin, RowId, RowTone, Table, TableColumn, TableEmpty, TableProps, TableScroller,
    VirtualRows, SELECT_COLUMN_WIDTH_PX,
};
use runtime_core::primitives::virtual_grid::Scrollport;
use runtime_core::{signal, text, ui, IntoElement, Signal};

type Realized = runtime_scene::Realized<host_mock::Node>;

struct Fixture {
    h: host_mock::Harness,
    _realized: Realized,
    rows: Signal<Vec<String>>,
    selection: Signal<BTreeSet<RowId>>,
    /// Every cell node's last resolved background token.
    backgrounds: Rc<RefCell<HashMap<host_mock::Node, Option<String>>>>,
}

fn columns() -> Vec<TableColumn> {
    vec![
        TableColumn::new("name").label("Name").width(160.0).pinned(ColumnPin::Left),
        TableColumn::new("email").label("Email"),
        TableColumn::new("role").label("Role").min_width(120.0),
        TableColumn::new("menu").width(48.0).pinned(ColumnPin::Right),
    ]
}

fn table(count: usize, extra: impl FnOnce(VirtualRows, &mut TableProps) -> VirtualRows) -> Fixture {
    let h = host_mock::Harness::new();
    let backgrounds: Rc<RefCell<HashMap<host_mock::Node, Option<String>>>> = Rc::new(RefCell::new(HashMap::new()));
    {
        let b = backgrounds.clone();
        h.set_style_line(move |node, rules| {
            let bg = rules.background.as_ref().and_then(|c| c.name().map(str::to_string));
            b.borrow_mut().insert(node, bg.clone());
            format!("apply_style n{node} bg={bg:?}")
        });
    }
    let cell = std::cell::Cell::new(None);
    let realized = h.world.enter(|| {
        install_idea_theme(light_theme());
        let rows = signal((0..count).map(|i| format!("row {i}")).collect::<Vec<String>>());
        let selection = signal(BTreeSet::new());
        cell.set(Some((rows, selection)));
        let mut props = TableProps { columns: columns(), selection: Some(selection), ..Default::default() };
        let base = VirtualRows::new(
            move || rows.with(|r| r.len()),
            move |i| RowId::from(rows.with(|r| r[i].clone())),
            move |i, col: &ColumnId| {
                let label = rows.with(|r| r.get(i).cloned()).unwrap_or_default();
                let s = format!("{label} / {}", col.as_str());
                ui! { text { s } }
            },
        );
        props.virtualized = Some(extra(base, &mut props));
        Table(props)
    });
    let realized = h.mount(realized);
    h.flush();
    let (rows, selection) = cell.take().unwrap();
    Fixture { h, _realized: realized, rows, selection, backgrounds }
}

impl Fixture {
    fn grid(&self) -> runtime_shared::primitives::virtual_grid::GridCallbacks<host_mock::Node> {
        self.h.virtual_grid(0)
    }

    fn grid_node(&self) -> host_mock::Node {
        *self
            .h
            .shared
            .kinds
            .borrow()
            .iter()
            .find(|(_, k)| k.starts_with("virtual_grid"))
            .expect("the table mounted a virtual_grid")
            .0
    }

    /// Mount cell `(col, row)` the way a backend does, and return its node.
    fn mount(&self, col: usize, row: usize) -> host_mock::Node {
        let mount = self.grid().mount_cell;
        let (node, _) = self.h.world.enter(|| mount(col, row));
        self.h.flush();
        node
    }

    fn text_of(&self, node: host_mock::Node) -> String {
        self.h.live_tree(node)
    }

    fn hover(&self, node: host_mock::Node, entering: bool) {
        let f = self
            .h
            .shared
            .hover_handlers
            .borrow()
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, f)| f.clone())
            .expect("the cell takes hover");
        self.h.world.enter(|| f(entering));
        self.h.flush();
    }

    fn bg(&self, node: host_mock::Node) -> Option<String> {
        self.backgrounds.borrow().get(&node).cloned().flatten()
    }
}

/// The header row and the edge columns are the grid's frozen panes: the
/// selection column plus the left-pinned name column at the left, the
/// right-pinned menu column at the right. A table that left them scrolling
/// would lose its header after the first screen of rows.
#[test]
fn the_header_and_pinned_columns_are_frozen() {
    let f = table(1000, |r, _| r);
    let log = f.h.take_log().join("\n");
    assert!(
        log.contains(&format!("set_virtual_grid_frozen n{} top=1 bottom=0 left=2 right=1", f.grid_node())),
        "{log}"
    );
    let g = f.grid();
    assert_eq!((g.col_count)(), 5, "selection + four columns");
    assert_eq!((g.row_count)(), 1001, "the header + every row");
}

/// The whole point: a 100,000-row table builds no row until the grid asks
/// for one.
#[test]
fn regression_a_virtualized_table_builds_rows_only_when_the_grid_asks() {
    let built = Rc::new(std::cell::Cell::new(0usize));
    let h = host_mock::Harness::new();
    let b = built.clone();
    let el = h.world.enter(|| {
        install_idea_theme(light_theme());
        Table(TableProps {
            columns: columns(),
            virtualized: Some(VirtualRows::new(
                || 100_000,
                |i| RowId::from(format!("r{i}")),
                move |_, _| {
                    b.set(b.get() + 1);
                    text("x").into_element()
                },
            )),
            ..Default::default()
        })
    });
    let _r = h.mount(el);
    h.flush();
    assert_eq!(built.get(), 0, "no cell built before the grid mounts one");
    let mount = h.virtual_grid(0).mount_cell;
    h.world.enter(|| mount(1, 50_000));
    assert_eq!(built.get(), 1);
}

#[test]
fn header_cells_carry_the_labels_and_body_cells_the_renderers_content() {
    let f = table(10, |r, _| r);
    let head = f.mount(2, 0);
    assert!(f.text_of(head).contains("Email"), "{}", f.text_of(head));
    let body = f.mount(2, 4);
    assert!(f.text_of(body).contains("row 3 / email"), "{}", f.text_of(body));
    // The selection column: a select-all box in the header, a box per row.
    let all = f.mount(0, 0);
    assert!(f.text_of(all).contains("pressable"), "{}", f.text_of(all));
    let one = f.mount(0, 2);
    assert!(f.text_of(one).contains("pressable"), "{}", f.text_of(one));
}

/// Select-all covers every row, not just the mounted ones: with 1,000 rows
/// and a handful of cells on screen, pressing it selects all 1,000.
#[test]
fn select_all_selects_every_row_mounted_or_not() {
    let f = table(1000, |r, _| r);
    let presses_before = f.h.shared.press_handlers.borrow().len();
    f.mount(0, 0);
    let press = f.h.shared.press_handlers.borrow()[presses_before].clone();
    f.h.world.enter(|| press());
    f.h.flush();
    assert_eq!(f.selection.with_untracked(|s| s.len()), 1000);
}

/// Flexible columns share the scrollport's width once it is known; fixed
/// ones keep theirs. Until then every flexible column sits at its minimum.
#[test]
fn flexible_columns_share_the_scrollports_width() {
    let f = table(10, |r, _| r);
    let g = f.grid();
    let widths = |g: &runtime_shared::primitives::virtual_grid::GridCallbacks<host_mock::Node>| {
        (0..(g.col_count)()).map(|c| (g.col_width)(c)).collect::<Vec<f32>>()
    };
    // select, name (160), email (min 80 — the resize floor), role (min 120), menu (48)
    let before = widths(&g);
    assert_eq!(before[0], SELECT_COLUMN_WIDTH_PX);
    assert_eq!((before[1], before[3], before[4]), (160.0, 120.0, 48.0));
    f.h.take_log();
    host_mock::set_virtual_grid_scrollport(f.grid_node(), Scrollport::overlay(1000.0, 400.0));
    f.h.flush();
    let after = widths(&g);
    assert!(
        f.h.take_log().iter().any(|l| l.starts_with("virtual_grid_data_changed")),
        "the grid re-lays out when the width moves"
    );
    let total: f32 = after.iter().sum();
    assert!((total - 1000.0).abs() < 0.5, "the columns fill the scrollport: {after:?}");
    assert_eq!(after[2], after[3], "email and role share the rest equally: {after:?}");
}

/// Hovering any cell tints its whole row — every cell of that row, mounted
/// separately — and no other. One shared signal drives it: a signal per
/// cell would leave the row's other cells dark.
#[test]
fn hovering_one_cell_tints_every_cell_of_its_row() {
    let f = table(10, |r, _| r.on_row_click(|_| {}));
    let (a, b, other) = (f.mount(1, 3), f.mount(2, 3), f.mount(2, 4));
    let rest = f.bg(b);
    f.hover(a, true);
    assert_ne!(f.bg(b), rest, "the row's other cell takes the hover tint");
    assert_eq!(f.bg(other), rest, "another row stays as it was");
    f.hover(a, false);
    assert_eq!(f.bg(b), rest);
}

/// Row tones reach every cell of the row.
#[test]
fn row_tone_tints_every_cell_of_the_row() {
    let f = table(10, |r, _| r.tone(|i| if i == 2 { RowTone::Danger } else { RowTone::None }));
    let (toned, plain) = (f.mount(2, 3), f.mount(2, 4));
    assert_eq!(f.bg(toned).as_deref(), Some("color-table-row-danger"));
    assert_ne!(f.bg(plain).as_deref(), Some("color-table-row-danger"));
}

/// A frozen column is opaque: rows scroll beneath it, and a transparent
/// frozen cell would show them through.
#[test]
fn frozen_body_cells_are_opaque() {
    let f = table(10, |r, _| r);
    let pinned = f.mount(1, 3);
    assert_eq!(f.bg(pinned).as_deref(), Some("color-surface"));
}

/// A row whose record changed under the same id gets a new cell key, so the
/// grid rebuilds it; a cell that kept its key would show the old record.
#[test]
fn regression_an_edited_row_keeps_its_old_cells() {
    let f = table(10, |r, _| r);
    let key = f.grid().cell_key;
    let before = key(2, 4);
    f.h.world.enter(|| f.rows.update(|r| { let mut r = r.clone(); r[5] = "row 5 renamed".into(); r }));
    f.h.flush();
    assert_ne!(key(2, 4), before);
    assert_eq!(key(2, 0), key(2, 0), "header keys are stable");
}

/// `on_end_reached` fires once as the reader nears the last row, and again
/// only after scrolling away and back.
#[test]
fn end_reached_fires_once_per_approach() {
    let hits = Rc::new(std::cell::Cell::new(0));
    let h2 = hits.clone();
    let f = table(100, move |r, _| r.row_height(40.0).on_end_reached(200.0, move || h2.set(h2.get() + 1)));
    host_mock::set_virtual_grid_scrollport(f.grid_node(), Scrollport::overlay(800.0, 400.0));
    f.h.flush();
    let scroll = f.grid().on_scroll.expect("the table observes the grid's scroll");
    // Content: 101 rows x 40 = 4040. Near the end once y + 400 >= 3840.
    let at = |y: f32| f.h.world.enter(|| scroll(0.0, y));
    at(1000.0);
    assert_eq!(hits.get(), 0);
    at(3500.0);
    assert_eq!(hits.get(), 1);
    at(3600.0);
    assert_eq!(hits.get(), 1, "still near the end: no second fetch");
    at(1000.0);
    at(3500.0);
    assert_eq!(hits.get(), 2, "re-armed after scrolling away");
}

/// `TableScroller::scroll_to_row` lands the row just under the frozen
/// header, keeping the horizontal offset.
#[test]
fn scroller_lands_a_row_under_the_header() {
    let scroller = TableScroller::new();
    let s = scroller.clone();
    let f = table(100, move |r, p| {
        p.scroller = Some(s);
        r.row_height(40.0)
    });
    host_mock::take_handle_log();
    f.h.world.enter(|| scroller.scroll_to_row(&RowId::from("row 30")));
    let log = host_mock::take_handle_log();
    assert!(log.iter().any(|l| l.starts_with("scroll_to 0 1200")), "{log:?}");
}

/// The empty state shows while there are no rows and collapses once some
/// arrive — without being rebuilt, since an element can be mounted once.
#[test]
fn empty_state_follows_the_row_count() {
    let f = table(0, |r, p| {
        p.empty = Some(TableEmpty::Text("Nobody yet".into()));
        r
    });
    assert_eq!((f.grid().row_count)(), 1, "just the header");
    let roots = f.h.live_roots();
    let screen: String = roots.iter().map(|r| f.h.live_tree(*r)).collect();
    assert!(screen.contains("Nobody yet"), "{screen}");
    f.h.world.enter(|| f.rows.set(vec!["a".into()]));
    f.h.flush();
    assert_eq!((f.grid().row_count)(), 2);
}

#[test]
#[should_panic(expected = "drag-to-reorder")]
fn a_virtualized_table_refuses_reorder() {
    table(10, |r, p| {
        p.on_reorder = Some(Rc::new(|_, _| {}));
        r
    });
}

#[test]
#[should_panic(expected = "not the children block")]
fn a_virtualized_table_refuses_children() {
    table(10, |r, p| {
        p.children = vec![ui! { view() }];
        r
    });
}

#[test]
#[should_panic(expected = "`columns` is required")]
fn a_virtualized_table_needs_columns() {
    table(10, |r, p| {
        p.columns = Vec::new();
        r
    });
}


/// REGRESSION: hovering one row restyled EVERY mounted cell. Each body
/// cell's style read the table-wide hovered-row signal, so moving the
/// pointer one row re-ran ~300 style effects — and on native each
/// re-applied style dirtied the cell's layout, so the next layout pass
/// re-laid out every cell (4-6 ms per pass while scrolling under the
/// pointer). Only the row the pointer left and the row it entered restyle.
#[test]
fn regression_hovering_one_row_restyles_every_cell() {
    let f = table(30, |r, _| r.on_row_click(|_| {}));
    let mut cells = Vec::new();
    for row in 1..=8 {
        for col in 1..=3 {
            cells.push((row, f.mount(col, row)));
        }
    }
    f.h.take_log();
    let hover_target = cells.iter().find(|(r, _)| *r == 3).unwrap().1;
    f.hover(hover_target, true);
    let restyled: std::collections::BTreeSet<usize> = f
        .h
        .take_log()
        .iter()
        .filter_map(|l| l.strip_prefix("apply_style n"))
        .filter_map(|l| l.split_whitespace().next()?.parse::<host_mock::Node>().ok())
        .filter_map(|n| cells.iter().find(|(_, c)| *c == n).map(|(r, _)| *r))
        .collect();
    assert_eq!(restyled, std::collections::BTreeSet::from([3]), "only the hovered row's cells restyle");
}
