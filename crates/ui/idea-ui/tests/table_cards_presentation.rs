//! `TableCards` mounted on the recording host: the per-row caption and empty
//! rules, the hairline presentation, and the select-all line.
//!
//! The list is a virtualizer, which mounts rows when the backend asks, so these
//! tests ask through the callbacks a backend gets.
//!
//! CrewForge: want_ad6146ce, want_dafe7ce3, want_5e506f37, want_db31df82,
//! want_0ba282ec.

use std::collections::BTreeSet;
use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{
    install_idea_theme, light_theme, CardPresentation, CardSlot, ColumnId, RowId, TableCards, TableColumn, TableScroller,
};
use runtime_core::{signal, ui, Element, Signal};

fn cell() -> Rc<dyn Fn(usize, &ColumnId) -> Element> {
    Rc::new(|i, col| {
        let content = format!("{}-{i}", col.as_str());
        ui! { text { content } }
    })
}

fn mount(h: &Harness, build: impl FnOnce() -> Element) -> runtime_scene::Realized<u32> {
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    realized
}

/// Mount row `i` as a backend's window would, and return the whole live tree.
fn show_row(h: &Harness, i: usize) -> String {
    let cbs = h.virtualizer(0);
    let _ = h.world.enter(|| (cbs.mount_item)(i));
    h.flush();
    live(h)
}

fn live(h: &Harness) -> String {
    h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n")
}

#[test]
fn regression_card_caption_changes_per_row_and_an_empty_field_is_not_drawn() {
    let h = Harness::new();
    let _r = mount(&h, || {
        let columns = vec![
            TableColumn::new("name").card(CardSlot::Title),
            TableColumn::new("rate").label("Rate"),
            TableColumn::new("notes").label("Notes"),
        ];
        let caption: Rc<dyn Fn(usize, &ColumnId) -> Option<String>> =
            Rc::new(|i, col| (col.as_str() == "rate" && i == 0).then(|| "Day rate".to_string()));
        let empty: Rc<dyn Fn(usize, &ColumnId) -> bool> = Rc::new(|i, col| col.as_str() == "notes" && i == 0);
        ui! {
            TableCards(
                columns = columns,
                row_count = 2usize,
                cell = cell(),
                cell_caption = Some(caption),
                cell_empty = Some(empty),
            )
        }
    });
    let first = show_row(&h, 0);
    assert!(first.contains("Day rate"), "row 0 names its own caption:\n{first}");
    assert!(!first.contains("\"Rate\""), "{first}");
    assert!(!first.contains("Notes") && !first.contains("notes-0"), "row 0's empty cell is gone:\n{first}");
    let both = show_row(&h, 1);
    assert!(both.contains("\"Rate\"") && both.contains("Notes") && both.contains("notes-1"), "{both}");
}

#[test]
fn hairline_row_draws_title_meta_line_and_chip_with_no_labels() {
    let h = Harness::new();
    let _r = mount(&h, || {
        let columns = vec![
            TableColumn::new("name").card(CardSlot::Title),
            TableColumn::new("role").label("Role").card(CardSlot::Meta),
            TableColumn::new("status").label("Status").card(CardSlot::Opposite),
            TableColumn::new("alert").card(CardSlot::Trailing),
            TableColumn::new("hours").label("Hours"),
        ];
        ui! {
            TableCards(
                columns = columns,
                row_count = 1usize,
                cell = cell(),
                presentation = CardPresentation::Hairline,
            )
        }
    });
    let tree = show_row(&h, 0);
    let at = |needle: &str| tree.find(needle).unwrap_or_else(|| panic!("{needle} is drawn:\n{tree}"));
    assert!(at("name-0") < at("role-0") && at("role-0") < at("·") && at("·") < at("status-0"), "{tree}");
    assert!(at("status-0") < at("alert-0"), "{tree}");
    assert!(!tree.contains("Role") && !tree.contains("Status"), "no captions on the row:\n{tree}");
    assert!(!tree.contains("hours-0") && !tree.contains("more"), "a field waits in the sheet:\n{tree}");
}

