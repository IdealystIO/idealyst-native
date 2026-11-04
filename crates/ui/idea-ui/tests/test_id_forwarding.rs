//! Every idea-ui `test_id` reaches the robot registry ON THE INTERACTIVE
//! PRIMITIVE, in a build shaped like `idealyst dev` / `idealyst test`: the
//! runtime's `robot` feature on (this crate's dev-dependency on
//! `runtime-vocabulary/robot`), idea-ui's own `robot` feature OFF.
//!
//! Regressions pinned here:
//!
//! - **test ids dropped in dev builds.** The forwarding sat behind
//!   idea-ui's own `#[cfg(feature = "robot")]`. The dev/test builds turn on
//!   the framework's robot features, never idea-ui's, so
//!   `app.test_id("login-submit").click()` found nothing for Button,
//!   Switch, Checkbox and Link. Under a bare `cargo test -p idea-ui` every
//!   lookup below fails against that gate.
//! - **a loading Button's test id on a wrapper.** With a live `loading`
//!   the Button rebuilds its pressable per state and parked the id on a
//!   transparent wrapper `view`, so `find_element` returned a View — and
//!   iOS `tap_native` on it failed ("action 'click' not available").
//!
//! The kind checks are what makes each lookup meaningful: a test id on a
//! label row or a layout wrapper would still be "found", but a robot
//! `fill` / `click` on it does nothing.

use std::rc::Rc;

use idea_ui::components::date_input::{DateInput, DateTimeInput};
use idea_ui::components::date_picker::{DatePicker, DateRangePicker, DateTimePicker};
use idea_ui::components::list::ListItem;
use idea_ui::components::menu::{MenuEntry, MenuItem, SubMenu};
use idea_ui::components::time_input::TimeInput;
use idea_ui::date::{CivilDate, CivilDateTime, CivilTime};
use idea_ui::{
    Adornment, install_idea_theme, light_theme, Autocomplete, Button, Checkbox, Chip, Collapsible, Field, IconButton, Link,
    Radio, RadioGroup, RadioOption, SegmentOption, SegmentedControl, Select, SelectOption, Slider, Switch, Tab,
    Tabs, Tag, Textarea,
};
use runtime_core::{rx, signal, ui, Element, Signal};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

/// Mount `build` on a fresh recording host with a clean robot registry.
fn mount<T>(build: impl FnOnce() -> (Element, T)) -> (host_mock::Harness, runtime_scene::Realized<u32>, T) {
    Robot::new().reset();
    let h = host_mock::Harness::new();
    let (tree, extra) = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized, extra)
}

/// The one live holder of `id`, asserting it is a `kind`.
fn holder(id: &str, kind: ElementKind) -> runtime_vocabulary::robot::Element {
    let robot = Robot::new();
    let all = robot.find_all(Query::test_id(id));
    assert_eq!(all.len(), 1, "exactly one live element carries test_id {id:?}, got {all:?}");
    let el = robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("no element matches test_id {id:?}"));
    assert_eq!(el.kind, kind, "test_id {id:?} must sit on the interactive primitive");
    el
}

const GLYPH: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M6 6l12 12"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

fn noop() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}

