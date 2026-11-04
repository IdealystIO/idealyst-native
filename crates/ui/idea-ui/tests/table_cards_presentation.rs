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
use idea_ui::{install_idea_theme, light_theme, CardPresentation, CardSlot, ColumnId, RowId, TableCards, TableColumn};
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
