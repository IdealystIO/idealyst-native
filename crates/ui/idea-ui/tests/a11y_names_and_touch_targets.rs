//! Glyph-only controls carry accessible names, and the pager steppers and
//! field adornment buttons meet the phone tap-target floor.
//!
//! CrewForge (2026-11-03, idea-ui 16.1), pinned here:
//!
//! - **A2** a label-less `Checkbox` (a table row's tick box) had no
//!   accessible name — the robot could not find it, screen readers read
//!   nothing. Same gap on `Radio`.
//! - **A3** the Calendar's step chevrons were unnamed IconButtons.
//! - **A4** `DateInput`'s clear ✕ and calendar glyph were 18pt pressables
//!   with empty names beside a 44pt field.
//! - **A5** `TablePager` / `Pagination` rendered two unnamed 32×32 ‹ ›
//!   buttons at a 390pt phone width.
//!
//! Names are checked the way a robot suite finds them
//! (`Robot::find(Query::label(…))`). Sizes are checked by laying the
//! mounted tree out for real (`host-mock` + `runtime_layout::LayoutTree`,
//! the harness of `table_pager_rows_picker_layout.rs`) at a phone width
//! (`Xs`) and a desktop width (`Lg`): the floor applies on phones and the
//! compact size is unchanged on desktop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::components::date_input::DateInput;
use idea_ui::date::CivilDate;
use idea_ui::touch::MIN_TOUCH_TARGET_PX;
use idea_ui::{
    install_idea_theme, light_theme, Adornment, Autocomplete, Calendar, Checkbox, ControlSize, Field, Pagination,
    Radio, RadioGroup, RadioOption, SelectOption, TablePager,
};
use runtime_core::{signal, ui, Element, Length, Signal, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};
use runtime_shared::ViewportSize;
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

const PHONE: ViewportSize = ViewportSize { width: 390.0, height: 844.0 };
const DESKTOP: ViewportSize = ViewportSize { width: 1280.0, height: 800.0 };

const GLYPH: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M6 6l12 12"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

// --- accessible names -------------------------------------------------------

/// Mount `build` on a fresh recording host with a clean robot registry.
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

/// The one live pressable named `name`.
fn named(name: &str) -> runtime_vocabulary::robot::Element {
    let robot = Robot::new();
    let all = robot.find_all(Query::label(name).and(Query::kind(ElementKind::Pressable)));
    assert_eq!(all.len(), 1, "exactly one pressable is named {name:?}, got {all:?}");
    all.into_iter().next().unwrap()
}

#[test]
fn regression_label_less_checkbox_and_radio_are_found_by_name() {
    let (_h, _r) = mount(|| {
        let done = signal(false);
        let air = signal(false);
        ui! {
            view {
                Checkbox(
                    value = done,
                    on_change = Rc::new(move |v| done.set(v)) as Rc<dyn Fn(bool)>,
                    size = ControlSize::Sm,
                    accessibility_label = Some("Mark Update ADP done".to_string()),
                )
                Radio(selected = air, accessibility_label = Some("Ship by air".to_string()))
            }
        }
    });
    let tick = named("Mark Update ADP done");
    Robot::new().click(&tick).expect("the named box is clickable");
    named("Ship by air");
}

#[test]
fn regression_calendar_step_chevrons_are_named_per_zoom() {
    let (h, _r) = mount(|| {
        let picked: Signal<Option<CivilDate>> = signal(None);
        ui! { Calendar(value = picked) }
    });
    named("Previous month");
    named("Next month");
    // The title is the pressable between the two chevrons; pressing it
    // zooms out to a year of months, and the chevrons step years.
    let robot = Robot::new();
    let presses = robot.find_all(Query::kind(ElementKind::Pressable));
    robot.click(&presses[1]).expect("the title zooms out");
    h.flush();
    named("Previous year");
    named("Next year");
    assert!(robot.find(Query::label("Previous month")).is_none(), "the month names are gone at the year zoom");
    let presses = robot.find_all(Query::kind(ElementKind::Pressable));
    robot.click(&presses[1]).expect("the title zooms out again");
    h.flush();
    named("Previous 12 years");
    named("Next 12 years");
}

#[test]
fn regression_date_input_adornments_are_named() {
    let (_h, _r) = mount(|| {
        let date: Signal<Option<CivilDate>> = signal(None);
        ui! { DateInput(value = date, clearable = true) }
    });
    named("Clear date");
    named("Pick a date");
}

