//! Reactive-branch behavior of idea-ui components, pinned at the
//! MOUNTED-tree level.
//!
//! Every component here swaps part of its subtree when a signal changes
//! (a glyph that exists only while checked, a popup that exists only while
//! open, a pager row rebuilt per page). Those swaps used to be wired with
//! hand-called `runtime_core::switch` / `when`; most now live as `if` /
//! `match` inside `ui!`. These tests were written against the hand-wired
//! versions FIRST and kept unchanged across the conversion, so they are
//! the evidence that the conversion changed spelling, not behavior.
//!
//! What is asserted is what a backend would actually be showing —
//! `Harness::live_tree`, with node ids stripped where only the SHAPE
//! matters, and kept where node IDENTITY matters (a part that must not be
//! rebuilt by the swap, e.g. the Checkbox's pressable or the Collapsible's
//! body).
//!
//! Mounted through the real `realize` path against `host-mock`: the
//! branch machinery is backend-independent (the scene's guarded holes),
//! so the mock exercises the same code a real backend runs.

use std::rc::Rc;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::autocomplete::Autocomplete;
use idea_ui::components::checkbox::Checkbox;
use idea_ui::components::collapsible::Collapsible;
use idea_ui::components::date_input::{DateInput, DateTimeInput};
use idea_ui::components::date_picker::{DatePicker, DateRangePicker, DateTimePicker};
use idea_ui::components::menu::{MenuEntry, SubMenu};
use idea_ui::components::pagination::Pagination;
use idea_ui::components::radio::Radio;
use idea_ui::components::select::{Select, SelectOption};
use idea_ui::components::tooltip::Tooltip;
use idea_ui::date::{CivilDate, CivilDateTime};
use runtime_core::{signal, text, ui, Element, Signal};

type Realized = runtime_scene::Realized<host_mock::Node>;

/// Mount `build` (theme installed) and settle.
fn boot(build: impl FnOnce() -> Element) -> (host_mock::Harness, Realized) {
    let h = host_mock::Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let r = h.mount(tree);
    h.flush();
    (h, r)
}

/// Every live root's tree, joined — top-level fragments (Tooltip) mount as
/// sibling roots.
fn screen(h: &host_mock::Harness) -> String {
    h.live_roots()
        .iter()
        .map(|r| h.live_tree(*r))
        .collect::<Vec<_>>()
        .join("\n--\n")
}

