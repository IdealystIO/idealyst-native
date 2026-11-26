//! Pickers and popovers open in a bottom sheet below `Sm`, and beside the
//! trigger from `Sm` up or when told `sheet_on_phone = false`.

use std::rc::Rc;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::autocomplete::Autocomplete;
use idea_ui::components::date_input::DateInput;
use idea_ui::components::popover::Popover;
use idea_ui::date::CivilDate;
use idea_ui::components::select::{Select, SelectOption};
use runtime_core::{signal, ui, viewport_size, Element, IntoElement, Signal, ViewportSize};

type Realized = runtime_scene::Realized<host_mock::Node>;

const PHONE: f32 = 390.0;
const DESKTOP: f32 = 1440.0;

fn boot(width: f32, build: impl FnOnce() -> Element) -> (host_mock::Harness, Realized) {
    let h = host_mock::Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(width, 800.0));
        build()
    });
    let r = h.mount(tree);
    h.flush();
    (h, r)
}

/// A Select at `width` on value "a", with `sheet_on_phone` as given; the
/// value signal comes back for the pick's assertion.
fn select(width: f32, sheet_on_phone: bool) -> (host_mock::Harness, Realized, Signal<String>) {
    let cell = std::cell::Cell::new(None);
    let (h, r) = boot(width, || {
        let val = signal("a".to_string());
        cell.set(Some(val));
        ui! {
            Select(
                value = val,
                on_change = Rc::new(move |s: String| val.set(s)) as Rc<dyn Fn(String)>,
                options = vec![SelectOption::new("a", "Alpha"), SelectOption::new("b", "Beta")],
                sheet_on_phone = sheet_on_phone,
            )
        }
    });
    (h, r, cell.take().expect("built"))
}

/// Press the trigger and return what opening it logged.
fn open(h: &host_mock::Harness) -> String {
    h.take_log();
    let p = h.press_handler(0);
    h.world.enter(|| p());
    h.flush();
    h.take_log().join("\n")
}

/// The sheet is a `Modal`, which keeps itself clear of the keyboard; the
/// anchored menu is a pair of bare portals.
fn is_sheet(log: &str) -> bool {
    log.contains("mark_keyboard_avoiding")
}

#[test]
fn a_phone_select_opens_its_options_in_a_sheet_and_a_row_picks() {
    let (h, _r, value) = select(PHONE, true);
    let log = open(&h);
    assert!(is_sheet(&log), "a sheet on a phone:\n{log}");
    assert!(log.contains("text \"Beta\""), "{log}");
    h.press_labelled("Beta");
    h.flush();
    assert_eq!(h.world.enter(|| value.get()), "b");
}

#[test]
fn a_wide_select_opens_its_menu_beside_the_trigger() {
    let (h, _r, _) = select(DESKTOP, true);
    let log = open(&h);
    assert!(!is_sheet(&log), "anchored from Sm up:\n{log}");
    assert!(log.contains("portal") && log.contains("text \"Beta\""), "{log}");
}

#[test]
fn a_select_told_not_to_keeps_its_menu_anchored_on_a_phone() {
    let (h, _r, _) = select(PHONE, false);
    let log = open(&h);
    assert!(!is_sheet(&log), "sheet_on_phone = false:\n{log}");
    assert!(log.contains("portal") && log.contains("text \"Beta\""), "{log}");
}

/// An Autocomplete at `width` over three fruit, with its value signal.
fn autocomplete(width: f32, sheet_on_phone: bool) -> (host_mock::Harness, Realized, Signal<String>) {
    let cell = std::cell::Cell::new(None);
    let (h, r) = boot(width, || {
        let val = signal(String::new());
        cell.set(Some(val));
        ui! {
            Autocomplete(
                value = val,
                on_change = Rc::new(move |s: String| val.set(s)) as Rc<dyn Fn(String)>,
                options = vec![
                    SelectOption::new("apple", "Apple"),
                    SelectOption::new("pear", "Pear"),
                    SelectOption::new("plum", "Plum"),
                ],
                sheet_on_phone = sheet_on_phone,
            )
        }
    });
    (h, r, cell.take().expect("built"))
}

/// Focus the field and return what opening it logged.
fn focus_field(h: &host_mock::Harness) -> String {
    h.take_log();
    let focus = h.text_input_focus_handler(0);
    h.world.enter(|| focus(true));
    h.flush();
    h.take_log().join("\n")
}

