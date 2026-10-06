//! `Radio(disabled = …)` / `RadioGroup(disabled = …)` — Switch/Button
//! parity: a disabled radio never fires `on_select` (a disabled group never
//! fires `on_change`), dims its ring (the `dimmed` axis, the same opacity as
//! a disabled Button), and tells the host it is disabled (`set_disabled`,
//! which carries the native / a11y disabled state). Mounted through the real
//! `realize` path against `host-mock`, because the press block is installed
//! by the pressable's mount handler, not by the build tree. (The label row's
//! tap guard is covered by the unit tests in `components/radio.rs` —
//! host-mock has no touch-dispatch surface.)

use std::cell::Cell;
use std::rc::Rc;

use idea_theme::extensible::DISABLED_CONTROL_OPACITY;
use idea_ui::{install_idea_theme, light_theme, Radio, RadioGroup, RadioOption};
use runtime_core::{rx, signal, ui, Element, Signal, Tokenized};

fn harness() -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.set_style_line(|n, r| format!("apply_style n{n} opacity={:?}", r.opacity));
    h
}

/// Every ring (pressable) node the host created, in creation order.
fn rings(h: &host_mock::Harness) -> Vec<String> {
    h.ops()
        .iter()
        .filter_map(|o| o.strip_prefix("create ").filter(|r| r.ends_with(" pressable")))
        .map(|r| r.trim_end_matches(" pressable").to_string())
        .collect()
}

/// The last opacity the host was told to paint `ring`.
fn opacity_of(h: &host_mock::Harness, ring: &str) -> String {
    let ops = h.ops();
    let prefix = format!("apply_style {ring} ");
    ops.iter()
        .rev()
        .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("ring {ring} never styled:\n{}", ops.join("\n")))
}

/// The last opacity of the first ring.
fn ring_opacity(h: &host_mock::Harness) -> String {
    let ring = rings(h).into_iter().next().unwrap_or_else(|| panic!("no ring pressable"));
    opacity_of(h, &ring)
}

fn dimmed() -> String {
    format!("opacity={:?}", Some(Tokenized::Literal(DISABLED_CONTROL_OPACITY)))
}

fn marked_disabled(h: &host_mock::Harness) -> bool {
    h.ops().iter().any(|l| l.contains("set_disabled") && l.ends_with(" true"))
}

/// Mount a Radio built by `build` (given its `selected` signal and an
/// `on_select` that bumps a counter), returning the harness and both.
fn mount(
    build: impl FnOnce(Signal<bool>, Rc<dyn Fn()>) -> Element,
) -> (host_mock::Harness, Signal<bool>, Rc<Cell<u32>>, runtime_scene::Realized<u32>) {
    let h = harness();
    h.record_all();
    let fired = Rc::new(Cell::new(0u32));
    let (tree, selected) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let selected = signal(false);
        let fired = fired.clone();
        let on_select: Rc<dyn Fn()> = Rc::new(move || {
            fired.set(fired.get() + 1);
            selected.set(true);
        });
        (build(selected, on_select), selected)
    });
    let realized = h.mount(tree);
    h.flush();
    (h, selected, fired, realized)
}

#[test]
fn a_disabled_radio_does_not_fire_on_select() {
    let (h, selected, fired, _r) = mount(|selected, on_select| {
        ui! { Radio(selected = selected, on_select = on_select, disabled = true) }
    });
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 0, "on_select must not fire while disabled");
    assert!(!h.world.enter(|| selected.get()), "the selection is unchanged");
}

#[test]
fn a_disabled_radio_is_dimmed_and_marked_disabled_for_the_host() {
    let (h, _selected, _fired, _r) = mount(|selected, on_select| {
        ui! { Radio(label = Some("Email".into()), selected = selected, on_select = on_select, disabled = true) }
    });
    assert_eq!(ring_opacity(&h), dimmed(), "the ring carries the disabled dim");
    assert!(marked_disabled(&h), "the host is told the ring is disabled:\n{}", h.ops().join("\n"));
}