#[test]
fn regression_button_family_test_ids_reach_the_registry_without_idea_ui_robot() {
    let (_h, _r, ()) = mount(|| {
        let on = signal(false);
        let picked = signal(false);
        let tree = ui! {
            view {
                Button(label = "Sign in".to_string(), on_click = noop(), test_id = Some("submit"))
                IconButton(glyph = "x".to_string(), on_click = noop(), tooltip = Some("Close".to_string()), test_id = Some("close"))
                Switch(value = on, label = Some("Wi-Fi".to_string()), test_id = Some("wifi"))
                Checkbox(value = on, label = Some("Agree".to_string()), test_id = Some("agree"))
                Radio(selected = picked, on_select = noop(), label = Some("One".to_string()), test_id = Some("radio-one"))
                Link(label = "Docs".to_string(), url = "https://example.com".to_string(), test_id = Some("docs-link"))
                Link(label = "More".to_string(), on_press = Some(noop()), test_id = Some("more-link"))
                Chip(label = "Open".to_string(), on_select = Some(noop()), test_id = Some("chip"))
                Tag(label = "rust".to_string(), on_remove = Some(noop()), test_id = Some("tag-remove"))
                Tag(label = "go".to_string(), test_id = Some("tag-plain"))
                ListItem(label = "Row".to_string(), on_press = Some(noop()), test_id = Some("row"))
                MenuItem(label = "Rename".to_string(), on_select = noop(), test_id = Some("rename"))
                SubMenu(label = "Move to".to_string(), items = vec![MenuEntry::new("Inbox", noop()).test_id("move-inbox")], test_id = Some("move-to"))
                Collapsible(title = "Details".to_string(), test_id = Some("details")) {
                    text { "body" }
                }
                Slider(value = 0.5, test_id = Some("volume"))
            }
        };
        (tree, ())
    });
    holder("submit", ElementKind::Pressable);
    holder("close", ElementKind::Pressable);
    holder("wifi", ElementKind::Pressable);
    holder("agree", ElementKind::Pressable);
    holder("radio-one", ElementKind::Pressable);
    holder("docs-link", ElementKind::Link);
    holder("more-link", ElementKind::Pressable);
    holder("chip", ElementKind::Pressable);
    holder("tag-remove", ElementKind::Pressable);
    holder("tag-plain", ElementKind::View);
    holder("row", ElementKind::Pressable);
    holder("rename", ElementKind::Pressable);
    holder("move-to", ElementKind::Pressable);
    holder("details", ElementKind::Pressable);
    // The Slider is a drag container (a view with the touch handler).
    holder("volume", ElementKind::View);
}

#[test]
fn regression_text_entry_test_ids_land_on_the_input() {
    let (_h, _r, ()) = mount(|| {
        let text = signal(String::new());
        let time: Signal<Option<CivilTime>> = signal(None);
        let date: Signal<Option<CivilDate>> = signal(None);
        let dt: Signal<Option<CivilDateTime>> = signal(None);
        let tree = ui! {
            view {
                Field(label = Some("Email".into()), value = text, test_id = Some("email"))
                Field(value = text, secure = true, trailing = Adornment::Button { icon: GLYPH, label: None, on_press: noop() }, test_id = Some("password"))
                Textarea(label = Some("Notes".into()), value = text, test_id = Some("notes"))
                TimeInput(value = time, test_id = Some("start-time"))
                DateInput(value = date, test_id = Some("start-date"))
                DateTimeInput(value = dt, test_id = Some("starts-at"))
                Autocomplete(value = text, options = vec![SelectOption::new("a", "Apple")], test_id = Some("fruit"))
            }
        };
        (tree, ())
    });
    for id in ["email", "password", "notes", "start-time", "start-date", "starts-at", "fruit"] {
        holder(id, ElementKind::TextInput);
    }
}

#[test]
fn regression_picker_and_group_test_ids_land_on_their_pressables() {
    let (h, _r, ()) = mount(|| {
        let value = signal("a".to_string());
        let date: Signal<Option<CivilDate>> = signal(None);
        let dt: Signal<Option<CivilDateTime>> = signal(None);
        let range: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
        let tabs = signal(vec![Tab::new("a", "A").test_id("tab-a"), Tab::new("b", "B").test_id("tab-b")]);
        let tree = ui! {
            view {
                Select(
                    value = value,
                    options = vec![SelectOption::new("a", "Apple").test_id("opt-apple"), SelectOption::new("b", "Pear")],
                    test_id = Some("fruit-select"),
                )
                DatePicker(value = date, test_id = Some("due"))
                DateTimePicker(value = dt, test_id = Some("due-at"))
                DateRangePicker(value = range, test_id = Some("period"))
                SegmentedControl(
                    value = value,
                    options = vec![SegmentOption::new("a", "List").test_id("seg-list"), SegmentOption::new("b", "Grid").test_id("seg-grid")],
                )
                Tabs(tabs = tabs, active = value)
                RadioGroup(
                    value = value,
                    options = vec![RadioOption::new("a", "One").test_id("rg-one"), RadioOption::new("b", "Two").test_id("rg-two")],
                )
            }
        };
        (tree, ())
    });
    for id in [
        "fruit-select", "due", "due-at", "period", "seg-list", "seg-grid", "tab-a", "tab-b", "rg-one", "rg-two",
    ] {
        holder(id, ElementKind::Pressable);
    }

    // The option rows exist only while the menu is open: a robot click on
    // the trigger opens it, and the row carries its own test id.
    let robot = Robot::new();
    assert!(robot.find(Query::test_id("opt-apple")).is_none(), "closed menu, no rows");
    let trigger = holder("fruit-select", ElementKind::Pressable);
    h.world.enter(|| robot.click(&trigger)).expect("the trigger takes a click");
    h.flush();
    holder("opt-apple", ElementKind::Pressable);
}