/// With no `on_row_click` the row's press opens the detail sheet, the way to the
/// columns it doesn't draw. A row with nothing more to show has no press.
#[test]
fn hairline_row_press_opens_the_detail_sheet() {
    let h = Harness::new();
    let _r = mount(&h, || {
        let columns = vec![TableColumn::new("name").card(CardSlot::Title), TableColumn::new("hours").label("Hours")];
        ui! { TableCards(columns = columns, row_count = 1usize, cell = cell(), presentation = CardPresentation::Hairline) }
    });
    show_row(&h, 0);
    assert_eq!(h.shared.touch_handlers.borrow().len(), 1, "the row carries a tap");

    let bare = Harness::new();
    let _r = mount(&bare, || {
        let columns = vec![TableColumn::new("name").card(CardSlot::Title)];
        ui! { TableCards(columns = columns, row_count = 1usize, cell = cell(), presentation = CardPresentation::Hairline) }
    });
    show_row(&bare, 0);
    assert_eq!(bare.shared.touch_handlers.borrow().len(), 0, "nothing to open, so no press to swallow");
}

/// want_0ba282ec: a selectable list has a pinned select-all line: it says how
/// many rows are selected, ticks every row, and clears them again. The rows are
/// live, so a row that arrives later takes part.
#[test]
fn regression_cards_selection_has_a_select_all_line() {
    let h = Harness::new();
    let selection: Rc<std::cell::Cell<Option<Signal<BTreeSet<RowId>>>>> = Rc::default();
    let count: Rc<std::cell::Cell<Option<Signal<usize>>>> = Rc::default();
    let (sel_out, count_out) = (selection.clone(), count.clone());
    let _r = mount(&h, move || {
        let picked = signal(BTreeSet::<RowId>::new());
        let rows = signal(2usize);
        sel_out.set(Some(picked));
        count_out.set(Some(rows));
        let columns = vec![TableColumn::new("name").card(CardSlot::Title)];
        ui! { TableCards(columns = columns, row_count = rows, cell = cell(), selection = Some(picked)) }
    });
    let (picked, rows) = (selection.get().unwrap(), count.get().unwrap());
    assert!(live(&h).contains("Select all"), "{}", live(&h));

    let press_box = h.press_handler(0);
    press_box();
    h.flush();
    assert_eq!(picked.peek().len(), 2);
    assert!(live(&h).contains("2 selected"), "{}", live(&h));

    rows.set(3);
    h.flush();
    press_box();
    h.flush();
    assert_eq!(picked.peek().len(), 3, "some of the rows selected: the press takes the new row too");

    press_box();
    h.flush();
    assert!(picked.peek().is_empty(), "all selected: the press clears them");
    assert!(live(&h).contains("Select all"), "{}", live(&h));
}

/// A screen with its own select-all header turns the line off, or a phone shows
/// two. The cards keep their boxes, so the selection still works.
#[test]
fn regression_selection_header_off_leaves_only_the_cards_boxes() {
    let h = Harness::new();
    let selection: Rc<std::cell::Cell<Option<Signal<BTreeSet<RowId>>>>> = Rc::default();
    let sel_out = selection.clone();
    let _r = mount(&h, move || {
        let picked = signal(BTreeSet::<RowId>::new());
        sel_out.set(Some(picked));
        let columns = vec![TableColumn::new("name").card(CardSlot::Title)];
        ui! {
            TableCards(
                columns = columns,
                row_count = 2usize,
                cell = cell(),
                selection = Some(picked),
                selection_header = false,
            )
        }
    });
    assert!(!live(&h).contains("Select all"), "{}", live(&h));
    show_row(&h, 0);
    h.press_handler(0)();
    h.flush();
    assert_eq!(selection.get().unwrap().peek().len(), 1, "the press took one row, not every row");
    assert!(!live(&h).contains("1 selected"), "{}", live(&h));
}

/// want_40fc88e7: the list's scroller reaches a row by id, through the
/// virtualizer's index scroll, so a row that isn't mounted is still reachable.
#[test]
fn regression_table_scroller_scrolls_the_cards_to_a_row() {
    let h = Harness::new();
    let scroller = TableScroller::new();
    let held = scroller.clone();
    let realized = mount(&h, move || {
        let columns = vec![TableColumn::new("name").card(CardSlot::Title)];
        let row_id: Rc<dyn Fn(usize) -> RowId> = Rc::new(|i| RowId::from(format!("row-{i}").as_str()));
        ui! { TableCards(columns = columns, row_count = 30usize, row_id = row_id, cell = cell(), scroller = Some(held)) }
    });
    host_mock::take_handle_log();
    scroller.scroll_to_row(&RowId::from("row-17"));
    let log = host_mock::take_handle_log();
    assert!(log.iter().any(|l| l.starts_with("scroll_to_index 17")), "{log:?}");

    scroller.scroll_to_row(&RowId::from("not-a-row"));
    assert!(host_mock::take_handle_log().is_empty(), "an unknown id does nothing");

    drop(realized);
    h.flush();
    scroller.scroll_to_row(&RowId::from("row-3"));
    assert!(host_mock::take_handle_log().is_empty(), "an unmounted list isn't reached");
}