#[test]
fn regression_pagination_and_table_pager_steppers_are_named() {
    let (_h, _r) = mount(|| {
        let page = signal(3usize);
        ui! { Pagination(page = page, total = 20usize, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>) }
    });
    named("Previous page");
    named("Next page");

    let (_h, _r) = mount(|| {
        ui! {
            TablePager(
                offset = 0,
                page_size = 25,
                total = 312,
                noun = "projects".to_string(),
                on_offset = Rc::new(|_| {}) as Rc<dyn Fn(usize)>,
                previous_label = "Page précédente".to_string(),
                next_label = "Page suivante".to_string(),
            )
        }
    });
    named("Page précédente");
    named("Page suivante");
}

#[test]
fn autocomplete_chevron_and_labeled_field_adornment_are_named() {
    let (_h, _r) = mount(|| {
        let text = signal(String::new());
        ui! {
            view {
                Autocomplete(value = text, options = vec![SelectOption::new("a", "Apple")])
                Field(value = text, trailing = Adornment::labeled_button(GLYPH, "Show password", || {}))
            }
        }
    });
    named("Show options");
    named("Show password");
}

// --- tap targets ------------------------------------------------------------

const CHAR_ADVANCE: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;

/// One-line text measure: `CHAR_ADVANCE` per char, `LINE_HEIGHT` tall.
fn text_measure(content: String) -> runtime_layout::MeasureFn {
    Rc::new(move |known: Size<Option<f32>>, _avail: Size<AvailableSpace>| Size {
        width: known.width.unwrap_or(content.chars().count() as f32 * CHAR_ADVANCE),
        height: known.height.unwrap_or(LINE_HEIGHT),
    })
}

struct Laid {
    h: host_mock::Harness,
    root: u32,
    /// Absolute `(x, y, w, h)` per host-mock node.
    abs: HashMap<u32, (f32, f32, f32, f32)>,
}