/// [`screen`] with the `nNN ` node ids stripped: the SHAPE.
fn shape(h: &host_mock::Harness) -> String {
    screen(h)
        .lines()
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let rest = line.trim_start();
            let rest = match rest.split_once(' ') {
                Some((id, kind)) if id.starts_with('n') && id[1..].parse::<u32>().is_ok() => kind,
                _ => rest,
            };
            format!("{}{}", &line[..indent], rest)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The node id on the first live line whose kind is `kind` (e.g.
/// `"text \"body\""`), for identity assertions.
fn node_of(h: &host_mock::Harness, kind: &str) -> Option<String> {
    screen(h).lines().find_map(|l| {
        let (id, k) = l.trim_start().split_once(' ')?;
        (k == kind).then(|| id.to_string())
    })
}

fn act(h: &host_mock::Harness, f: impl FnOnce()) {
    h.world.enter(f);
    h.flush();
}

fn press(h: &host_mock::Harness, i: usize) {
    let p = h.press_handler(i);
    act(h, || p());
}

fn press_count(h: &host_mock::Harness) -> usize {
    h.shared.press_handlers.borrow().len()
}

fn hover(h: &host_mock::Harness, i: usize, entering: bool) {
    let handler = h.shared.hover_handlers.borrow()[i].1.clone();
    act(h, || handler(entering));
}

/// Fire every portal's dismiss handler — the backdrop tap / Escape path.
fn dismiss_portals(h: &host_mock::Harness) {
    let handlers: Vec<_> = h.shared.portal_dismissals.borrow().iter().flatten().cloned().collect();
    act(h, || handlers.iter().for_each(|d| d()));
}

// ---------------------------------------------------------------------------
// Presence-by-boolean: a node that exists only while a signal is true.
// ---------------------------------------------------------------------------

#[test]
fn checkbox_glyph_exists_only_while_checked_and_box_is_not_rebuilt() {
    let value = std::cell::Cell::new(None::<Signal<bool>>);
    let (h, _r) = boot(|| {
        let on = signal(false);
        value.set(Some(on));
        ui! { Checkbox(value = on, on_change = Rc::new(move |b: bool| on.set(b)) as Rc<dyn Fn(bool)>) }
    });
    let on = value.get().unwrap();
    assert_eq!(shape(&h), "pressable\n  anchor\n    view");
    let box_node = node_of(&h, "pressable").unwrap();

    act(&h, || on.set(true));
    assert_eq!(shape(&h), "pressable\n  anchor\n    text \"✓\"");
    assert_eq!(node_of(&h, "pressable").unwrap(), box_node, "the box must not remount");

    act(&h, || on.set(false));
    assert_eq!(shape(&h), "pressable\n  anchor\n    view");
    assert_eq!(node_of(&h, "pressable").unwrap(), box_node);
}

#[test]
fn radio_dot_swaps_with_selection_inside_a_stable_ring() {
    let value = std::cell::Cell::new(None::<Signal<bool>>);
    let (h, _r) = boot(|| {
        let on = signal(false);
        value.set(Some(on));
        ui! { Radio(selected = on, on_select = Rc::new(move || on.set(true)) as Rc<dyn Fn()>) }
    });
    let on = value.get().unwrap();
    let ring = node_of(&h, "pressable").unwrap();
    assert_eq!(shape(&h), "pressable\n  anchor\n    view");
    let slot_before = h.children_of(h.children_of(h.live_roots()[0])[0])[0];

    act(&h, || on.set(true));
    assert_eq!(shape(&h), "pressable\n  anchor\n    view");
    let slot_after = h.children_of(h.children_of(h.live_roots()[0])[0])[0];
    assert_ne!(slot_before, slot_after, "selection must swap the placeholder for the dot");
    assert_eq!(node_of(&h, "pressable").unwrap(), ring, "the ring must not remount");

    // Same value again: no swap (the branch is keyed on the boolean).
    act(&h, || on.set(true));
    let slot_again = h.children_of(h.children_of(h.live_roots()[0])[0])[0];
    assert_eq!(slot_after, slot_again);
}

#[test]
fn collapsible_chevron_follows_value_without_touching_the_body() {
    let value = std::cell::Cell::new(None::<Signal<bool>>);
    let (h, _r) = boot(|| {
        let on = signal(false);
        value.set(Some(on));
        ui! {
            Collapsible(
                title = "T".to_string(),
                value = on,
                on_change = Rc::new(move |b: bool| on.set(b)) as Rc<dyn Fn(bool)>,
            ) {
                text("body")
            }
        }
    });
    let on = value.get().unwrap();
    let body = node_of(&h, "text \"body\"").unwrap();
    assert!(node_of(&h, "text \"›\"").is_some(), "{}", screen(&h));
    assert!(node_of(&h, "text \"⌄\"").is_none());

    act(&h, || on.set(true));
    assert!(node_of(&h, "text \"⌄\"").is_some(), "{}", screen(&h));
    assert!(node_of(&h, "text \"›\"").is_none());
    assert_eq!(node_of(&h, "text \"body\"").unwrap(), body, "the body must not remount");

    act(&h, || on.set(false));
    assert!(node_of(&h, "text \"›\"").is_some(), "{}", screen(&h));
}

// ---------------------------------------------------------------------------
// Popups: a panel that exists only while the component is open.
// ---------------------------------------------------------------------------

#[test]
fn select_menu_exists_only_while_open() {
    let (h, _r) = boot(|| {
        let val = signal("a".to_string());
        ui! {
            Select(
                value = val,
                on_change = Rc::new(move |s: String| val.set(s)) as Rc<dyn Fn(String)>,
                options = vec![SelectOption::new("a", "Alpha"), SelectOption::new("b", "Beta")],
            )
        }
    });
    let closed = shape(&h);
    assert!(!closed.contains("portal"), "{closed}");
    let trigger = node_of(&h, "text \"Alpha\"").unwrap();

    press(&h, 0);
    let open = shape(&h);
    assert!(open.contains("portal"), "{open}");
    assert!(open.contains("text \"Beta\""), "{open}");
    assert_eq!(node_of(&h, "text \"Alpha\"").unwrap(), trigger, "the trigger must not remount");

    dismiss_portals(&h);
    assert_eq!(shape(&h), closed);
}

#[test]
fn tooltip_bubble_exists_only_while_hovered() {
    let (h, _r) = boot(|| ui! { Tooltip(text = "tip".to_string()) { text("trigger") } });
    let closed = shape(&h);
    assert_eq!(closed, "view\n  text \"trigger\"\n--\nanchor\n  view");

    hover(&h, 0, true);
    assert_eq!(
        shape(&h),
        "view\n  text \"trigger\"\n--\nanchor\n  portal\n    view\n      view\n        text \"tip\""
    );

    hover(&h, 0, false);
    assert_eq!(shape(&h), closed);
}

#[test]
fn submenu_flyout_opens_on_hover_with_its_rows() {
    let (h, _r) = boot(|| {
        ui! {
            SubMenu(
                label = "More".to_string(),
                items = vec![
                    MenuEntry::new("One", Rc::new(|| {})),
                    MenuEntry::new("Two", Rc::new(|| {})),
                ],
            )
        }
    });
    let closed = shape(&h);
    assert!(!closed.contains("portal"), "{closed}");
    let row = node_of(&h, "text \"More\"").unwrap();

    hover(&h, 0, true);
    let open = shape(&h);
    assert!(open.contains("portal"), "{open}");
    assert!(open.contains("text \"One\"") && open.contains("text \"Two\""), "{open}");
    assert_eq!(node_of(&h, "text \"More\"").unwrap(), row);

    dismiss_portals(&h);
    assert_eq!(shape(&h), closed);
}

#[test]
fn autocomplete_panel_opens_on_typing_with_the_filtered_rows() {
    let (h, _r) = boot(|| {
        let val = signal("a".to_string());
        ui! {
            Autocomplete(
                value = val,
                on_change = Rc::new(move |s: String| val.set(s)) as Rc<dyn Fn(String)>,
                options = vec![SelectOption::new("a", "Alpha"), SelectOption::new("b", "Beta")],
            )
        }
    });
    let closed = shape(&h);
    assert!(!closed.contains("portal"), "{closed}");

    let type_query = h.text_input_change(0);
    act(&h, || type_query("B".into()));
    let open = shape(&h);
    assert!(open.contains("portal"), "{open}");
    assert!(open.contains("text \"Beta\"") && !open.contains("text \"Alpha\""), "{open}");

    dismiss_portals(&h);
    assert_eq!(shape(&h), closed);
}

/// Opens a date-family component with the press handler `pick` selects
/// and checks the popup appears and dismisses back to the closed shape.
fn assert_date_popup(build: impl FnOnce() -> Element, pick: impl Fn(usize) -> usize) {
    let (h, _r) = boot(build);
    let closed = shape(&h);
    assert!(!closed.contains("portal"), "{closed}");

    press(&h, pick(press_count(&h)));
    let open = shape(&h);
    assert!(open.contains("portal"), "{open}");

    dismiss_portals(&h);
    assert_eq!(shape(&h), closed);
}

#[test]
fn date_family_popups_exist_only_while_open() {
    // Pickers open from their trigger (the first pressable); inputs from
    // the trailing calendar adornment (the last one).
    let first = |_: usize| 0;
    let last = |n: usize| n - 1;
    assert_date_popup(
        || {
            let d: Signal<Option<CivilDate>> = signal(None);
            ui! { DatePicker(value = d, on_change = Rc::new(move |x| d.set(x)) as Rc<dyn Fn(Option<CivilDate>)>) }
        },
        first,
    );
    assert_date_popup(
        || {
            let d: Signal<Option<CivilDateTime>> = signal(None);
            ui! { DateTimePicker(value = d, on_change = Rc::new(move |x| d.set(x)) as Rc<dyn Fn(Option<CivilDateTime>)>) }
        },
        first,
    );
    assert_date_popup(
        || {
            let d: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
            ui! {
                DateRangePicker(
                    value = d,
                    on_change = Rc::new(move |x| d.set(x)) as Rc<dyn Fn(Option<(CivilDate, CivilDate)>)>,
                )
            }
        },
        first,
    );
    assert_date_popup(
        || {
            let d: Signal<Option<CivilDate>> = signal(None);
            ui! { DateInput(value = d, on_change = Rc::new(move |x| d.set(x)) as Rc<dyn Fn(Option<CivilDate>)>) }
        },
        last,
    );
    assert_date_popup(
        || {
            let d: Signal<Option<CivilDateTime>> = signal(None);
            ui! { DateTimeInput(value = d, on_change = Rc::new(move |x| d.set(x)) as Rc<dyn Fn(Option<CivilDateTime>)>) }
        },
        last,
    );
}

// ---------------------------------------------------------------------------
// Pagination: the whole row rebuilds per page (a legitimate direct
// `switch` — see the component).
// ---------------------------------------------------------------------------

fn page_labels(h: &host_mock::Harness) -> Vec<String> {
    shape(h)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("text \"")?.strip_suffix('"').map(str::to_string))
        .collect()
}

#[test]
fn pagination_window_slides_with_page() {
    let page_cell = std::cell::Cell::new(None::<Signal<usize>>);
    let (h, _r) = boot(|| {
        let page = signal(1usize);
        page_cell.set(Some(page));
        ui! { Pagination(page = page, total = 20usize, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>) }
    });
    let page = page_cell.get().unwrap();
    assert_eq!(page_labels(&h), ["‹", "1", "2", "…", "20", "›"]);

    act(&h, || page.set(10));
    assert_eq!(page_labels(&h), ["‹", "1", "…", "9", "10", "11", "…", "20", "›"]);
}

/// `total` was snapshotted at build (the row was keyed on `page` alone), so
/// a live page count — a result set that grows as it loads — kept the
/// original window: the last-page cell and the next-arrow target never
/// moved.
#[test]
fn regression_pagination_live_total_resizes_the_window() {
    let cells = std::cell::Cell::new(None::<(Signal<usize>, Signal<usize>)>);
    let (h, _r) = boot(|| {
        let page = signal(1usize);
        let total = signal(3usize);
        cells.set(Some((page, total)));
        ui! { Pagination(page = page, total = total, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>) }
    });
    let (_page, total) = cells.get().unwrap();
    assert_eq!(page_labels(&h), ["‹", "1", "2", "3", "›"]);

    act(&h, || total.set(20));
    assert_eq!(page_labels(&h), ["‹", "1", "2", "…", "20", "›"]);
}

// ---------------------------------------------------------------------------
// Calendar: one grid per zoom level (a reactive `match`).
// ---------------------------------------------------------------------------

#[test]
fn calendar_body_swaps_grid_per_zoom_and_rebuilds_per_month() {
    use idea_ui::components::calendar::Calendar;
    let (h, _r) = boot(|| {
        let d: Signal<Option<CivilDate>> = signal(CivilDate::new(2026, 8, 3));
        ui! { Calendar(value = d, on_change = Rc::new(move |x: CivilDate| d.set(Some(x))) as Rc<dyn Fn(CivilDate)>) }
    });
    // Header pressables in tree order: prev, title, next.
    const TITLE: usize = 1;
    const NEXT: usize = 2;
    let labels = page_labels(&h);
    assert_eq!(&labels[..8], ["August 2026", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
    assert_eq!(labels.len(), 1 + 7 + 42, "title + weekday row + six weeks: {labels:?}");

    // Title cycles Days → Months → Years → Days.
    press(&h, TITLE);
    assert_eq!(
        page_labels(&h),
        ["2026", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    );
    press(&h, TITLE);
    let years = page_labels(&h);
    assert_eq!(years[0], "2016 – 2027");
    assert_eq!(years[1..], (2016..=2027).map(|y| y.to_string()).collect::<Vec<_>>()[..]);
    press(&h, TITLE);
    let back = page_labels(&h);
    assert_eq!(back, labels, "back at Days, same month");

    // Month navigation rebuilds the day grid for the new month.
    press(&h, NEXT);
    let sept = page_labels(&h);
    assert_eq!(sept[0], "September 2026");
    assert_ne!(sept[8..], labels[8..], "the day cells follow the visible month");
}