/// The table SDK fills the scroller handle on every shape of scrolling table:
/// plain, pinned head (split) and a two-axis one. Row frames are the backend's,
/// which the recording host's handles don't report, so `T/scroller.rs` pins the
/// arithmetic.
#[test]
fn table_sdk_binds_the_rows_scroller_for_every_scrolling_shape() {
    for (scroll_x, scroll_y, head) in [(false, true, false), (false, true, true), (true, true, true), (true, false, false)] {
        let h = Harness::new();
        let handle: Rc<std::cell::Cell<Option<runtime_core::Ref<runtime_core::ScrollViewHandle>>>> = Rc::default();
        let out = handle.clone();
        let _r = mount(&h, move || {
            let body: runtime_core::Ref<runtime_core::ScrollViewHandle> = runtime_core::Ref::new();
            out.set(Some(body));
            let cell = |content: &str, header: bool| {
                let content = content.to_string();
                let c = table::table_cell(table::TableCellProps {
                    children: vec![ui! { text { content } }],
                    header,
                    ..Default::default()
                });
                runtime_core::IntoElement::into_element(c)
            };
            let row = |c: Element| runtime_core::IntoElement::into_element(table::table_row(table::TableRowProps { children: vec![c] }));
            let mut children = vec![row(cell("Body", false))];
            if head {
                children.insert(0, row(cell("Head", true)));
            }
            runtime_core::IntoElement::into_element(
                table::table(table::TableProps { children, scroll_x, scroll_y, ..Default::default() })
                    .bind_scroller(body, |_, _| {}),
            )
        });
        host_mock::take_handle_log();
        let body = handle.get().expect("built");
        assert!(body.with(|h| h.scroll_to(0.0, 120.0)).is_some(), "scroller filled for x={scroll_x} y={scroll_y} head={head}");
        let log = host_mock::take_handle_log();
        assert!(log.iter().any(|l| l.starts_with("scroll_to 0 120")), "{log:?}");
    }
}

/// want_d4d6a0c8: a `long_press` wrapped around a `TableCards` list never
/// saw a hold on a phone row, because the row's own tap consumes the
/// touch. The row's press handler takes `on_row_menu` instead: a hold
/// opens the menu for that row, and does not also open the row.
#[test]
fn regression_hairline_row_hold_opens_the_row_menu() {
    use runtime_core::{TouchEvent, TouchId, TouchPhase, TouchPoint};
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let clicks = Rc::new(std::cell::RefCell::new(Vec::<usize>::new()));
    let menus = Rc::new(std::cell::RefCell::new(Vec::<usize>::new()));
    let (c, m) = (clicks.clone(), menus.clone());
    let _r = mount(&h, move || {
        let columns = vec![TableColumn::new("name").card(CardSlot::Title)];
        let on_click: Rc<dyn Fn(usize)> = Rc::new(move |i| c.borrow_mut().push(i));
        let on_menu: Rc<dyn Fn(usize, TouchPoint)> = Rc::new(move |i, _| m.borrow_mut().push(i));
        ui! {
            TableCards(
                columns = columns,
                row_count = 2usize,
                cell = cell(),
                presentation = CardPresentation::Hairline,
                on_row_click = Some(on_click),
                on_row_menu = Some(on_menu),
            )
        }
    });
    show_row(&h, 1);
    let handler = h.shared.touch_handlers.borrow().last().map(|(_, f)| f.clone()).expect("the row takes presses");
    let at = |phase, t_ms: u64| TouchEvent {
        id: TouchId(1),
        phase,
        position: TouchPoint::new(5.0, 5.0),
        window_position: TouchPoint::new(5.0, 5.0),
        timestamp_ns: t_ms * 1_000_000,
        force: None,
    };
    h.world.enter(|| handler(&at(TouchPhase::Began, 0)));
    host_mock::pump::pump_timers();
    h.flush();
    h.world.enter(|| handler(&at(TouchPhase::Ended, 600)));
    h.flush();
    assert_eq!(*menus.borrow(), vec![1], "the hold opened row 1's menu");
    assert!(clicks.borrow().is_empty(), "and not the row");
}
