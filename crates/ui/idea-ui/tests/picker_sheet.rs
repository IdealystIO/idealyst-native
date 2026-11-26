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
    // The sheet's slide out is a timed hold; the pumped scheduler runs it.
    host_mock::pump::install_scheduler();
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

/// iOS answers the field's `blur()` at once, inside the focus handler's
/// own turn: the blur it reports is the hand-off to the sheet, not a
/// departure, and the sheet stays open.
#[test]
fn regression_a_blur_reported_during_the_hand_off_keeps_the_sheet() {
    let (h, _r, _value) = autocomplete(PHONE, true);
    h.take_log();
    let focus = h.text_input_focus_handler(0);
    h.world.enter(|| {
        focus(true);
        focus(false);
    });
    h.flush();
    let log = h.take_log().join("\n");
    assert!(is_sheet(&log), "the sheet survives the field's own blur:\n{log}");
    let tree = h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n");
    assert!(tree.contains("\"Plum\""), "its rows are up:\n{tree}");
}

/// The sheet's search is pinned above the rows, which scroll under it,
/// and a ✕ beside it closes the sheet.
#[test]
fn a_phone_autocomplete_sheet_pins_its_search_and_closes_on_its_x() {
    let (h, _r, value) = autocomplete(PHONE, true);
    let log = focus_field(&h);
    assert!(is_sheet(&log), "{log}");
    let tree = screen(&h);
    let search = tree.rfind("text_input").expect("the sheet's search");
    let scroller = tree.find("scroll_view").expect("the rows scroll");
    assert!(search < scroller, "the search sits above the scroller, not in it:\n{tree}");
    // The mock keeps no accessible names: the last three presses are the
    // rows, and the one mounted just before them is the ✕.
    let close = {
        let presses = h.shared.press_handlers.borrow();
        presses[presses.len() - 4].clone()
    };
    h.world.enter(|| close());
    h.flush();
    host_mock::pump::pump_timers();
    h.flush();
    assert!(!screen(&h).contains("portal"), "the ✕ closes the sheet:\n{}", screen(&h));
    assert_eq!(h.world.enter(|| value.get()), "", "closing picks nothing");
}