/// A live `loading` rebuilds the Button's pressable per state. The test id
/// must ride the pressable in every state — one live holder, a
/// `Pressable` — and stay clickable once loading clears.
#[test]
fn regression_loading_button_test_id_is_on_the_pressable_across_rebuilds() {
    let clicks = Rc::new(std::cell::Cell::new(0));
    let (h, _r, busy) = {
        let clicks = clicks.clone();
        mount(move || {
            let busy = signal(false);
            let tree = ui! {
                Button(
                    label = "Save".to_string(),
                    on_click = Rc::new(move || clicks.set(clicks.get() + 1)) as Rc<dyn Fn()>,
                    loading = rx!(busy.get()),
                    test_id = Some("save"),
                )
            };
            (tree, busy)
        })
    };
    let robot = Robot::new();
    let before = holder("save", ElementKind::Pressable);
    assert!(
        robot.find_all(Query::Kind(ElementKind::View)).iter().all(|v| v.test_id != Some("save")),
        "no wrapper view carries the id"
    );

    h.world.enter(|| busy.set(true));
    h.flush();
    let loading = holder("save", ElementKind::Pressable);
    assert_ne!(loading.id, before.id, "the loading arm is a rebuilt pressable");
    assert!(h.world.enter(|| robot.click(&loading)).is_err(), "a loading Button refuses the click");

    h.world.enter(|| busy.set(false));
    h.flush();
    let after = holder("save", ElementKind::Pressable);
    h.world.enter(|| robot.click(&after)).expect("the rebuilt pressable takes the click");
    assert_eq!(clicks.get(), 1, "the click reached on_click");
}

/// CrewForge want_4e521378: nothing could press an idea-ui Table row from a
/// robot suite. `TableRow(test_id = …)` anchors the row, a click on it (or on any
/// text in the row) fires `on_row_click`, and a row without a handler is not
/// pressable.
#[test]
fn regression_robot_presses_a_table_row_by_test_id() {
    let robot = runtime_vocabulary::robot::Robot::new();
    robot.reset();
    let h = host_mock::Harness::new();
    let pressed = std::rc::Rc::new(std::cell::Cell::new(0));
    let sink = pressed.clone();
    let tree = h.world.enter(|| {
        idea_ui::install_idea_theme(idea_ui::light_theme());
        let on_row_click: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || sink.set(sink.get() + 1));
        use idea_ui::{Table, TableCell, TableRow};
        runtime_core::ui! {
            Table {
                TableRow {
                    TableCell(header = true, text = Some("Name".to_string()))
                    TableCell(header = true, text = Some("Hours".to_string()))
                }
                TableRow(on_row_click = Some(on_row_click), test_id = Some("intake")) {
                    TableCell(text = Some("New Hire Intake".to_string()))
                    TableCell(text = Some("12".to_string()))
                }
                TableRow(test_id = Some("plain")) {
                    TableCell(text = Some("Audit".to_string()))
                    TableCell(text = Some("3".to_string()))
                }
            }
        }
    });
    let _realized = h.mount(tree);
    h.flush();

    use runtime_vocabulary::robot::Query;
    let row = robot.find(Query::test_id("intake")).expect("the row's anchor is registered");
    robot.click(&row).expect("the anchored row presses");
    assert_eq!(pressed.get(), 1);

    let text = robot.find(Query::label("12")).expect("another cell's text");
    robot.click(&text).expect("a click anywhere in the row reaches its handler");
    assert_eq!(pressed.get(), 2);

    let plain = robot.find(Query::test_id("plain")).expect("the anchor needs no handler");
    assert!(robot.click(&plain).is_err(), "a row with no on_row_click has nothing to press");
    assert_eq!(pressed.get(), 2);
    robot.reset();
}
