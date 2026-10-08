//! `disabled` on `Field`, `Textarea` and `Select` (bug report: only
//! Button / IconButton / Slider — and Switch / Checkbox / Radio — declared
//! one, so a read-only settings panel couldn't freeze its text inputs and
//! dropdowns; `Field`'s sheet even carried a `__state_disabled` arm nothing
//! could turn on).
//!
//! Each control, mounted through the real `realize` path against
//! `host-mock` (the edit gate / press block and the host's `set_disabled`
//! are installed by the primitives' mount handlers, not the build tree):
//!
//! - applies its disabled styling (the dim),
//! - blocks input (`on_change` never fires; the Select never opens),
//! - leaves keyboard focus — `set_disabled(true)` is the host call that
//!   makes the native widget non-focusable (`<input disabled>` /
//!   `tabindex=-1` on web, `NSTextField.enabled` / key-view opt-out on
//!   macOS, `UITextField.enabled` on iOS, `View.setEnabled` on Android;
//!   each backend's own tests pin that half),
//! - and follows a live `disabled` in place.
//!
//! `Switch`'s equivalents live in `switch_disabled.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, Adornment, Field, Select, SelectOption, Textarea,
};
use runtime_core::{rx, signal, ui, Element, IconData, Signal, Tokenized};

const FIELD_DIM: f32 = 0.55;

const GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M6 6l12 12"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

fn harness() -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.record_all();
    h.set_style_line(|n, r| format!("apply_style n{n} opacity={:?}", r.opacity));
    h
}

/// Node ids (`"n3"`) of every created node whose kind starts with `kind`.
fn nodes_of(h: &host_mock::Harness, kind: &str) -> Vec<String> {
    h.ops()
        .iter()
        .filter_map(|o| o.strip_prefix("create "))
        .filter_map(|r| {
            let (id, k) = r.split_once(' ')?;
            k.starts_with(kind).then(|| id.to_string())
        })
        .collect()
}

fn node_of(h: &host_mock::Harness, kind: &str) -> String {
    nodes_of(h, kind)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no `{kind}` node:\n{}", h.ops().join("\n")))
}

/// The last opacity the host was told to paint `node`.
fn opacity(h: &host_mock::Harness, node: &str) -> String {
    let prefix = format!("apply_style {node} ");
    let ops = h.ops();
    ops.iter()
        .rev()
        .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("{node} never styled:\n{}", ops.join("\n")))
}

/// The last `set_disabled` the host got for `node`, if any.
fn host_disabled(h: &host_mock::Harness, node: &str) -> Option<bool> {
    let prefix = format!("set_disabled {node} ");
    h.ops().iter().rev().find_map(|o| o.strip_prefix(&prefix).map(|v| v == "true"))
}

fn dimmed(o: f32) -> String {
    format!("opacity={:?}", Some(Tokenized::Literal(o)))
}

/// Mount `build(value, on_change)`; the `on_change` records every value.
fn mount(
    build: impl FnOnce(Signal<String>, Rc<dyn Fn(String)>) -> Element,
) -> (host_mock::Harness, Rc<RefCell<Vec<String>>>, runtime_scene::Realized<u32>) {
    let h = harness();
    let got = Rc::new(RefCell::new(Vec::new()));
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        let got = got.clone();
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v: String| {
            got.borrow_mut().push(v.clone());
            value.set(v);
        });
        build(value, on_change)
    });
    let realized = h.mount(tree);
    h.flush();
    (h, got, realized)
}

// ---------------------------------------------------------------------------
// Field
// ---------------------------------------------------------------------------

#[test]
fn a_disabled_field_is_dimmed_inert_and_out_of_focus() {
    let (h, got, _r) = mount(|value, on_change| {
        ui! { Field(label = Some("Name".into()), value = value, on_change = on_change, disabled = true) }
    });
    let input = node_of(&h, "text_input");
    assert_eq!(host_disabled(&h, &input), Some(true), "the host makes the input inert + unfocusable");
    assert_eq!(opacity(&h, &input), dimmed(FIELD_DIM), "the sheet's `state disabled` dim applies");
    (h.text_input_change(0))("typed".into());
    h.flush();
    assert!(got.borrow().is_empty(), "on_change must not fire while disabled");
}