/// The keyboard lifts a sheet by its height, so the sheet's cap is the
/// room above the keyboard: capped against the whole screen, its top
/// (the search) would rise past the screen's edge.
#[test]
fn a_sheet_fits_above_the_keyboard() {
    let (h, _r, _value) = autocomplete(PHONE, true);
    runtime_vocabulary::safe_area::push(runtime_shared::EdgeInsets { bottom: 34.0, ..runtime_shared::EdgeInsets::ZERO });
    h.flush();
    h.set_style_line(|node, rules| {
        format!("apply_style n{node} max_height={:?} pad_bottom={:?}", rules.max_height, rules.padding_bottom)
    });
    let resting = focus_field(&h);
    assert!(resting.contains("pad_bottom=Some(Literal(Px(34.0)))"), "the sheet clears the home indicator:\n{resting}");
    runtime_vocabulary::keyboard::push(runtime_shared::KeyboardInset::new(
        300.0,
        runtime_shared::keyboard::WEB_KEYBOARD_ESTIMATE,
    ));
    h.flush();
    let log = h.take_log().join("\n");
    let cap = (800.0 - 300.0) * idea_ui::components::modal::SHEET_MAX_HEIGHT_FRACTION;
    assert!(log.contains(&format!("max_height=Some(Literal(Px({cap:?})))")), "capped to the room above the keyboard ({cap}):\n{log}");
    assert!(!log.contains("pad_bottom=Some(Literal(Px(34.0)))"), "no home-indicator inset under the keyboard:\n{log}");
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
    h.set_style_line(|node, rules| {
        format!(
            "apply_style n{node} align={:?} width={:?}",
            rules.align_items, rules.width
        )
    });
    let log = open_calendar(&h);
    assert!(is_sheet(&log), "a sheet on a phone:\n{log}");
    // The grid is narrower than the sheet: it sits centred, not at the start.
    assert!(
        log.contains("align=Some(Center) width=Some(Literal(Percent(100.0)))"),
        "the calendar is centred in the sheet:\n{log}",
    );
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
    host_mock::pump::install_scheduler();
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

/// Every live root's tree, joined: the portals a sheet mounts are roots
/// of their own.
fn screen(h: &host_mock::Harness) -> String {
    h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n")
}

/// A closed picker mounts nothing for its sheet, at any width: the sheet
/// is built when a phone opens it. Every Select and Autocomplete on a
/// screen carried a closed `Modal` before.
#[test]
fn regression_a_closed_picker_mounts_no_sheet() {
    for width in [PHONE, DESKTOP] {
        let (h, _r, _) = select(width, true);
        let mounted = h.take_log().join("\n");
        assert!(!mounted.contains("presence_placeholder"), "a closed Select at {width}:\n{mounted}");
        let (h, _r, _) = autocomplete(width, true);
        let mounted = h.take_log().join("\n");
        assert!(!mounted.contains("presence_placeholder"), "a closed Autocomplete at {width}:\n{mounted}");
    }
}

/// A sheet slides out before it leaves: the calendar's sheet stays
/// mounted while its exit plays and is gone, nothing left behind, when
/// the hold runs out. It used to vanish in the turn the picker closed.
#[test]
fn regression_a_closing_calendar_sheet_slides_out_before_it_leaves() {
    let (h, _r) = date_input(PHONE, true);
    let closed_roots = h.live_roots().len();
    let log = open_calendar(&h);
    assert!(is_sheet(&log), "{log}");
    let dismissals: Vec<_> = h.shared.portal_dismissals.borrow().iter().flatten().cloned().collect();
    h.world.enter(|| dismissals.iter().for_each(|d| d()));
    h.flush();
    assert!(screen(&h).contains("portal"), "still mounted while it slides out:\n{}", screen(&h));
    host_mock::pump::pump_timers();
    h.flush();
    let after = screen(&h);
    assert!(!after.contains("portal"), "gone once the slide is over:\n{after}");
    assert_eq!(h.live_roots().len(), closed_roots, "nothing left behind:\n{after}");
}

/// While a sheet slides out its rows take no presses: a second tap on
/// the leaving list must not pick again.
#[test]
fn a_leaving_sheet_takes_no_presses() {
    let (h, _r, _) = select(PHONE, true);
    // Nodes styled to take no presses, in order. Parts of an open sheet
    // already are (its layers); what the pick adds is the leaving body.
    let inert = Rc::new(std::cell::RefCell::new(Vec::<host_mock::Node>::new()));
    let seen = inert.clone();
    h.set_style_line(move |n, r| {
        if r.pointer_events == Some(runtime_core::PointerEvents::None) {
            seen.borrow_mut().push(n);
        }
        format!("apply_style n{n}")
    });
    open(&h);
    let while_open = inert.borrow().clone();
    h.press_labelled("Beta");
    h.flush();
    let leaving: Vec<_> = inert.borrow().iter().filter(|n| !while_open.contains(n)).copied().collect();
    assert!(!leaving.is_empty(), "the picked sheet's body goes inert as it leaves");
}

/// A Popover's sheet closes on its scrim even when the host gave it no
/// `on_dismiss`: a sheet covers the screen, and one that cannot close
/// would trap it.
#[test]
fn regression_a_popover_sheet_without_on_dismiss_closes_on_its_scrim() {
    let (h, _r) = boot(PHONE, || {
        let anchor: runtime_core::Ref<runtime_core::ViewHandle> = runtime_core::Ref::new();
        let trigger = runtime_core::view(vec![runtime_core::text("trigger").into_element()]).bind(anchor).into_element();
        let popover = ui! {
            Popover(target = Some(runtime_core::AnchorTarget::from(anchor))) {
                runtime_core::text("PANEL")
            }
        };
        runtime_core::view(vec![trigger, popover]).into_element()
    });
    assert!(screen(&h).contains("\"PANEL\""), "{}", screen(&h));
    let dismissals: Vec<_> = h.shared.portal_dismissals.borrow().iter().flatten().cloned().collect();
    assert!(!dismissals.is_empty(), "the sheet takes a dismissal");
    h.world.enter(|| dismissals.iter().for_each(|d| d()));
    h.flush();
    host_mock::pump::pump_timers();
    h.flush();
    assert!(!screen(&h).contains("\"PANEL\""), "the scrim closed it:\n{}", screen(&h));
}
