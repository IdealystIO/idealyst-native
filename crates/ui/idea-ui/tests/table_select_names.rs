//! The plain `Table` and `TableCards` name every selection box and give it
//! a robot test id (the virtualized table's are in `virtual_table.rs`).

use std::collections::BTreeSet;
use std::rc::Rc;

use idea_ui::components::table::{TableCards, TableCardsProps};
use idea_ui::{
    install_idea_theme, light_theme, RowId, Table, TableCell, TableCellProps, TableColumn, TableProps, TableRow,
    TableRowProps,
};
use runtime_core::{signal, ui, Element};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

fn mount(build: impl FnOnce() -> Element) -> (host_mock::Harness, runtime_scene::Realized<u32>) {
    Robot::new().reset();
    let h = host_mock::Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized)
}

fn labels(test_id: &str) -> Vec<String> {
    Robot::new()
        .find_all(Query::test_id(test_id))
        .into_iter()
        .inspect(|e| assert_eq!(e.kind, ElementKind::Pressable, "the box itself carries {test_id}"))
        .map(|e| e.label.unwrap_or_default())
        .collect()
}

fn row(id: Option<&str>, header: bool, text: &str) -> Element {
    let cell = TableCell(TableCellProps {
        header,
        text: runtime_core::Reactive::Static(Some(text.into())),
        ..Default::default()
    });
    TableRow(TableRowProps { id: id.map(RowId::from), selectable: true, children: vec![cell], ..Default::default() })
}

/// CrewForge (Safety Tips): every row's box was an unnamed checkbox with no
/// test id. Named from `select_row_label`, else "Select row"; the header's
/// "Select all rows"; ids `table-select-row` / `table-select-all`.
#[test]
fn regression_table_selection_boxes_are_named_and_carry_test_ids() {
    let tips = [("t1", "Ice on the ramp"), ("t2", "Hard hats on")];
    let (_h, _r) = mount(|| {
        Table(TableProps {
            columns: vec![TableColumn::new("tip")],
            selection: Some(signal(BTreeSet::new())),
            select_row_label: Some(Rc::new(move |id: &RowId| {
                let name = tips.iter().find(|(i, _)| *i == id.as_str()).map_or("?", |(_, n)| n);
                format!("Select {name}")
            })),
            children: vec![row(None, true, "Tip"), row(Some("t1"), false, "Ice on the ramp"), row(Some("t2"), false, "Hard hats on")],
            ..Default::default()
        })
    });
    assert_eq!(labels(idea_ui::SELECT_ROW_TEST_ID), ["Select Ice on the ramp", "Select Hard hats on"]);
    assert_eq!(labels(idea_ui::SELECT_ALL_TEST_ID), [idea_ui::SELECT_ALL_NAME]);

    // No `select_row_label`: still named.
    let (_h, _r) = mount(|| {
        Table(TableProps {
            columns: vec![TableColumn::new("tip")],
            selection: Some(signal(BTreeSet::new())),
            children: vec![row(None, true, "Tip"), row(Some("t1"), false, "Ice on the ramp")],
            ..Default::default()
        })
    });
    assert_eq!(labels(idea_ui::SELECT_ROW_TEST_ID), [idea_ui::SELECT_ROW_NAME]);
}

#[test]
fn table_cards_selection_boxes_are_named_and_carry_test_ids() {
    let (h, _r) = mount(|| {
        TableCards(&TableCardsProps {
            columns: vec![TableColumn::new("tip")],
            row_count: runtime_core::Reactive::Static(2),
            row_id: Rc::new(|i| RowId::from(format!("t{i}"))),
            cell: Rc::new(|i, _| ui! { text { format!("tip {i}") } }),
            selection: Some(signal(BTreeSet::new())),
            select_row_label: Some(Rc::new(|id: &RowId| format!("Select {id}"))),
            ..Default::default()
        })
    });
    // The cards are a virtualized list: mount them as a backend's window would.
    let cbs = h.virtualizer(0);
    for i in 0..2 {
        let _ = h.world.enter(|| (cbs.mount_item)(i));
    }
    h.flush();
    assert_eq!(labels(idea_ui::SELECT_ROW_TEST_ID), ["Select t0", "Select t1"]);
    assert_eq!(labels(idea_ui::SELECT_ALL_TEST_ID), [idea_ui::SELECT_ALL_NAME]);
}