#[test]
fn a_phone_autocomplete_searches_and_picks_in_a_sheet() {
    let (h, _r, value) = autocomplete(PHONE, true);
    host_mock::take_handle_log();
    let log = focus_field(&h);
    assert!(is_sheet(&log), "a sheet on a phone:\n{log}");
    // The field lets go of focus: the sheet's own input takes the typing.
    let handles = host_mock::take_handle_log().join("\n");
    assert!(handles.contains("blur"), "the field hands focus to the sheet:\n{handles}");
    // Typing in the sheet's input narrows the rows under it.
    let typed = h.text_input_change(1);
    h.world.enter(|| typed("pl".to_string()));
    h.flush();
    let rows = |h: &host_mock::Harness| {
        h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n")
    };
    let narrowed = rows(&h);
    assert!(narrowed.contains("\"Plum\"") && !narrowed.contains("\"Pear\""), "{narrowed}");
    h.press_labelled("Plum");
    h.flush();
    assert_eq!(h.world.enter(|| value.get()), "plum");
}

#[test]
fn a_wide_autocomplete_opens_its_menu_under_the_field() {
    let (h, _r, _) = autocomplete(DESKTOP, true);
    let log = focus_field(&h);
    assert!(!is_sheet(&log), "anchored from Sm up:\n{log}");
    assert!(log.contains("text \"Pear\""), "{log}");
}

#[test]
fn an_autocomplete_told_not_to_keeps_its_menu_anchored_on_a_phone() {
    let (h, _r, _) = autocomplete(PHONE, false);
    let log = focus_field(&h);
    assert!(!is_sheet(&log), "sheet_on_phone = false:\n{log}");
    assert!(log.contains("text \"Pear\""), "{log}");
}

/// A DateInput at `width` with its calendar button.
fn date_input(width: f32, sheet_on_phone: bool) -> (host_mock::Harness, Realized) {
    boot(width, || {
        let val: Signal<Option<CivilDate>> = signal(None);
        ui! {
            DateInput(
                value = val,
                on_change = Rc::new(move |d: Option<CivilDate>| val.set(d)) as Rc<dyn Fn(Option<CivilDate>)>,
                sheet_on_phone = sheet_on_phone,
            )
        }
    })
}

/// Press the calendar button ("Pick a date", the field's one pressable)
/// and return what opening the calendar logged.
fn open_calendar(h: &host_mock::Harness) -> String {
    assert!(h.a11y_by_label("Pick a date").is_some(), "the calendar button is named");
    open(h)
}

#[test]
fn a_phone_date_input_opens_its_calendar_in_a_sheet() {
    let (h, _r) = date_input(PHONE, true);
    let log = open_calendar(&h);
    assert!(is_sheet(&log), "a sheet on a phone:\n{log}");
}

#[test]
fn a_wide_date_input_opens_its_calendar_under_the_field() {
    let (h, _r) = date_input(DESKTOP, true);
    let log = open_calendar(&h);
    assert!(!is_sheet(&log) && log.contains("portal"), "anchored from Sm up:\n{log}");
}

#[test]
fn a_date_input_told_not_to_keeps_its_calendar_anchored_on_a_phone() {
    let (h, _r) = date_input(PHONE, false);
    let log = open_calendar(&h);
    assert!(!is_sheet(&log) && log.contains("portal"), "sheet_on_phone = false:\n{log}");
}

/// A trigger with a Popover open beside it, at `width`; returns what
/// mounting it logged.
fn popover(width: f32, sheet_on_phone: bool) -> String {
    let h = host_mock::Harness::new();
    // The width settles before the popover mounts, as it has in an app.
    h.world.enter(|| {
        install_idea_theme(light_theme());
        viewport_size().set(ViewportSize::new(width, 800.0));
    });
    h.flush();
    let tree = h.world.enter(|| {
        let anchor: runtime_core::Ref<runtime_core::ViewHandle> = runtime_core::Ref::new();
        let trigger = runtime_core::view(vec![runtime_core::text("trigger").into_element()]).bind(anchor).into_element();
        let popover = ui! {
            Popover(
                target = Some(runtime_core::AnchorTarget::from(anchor)),
                on_dismiss = Some(Rc::new(|| {}) as Rc<dyn Fn()>),
                sheet_on_phone = sheet_on_phone,
            ) {
                runtime_core::text("PANEL")
            }
        };
        runtime_core::view(vec![trigger, popover]).into_element()
    });
    let _r = h.mount(tree);
    h.flush();
    h.take_log().join("\n")
}

#[test]
fn a_popover_on_a_phone_is_a_sheet_and_beside_its_trigger_when_wide_or_told_not_to() {
    let phone = popover(PHONE, true);
    assert!(is_sheet(&phone) && phone.contains("\"PANEL\""), "a sheet on a phone:\n{phone}");
    // Anchored: a portal beside the trigger, no sheet.
    let wide = popover(DESKTOP, true);
    assert!(!is_sheet(&wide) && wide.contains("portal"), "anchored from Sm up:\n{wide}");
    let pinned = popover(PHONE, false);
    assert!(!is_sheet(&pinned) && pinned.contains("portal"), "sheet_on_phone = false:\n{pinned}");
}