impl Laid {
    /// Every pressable, pre-order.
    fn pressables(&self) -> Vec<u32> {
        fn walk(h: &host_mock::Harness, n: u32, out: &mut Vec<u32>) {
            if h.kind_of(n).is_some_and(|k| k.starts_with("pressable")) {
                out.push(n);
            }
            for c in h.children_of(n) {
                walk(h, c, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.h, self.root, &mut out);
        out
    }

    /// The node whose children include `child`.
    fn parent_of(&self, child: u32) -> u32 {
        fn walk(h: &host_mock::Harness, n: u32, child: u32) -> Option<u32> {
            let children = h.children_of(n);
            if children.contains(&child) {
                return Some(n);
            }
            children.into_iter().find_map(|c| walk(h, c, child))
        }
        walk(&self.h, self.root, child).expect("the node is in the tree")
    }

    fn size(&self, n: u32) -> (f32, f32) {
        let (_, _, w, h) = self.abs[&n];
        (w, h)
    }
}

/// Mount `build` at `viewport`, mirror the host tree into a `LayoutTree`
/// with the styles the host received, lay it out in the viewport and
/// return every node's absolute frame.
fn lay_out(viewport: ViewportSize, build: impl FnOnce() -> Element) -> Laid {
    // Seeded before the world exists: its viewport ctx classifies the
    // breakpoint from this value (the native backends' first sample).
    runtime_shared::set_viewport_size(viewport);
    let h = host_mock::Harness::new();
    let styles: Rc<RefCell<HashMap<u32, StyleRules>>> = Rc::default();
    {
        let styles = styles.clone();
        h.set_style_line(move |n, rules| {
            styles.borrow_mut().insert(n, rules.clone());
            format!("apply_style n{n}")
        });
    }
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let _realized = h.mount(tree);
    h.flush();
    let roots = h.live_roots();
    assert_eq!(roots.len(), 1, "one mounted root");
    let root = roots[0];

    let mut layout = LayoutTree::new();
    let mut ids: HashMap<u32, LayoutNode> = HashMap::new();
    fn mirror(
        h: &host_mock::Harness,
        styles: &HashMap<u32, StyleRules>,
        layout: &mut LayoutTree,
        ids: &mut HashMap<u32, LayoutNode>,
        node: u32,
    ) -> LayoutNode {
        let kind = h.kind_of(node).unwrap_or_default();
        let ln = if kind == "anchor" { layout.new_contents_node() } else { layout.new_node() };
        let mut rules = styles.get(&node).cloned().unwrap_or_default();
        if rules.min_width.is_none() {
            rules.min_width = Some(Tokenized::Literal(Length::Auto));
        }
        layout.set_style(ln, &rules);
        if let Some(content) = kind.strip_prefix("text ") {
            layout.set_measure_fn(ln, text_measure(content.trim_matches('"').to_string()));
        }
        ids.insert(node, ln);
        for child in h.children_of(node) {
            let c = mirror(h, styles, layout, ids, child);
            layout.add_child(ln, c);
        }
        ln
    }
    let root_ln = mirror(&h, &styles.borrow(), &mut layout, &mut ids, root);
    layout.compute(root_ln, viewport.width, viewport.height);
    fn absolute(
        h: &host_mock::Harness,
        frames: &HashMap<u32, runtime_layout::Frame>,
        node: u32,
        origin: (f32, f32),
        out: &mut HashMap<u32, (f32, f32, f32, f32)>,
    ) {
        let f = frames[&node];
        let (x, y) = (origin.0 + f.x, origin.1 + f.y);
        out.insert(node, (x, y, f.width, f.height));
        for c in h.children_of(node) {
            absolute(h, frames, c, (x, y), out);
        }
    }
    let frames: HashMap<u32, runtime_layout::Frame> = ids.iter().map(|(n, ln)| (*n, layout.frame_of(*ln))).collect();
    let mut abs = HashMap::new();
    absolute(&h, &frames, root, (0.0, 0.0), &mut abs);
    Laid { h, root, abs }
}

fn pagination() -> Element {
    let page = signal(3usize);
    // Wrapped: the Pagination's root is a reactive anchor (layout-transparent).
    ui! {
        view {
            Pagination(page = page, total = 5usize, on_change = Rc::new(move |n| page.set(n)) as Rc<dyn Fn(usize)>)
        }
    }
}

/// A5: the ‹ › steppers are 44 square on a phone and keep the compact 32
/// square on desktop; the page numbers keep their compact size on both,
/// so a windowed row still fits a phone.
#[test]
fn regression_pagination_steppers_meet_the_phone_tap_target() {
    let phone = lay_out(PHONE, pagination);
    let presses = phone.pressables();
    let (prev, next) = (presses[0], *presses.last().unwrap());
    for n in [prev, next] {
        let (w, h) = phone.size(n);
        assert!(w >= MIN_TOUCH_TARGET_PX && h >= MIN_TOUCH_TARGET_PX, "stepper is {w}×{h} at 390:\n{}", phone.h.tree(phone.root));
    }
    let (_, page_h) = phone.size(presses[1]);
    assert_eq!(page_h, 32.0, "page numbers keep their compact height");

    let desktop = lay_out(DESKTOP, pagination);
    let presses = desktop.pressables();
    for n in [presses[0], *presses.last().unwrap()] {
        assert_eq!(desktop.size(n), (32.0, 32.0), "desktop steppers stay compact");
    }
}

fn adorned_field() -> Element {
    let date: Signal<Option<CivilDate>> = signal(None);
    ui! { DateInput(value = date, clearable = true) }
}

/// A4: the DateInput's ✕ and calendar buttons are 44 square on a phone,
/// side by side without overlapping, and the field box is the same height
/// on a phone as on desktop — the hit area grows, the field does not.
#[test]
fn regression_field_adornment_buttons_meet_the_phone_tap_target_without_growing_the_field() {
    let phone = lay_out(PHONE, adorned_field);
    let buttons = phone.pressables();
    assert_eq!(buttons.len(), 2, "the ✕ and the calendar button:\n{}", phone.h.tree(phone.root));
    for &n in &buttons {
        let (w, h) = phone.size(n);
        assert!(w >= MIN_TOUCH_TARGET_PX && h >= MIN_TOUCH_TARGET_PX, "adornment is {w}×{h} at 390");
    }
    let (x0, _, w0, _) = phone.abs[&buttons[0]];
    let (x1, _, _, _) = phone.abs[&buttons[1]];
    assert!(x0 + w0 <= x1, "the two targets do not overlap ({x0}+{w0} > {x1})");

    let desktop = lay_out(DESKTOP, adorned_field);
    let shell_height = |laid: &Laid, button: u32| {
        // The shell row is the buttons' parent.
        fn parent(h: &host_mock::Harness, n: u32, target: u32) -> Option<u32> {
            let kids = h.children_of(n);
            if kids.contains(&target) {
                return Some(n);
            }
            kids.into_iter().find_map(|c| parent(h, c, target))
        }
        let shell = parent(&laid.h, laid.root, button).expect("the button sits in the shell row");
        laid.size(shell).1
    };
    let desktop_buttons = desktop.pressables();
    let (dw, dh) = desktop.size(desktop_buttons[0]);
    assert!(dw < MIN_TOUCH_TARGET_PX && dh < MIN_TOUCH_TARGET_PX, "desktop keeps the icon-sized button ({dw}×{dh})");
    assert_eq!(
        shell_height(&phone, buttons[0]),
        shell_height(&desktop, desktop_buttons[0]),
        "the field box is the same height on a phone"
    );
}

fn radio_group() -> Element {
    let picked = signal("camera".to_string());
    ui! {
        view {
            RadioGroup(
                value = picked,
                on_change = Rc::new(move |id: String| picked.set(id)) as Rc<dyn Fn(String)>,
                options = vec![
                    RadioOption::new("camera", "Camera"),
                    RadioOption::new("library", "Library"),
                    RadioOption::new("either", "Either"),
                ],
            )
        }
    }
}

/// CrewForge want_49e63639: a RadioGroup's options sat 28 apart at 390, under
/// the 44 phone target. Each labelled row (the node carrying the tap) now meets
/// the floor with no dead strip between rows; the ring stays 20; from `Md` up
/// the row hugs the ring and the group keeps its gap.
#[test]
fn regression_radio_rows_meet_the_phone_tap_target() {
    let phone = lay_out(PHONE, radio_group);
    let rings = phone.pressables();
    assert_eq!(rings.len(), 3, "{}", phone.h.tree(phone.root));
    let row_of = |laid: &Laid, ring: u32| laid.parent_of(ring);
    for &ring in &rings {
        assert_eq!(phone.size(ring), (20.0, 20.0), "the ring is unchanged");
        let (_, row_h) = phone.size(row_of(&phone, ring));
        assert!(row_h >= MIN_TOUCH_TARGET_PX, "row is {row_h} tall at 390:\n{}", phone.h.tree(phone.root));
    }
    let top = |laid: &Laid, n: u32| laid.abs[&n].1;
    let first = row_of(&phone, rings[0]);
    let second = row_of(&phone, rings[1]);
    assert_eq!(top(&phone, first) + phone.size(first).1, top(&phone, second), "phone rows touch: no dead strip");

    let desktop = lay_out(DESKTOP, radio_group);
    let rings = desktop.pressables();
    let first = row_of(&desktop, rings[0]);
    let second = row_of(&desktop, rings[1]);
    assert!(desktop.size(first).1 < MIN_TOUCH_TARGET_PX, "the floor is released at Lg");
    assert!(top(&desktop, second) - (top(&desktop, first) + desktop.size(first).1) > 0.0, "desktop keeps the gap");
    assert_eq!(desktop.size(rings[0]), (20.0, 20.0));
}

fn labelled_checkbox() -> Element {
    let on = signal(false);
    ui! {
        view {
            Checkbox(
                value = on,
                on_change = Rc::new(move |v| on.set(v)) as Rc<dyn Fn(bool)>,
                label = Some("Include archived".to_string()),
            )
        }
    }
}

/// The same floor on a labelled Checkbox row; the box stays 20.
#[test]
fn regression_checkbox_rows_meet_the_phone_tap_target() {
    let phone = lay_out(PHONE, labelled_checkbox);
    let tick = phone.pressables()[0];
    assert_eq!(phone.size(tick), (20.0, 20.0), "the box is unchanged");
    let row = phone.parent_of(tick);
    assert!(phone.size(row).1 >= MIN_TOUCH_TARGET_PX, "row at 390:\n{}", phone.h.tree(phone.root));

    let desktop = lay_out(DESKTOP, labelled_checkbox);
    let tick = desktop.pressables()[0];
    let row = desktop.parent_of(tick);
    assert!(desktop.size(row).1 < MIN_TOUCH_TARGET_PX, "the floor is released at Lg");
}