#[test]
fn an_enabled_field_attaches_no_disabled_binding() {
    let (h, got, _r) = mount(|value, on_change| {
        ui! { Field(value = value, on_change = on_change) }
    });
    let input = node_of(&h, "text_input");
    assert_eq!(host_disabled(&h, &input), None, "Static(false) attaches nothing");
    assert_ne!(opacity(&h, &input), dimmed(FIELD_DIM));
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()]);
}

#[test]
fn a_live_disabled_field_follows_its_signal() {
    let lock_out: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));
    let (h, got, _r) = {
        let lock_out = lock_out.clone();
        mount(move |value, on_change| {
            let lock = signal(true);
            lock_out.set(Some(lock));
            ui! { Field(value = value, on_change = on_change, disabled = rx!(lock.get())) }
        })
    };
    let lock = lock_out.get().unwrap();
    let input = node_of(&h, "text_input");
    assert_eq!(host_disabled(&h, &input), Some(true));
    assert_eq!(opacity(&h, &input), dimmed(FIELD_DIM));
    (h.text_input_change(0))("blocked".into());
    assert!(got.borrow().is_empty());

    h.world.enter(|| lock.set(false));
    h.flush();
    assert_eq!(host_disabled(&h, &input), Some(false), "re-enabled in place");
    assert_ne!(opacity(&h, &input), dimmed(FIELD_DIM), "the dim clears");
    assert_eq!(nodes_of(&h, "text_input").len(), 1, "the input is never rebuilt");
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()]);
}

/// An adorned field dims its whole box (the row shell), not just the
/// input — and the input inside does NOT dim a second time (opacity
/// multiplies down the tree: 0.55 × 0.55 would leave the text fainter than
/// its own border). Its `Adornment::Button` is disabled with it, so a
/// read-only field can't be cleared through its ✕.
#[test]
fn a_disabled_adorned_field_dims_its_shell_once_and_disables_its_buttons() {
    let cleared = Rc::new(Cell::new(0u32));
    let (h, got, _r) = {
        let cleared = cleared.clone();
        mount(move |value, on_change| {
            let clear = Adornment::button(GLYPH, move || cleared.set(cleared.get() + 1));
            ui! {
                Field(
                    value = value,
                    on_change = on_change,
                    leading = Adornment::Icon(GLYPH),
                    trailing = clear,
                    disabled = true,
                )
            }
        })
    };
    let input = node_of(&h, "text_input");
    // The shell is the view created right before the input's siblings: the
    // first view whose style carries the dim.
    let views = nodes_of(&h, "view");
    let shell = views
        .iter()
        .find(|n| {
            let prefix = format!("apply_style {n} ");
            h.ops().iter().any(|o| o.strip_prefix(&prefix) == Some(dimmed(FIELD_DIM).as_str()))
        })
        .unwrap_or_else(|| panic!("no dimmed shell view:\n{}", h.ops().join("\n")));
    assert_eq!(opacity(&h, shell), dimmed(FIELD_DIM), "the adorned box dims");
    assert_eq!(
        opacity(&h, &input),
        dimmed(1.0),
        "the input inside the dimmed shell is not dimmed again"
    );
    assert_eq!(host_disabled(&h, &input), Some(true));

    let button = node_of(&h, "pressable");
    assert_eq!(host_disabled(&h, &button), Some(true), "the ✕ leaves keyboard focus too");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(cleared.get(), 0, "the adornment button's press is blocked");
    (h.text_input_change(0))("typed".into());
    assert!(got.borrow().is_empty());
}

// ---------------------------------------------------------------------------
// Textarea
// ---------------------------------------------------------------------------