#[test]
fn an_enabled_radio_selects_and_is_not_dimmed() {
    let (h, selected, fired, _r) = mount(|selected, on_select| {
        ui! { Radio(selected = selected, on_select = on_select) }
    });
    assert_ne!(ring_opacity(&h), dimmed(), "an enabled ring is not dimmed");
    assert!(!marked_disabled(&h), "an enabled ring is never marked disabled for the host");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 1, "an enabled radio fires on_select");
    assert!(h.world.enter(|| selected.get()), "and selects");
}

/// A live `disabled` enables/disables in place: the press block and the
/// dim both follow the signal without rebuilding the radio.
#[test]
fn a_live_disabled_radio_follows_its_signal() {
    let locked: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));
    let (h, _selected, fired, _r) = {
        let locked = locked.clone();
        mount(move |selected, on_select| {
            let lock = signal(true);
            locked.set(Some(lock));
            ui! { Radio(selected = selected, on_select = on_select, disabled = rx!(lock.get())) }
        })
    };
    let lock = locked.get().unwrap();
    assert_eq!(ring_opacity(&h), dimmed(), "starts disabled + dimmed");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 0, "blocked while the signal is true");

    h.world.enter(|| lock.set(false));
    h.flush();
    assert_ne!(ring_opacity(&h), dimmed(), "the dim clears in place");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 1, "and the press goes through once enabled");
}

/// Mount a three-option RadioGroup over `value` (starting at "a"),
/// disabled either statically or through a `lock` signal (starting `true`)
/// when `live`, counting `on_change` calls.
fn mount_group(
    live: bool,
) -> (host_mock::Harness, Signal<String>, Signal<bool>, Rc<Cell<u32>>, runtime_scene::Realized<u32>) {
    let h = harness();
    h.record_all();
    let fired = Rc::new(Cell::new(0u32));
    let (tree, value, lock) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal("a".to_string());
        let lock = signal(true);
        let fired = fired.clone();
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
            fired.set(fired.get() + 1);
            value.set(id);
        });
        let options = vec![
            RadioOption::new("a", "A"),
            RadioOption::new("b", "B"),
            RadioOption::new("c", "C"),
        ];
        let tree = if live {
            ui! { RadioGroup(value = value, on_change = on_change, options = options, disabled = rx!(lock.get())) }
        } else {
            ui! { RadioGroup(value = value, on_change = on_change, options = options, disabled = true) }
        };
        (tree, value, lock)
    });
    let realized = h.mount(tree);
    h.flush();
    (h, value, lock, fired, realized)
}

/// A disabled group disables EVERY option: no option's press reaches
/// `on_change`, and every ring is dimmed + marked disabled.
#[test]
fn a_disabled_radio_group_blocks_and_dims_every_option() {
    let (h, value, _lock, fired, _r) = mount_group(false);
    let rings = rings(&h);
    assert_eq!(rings.len(), 3, "one ring per option");
    for ring in &rings {
        assert_eq!(opacity_of(&h, ring), dimmed(), "ring {ring} carries the disabled dim");
    }
    for i in 0..3 {
        h.world.enter(|| (h.press_handler(i))());
        h.flush();
    }
    assert_eq!(fired.get(), 0, "no option fires on_change while the group is disabled");
    assert_eq!(h.world.enter(|| value.get()), "a", "the selection is unchanged");
    let disabled_marks = h
        .ops()
        .iter()
        .filter(|l| l.contains("set_disabled") && l.ends_with(" true"))
        .count();
    assert!(disabled_marks >= 3, "every ring is marked disabled for the host:\n{}", h.ops().join("\n"));
}

/// A live group `disabled` re-enables every option in place.
#[test]
fn a_live_disabled_radio_group_follows_its_signal() {
    let (h, value, lock, fired, _r) = mount_group(true);
    h.world.enter(|| (h.press_handler(1))());
    h.flush();
    assert_eq!(fired.get(), 0, "blocked while the signal is true");

    h.world.enter(|| lock.set(false));
    h.flush();
    for ring in &rings(&h) {
        assert_ne!(opacity_of(&h, ring), dimmed(), "ring {ring}'s dim clears in place");
    }
    h.world.enter(|| (h.press_handler(1))());
    h.flush();
    assert_eq!(fired.get(), 1, "the press goes through once enabled");
    assert_eq!(h.world.enter(|| value.get()), "b", "and selects that option");
}