#[test]
fn a_disabled_textarea_is_dimmed_inert_and_out_of_focus() {
    let (h, got, _r) = mount(|value, on_change| {
        ui! { Textarea(label = Some("Notes".into()), value = value, on_change = on_change, disabled = true) }
    });
    let area = node_of(&h, "text_area");
    assert_eq!(host_disabled(&h, &area), Some(true));
    assert_eq!(opacity(&h, &area), dimmed(FIELD_DIM));
    (h.text_input_change(0))("typed".into());
    h.flush();
    assert!(got.borrow().is_empty(), "on_change must not fire while disabled");
}

#[test]
fn a_live_disabled_textarea_follows_its_signal() {
    let lock_out: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));
    let (h, got, _r) = {
        let lock_out = lock_out.clone();
        mount(move |value, on_change| {
            let lock = signal(true);
            lock_out.set(Some(lock));
            ui! { Textarea(value = value, on_change = on_change, disabled = rx!(lock.get())) }
        })
    };
    let lock = lock_out.get().unwrap();
    let area = node_of(&h, "text_area");
    assert_eq!(host_disabled(&h, &area), Some(true));
    (h.text_input_change(0))("blocked".into());
    assert!(got.borrow().is_empty());

    h.world.enter(|| lock.set(false));
    h.flush();
    assert_eq!(host_disabled(&h, &area), Some(false));
    assert_ne!(opacity(&h, &area), dimmed(FIELD_DIM));
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()]);
}

// ---------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------

fn options() -> Vec<SelectOption> {
    vec![SelectOption::new("a", "Apple"), SelectOption::new("b", "Pear")]
}

#[test]
fn a_disabled_select_does_not_open_and_is_out_of_focus() {
    let (h, got, _r) = mount(|value, on_change| {
        ui! { Select(value = value, on_change = on_change, options = options(), disabled = true) }
    });
    let trigger = node_of(&h, "pressable");
    assert_eq!(host_disabled(&h, &trigger), Some(true), "the trigger leaves keyboard focus");
    assert_eq!(opacity(&h, &trigger), dimmed(FIELD_DIM), "the trigger's `state disabled` dim");

    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert!(nodes_of(&h, "portal").is_empty(), "a disabled Select never opens its menu");
    assert!(got.borrow().is_empty());
}

#[test]
fn an_enabled_select_opens() {
    let (h, _got, _r) = mount(|value, on_change| {
        ui! { Select(value = value, on_change = on_change, options = options()) }
    });
    let trigger = node_of(&h, "pressable");
    assert_eq!(host_disabled(&h, &trigger), None, "Static(false) attaches nothing");
    assert_ne!(opacity(&h, &trigger), dimmed(FIELD_DIM));
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert!(!nodes_of(&h, "portal").is_empty(), "control: the menu opens");
}

/// A Select that goes disabled while its menu is open closes the menu —
/// the press block only stops new opens — and re-enables in place.
#[test]
fn a_live_disabled_select_closes_its_open_menu_and_reenables() {
    let lock_out: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));
    let (h, got, _r) = {
        let lock_out = lock_out.clone();
        mount(move |value, on_change| {
            let lock = signal(false);
            lock_out.set(Some(lock));
            ui! {
                Select(value = value, on_change = on_change, options = options(), disabled = rx!(lock.get()))
            }
        })
    };
    let lock = lock_out.get().unwrap();
    let trigger = node_of(&h, "pressable");
    assert_eq!(host_disabled(&h, &trigger), Some(false));

    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    let portals = nodes_of(&h, "portal");
    assert!(!portals.is_empty(), "the menu opens while enabled");

    h.world.enter(|| lock.set(true));
    h.flush();
    assert_eq!(host_disabled(&h, &trigger), Some(true));
    for p in &portals {
        assert!(
            h.ops().iter().any(|o| o == &format!("release_portal {p}")),
            "going disabled closes the open menu ({p}):\n{}",
            h.ops().join("\n")
        );
    }
    let opened = nodes_of(&h, "portal").len();
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(nodes_of(&h, "portal").len(), opened, "blocked while disabled");

    h.world.enter(|| lock.set(false));
    h.flush();
    assert_eq!(host_disabled(&h, &trigger), Some(false));
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert!(nodes_of(&h, "portal").len() > opened, "opens again once re-enabled");
    assert!(got.borrow().is_empty());
}
