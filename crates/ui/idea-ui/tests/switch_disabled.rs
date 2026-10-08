//! `Switch(disabled = …)` — Button parity: a disabled switch never fires
//! `on_change`, dims its track (the `dimmed` axis, the same opacity as a
//! disabled Button), and tells the host it is disabled (`set_disabled`,
//! which carries the native / a11y disabled state). Mounted through the real
//! `realize` path against `host-mock`, because the press block is installed
//! by the pressable's mount handler, not by the build tree.

use std::cell::Cell;
use std::rc::Rc;

use idea_theme::extensible::DISABLED_CONTROL_OPACITY;
use idea_ui::{install_idea_theme, light_theme, Switch};
use runtime_core::{rx, signal, ui, Element, Signal, Tokenized};

fn harness() -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.set_style_line(|n, r| format!("apply_style n{n} opacity={:?}", r.opacity));
    h
}

/// The last opacity the host was told to paint the track (the pressable).
fn track_opacity(h: &host_mock::Harness) -> String {
    let ops = h.ops();
    let track = ops
        .iter()
        .find_map(|o| o.strip_prefix("create ").filter(|r| r.ends_with(" pressable")))
        .map(|r| r.trim_end_matches(" pressable").to_string())
        .unwrap_or_else(|| panic!("no track pressable:\n{}", ops.join("\n")));
    let prefix = format!("apply_style {track} ");
    ops.iter()
        .rev()
        .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("track never styled:\n{}", ops.join("\n")))
}

fn dimmed() -> String {
    format!("opacity={:?}", Some(Tokenized::Literal(DISABLED_CONTROL_OPACITY)))
}

/// Mount a Switch built by `build` (given the value signal and a counter
/// the `on_change` bumps), returning the harness and both.
fn mount(
    build: impl FnOnce(Signal<bool>, Rc<dyn Fn(bool)>) -> Element,
) -> (host_mock::Harness, Signal<bool>, Rc<Cell<u32>>, runtime_scene::Realized<u32>) {
    let h = harness();
    h.record_all();
    let fired = Rc::new(Cell::new(0u32));
    let (tree, value) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(false);
        let fired = fired.clone();
        let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v: bool| {
            fired.set(fired.get() + 1);
            value.set(v);
        });
        (build(value, on_change), value)
    });
    let realized = h.mount(tree);
    h.flush();
    (h, value, fired, realized)
}

#[test]
fn a_disabled_switch_does_not_fire_on_change() {
    let (h, value, fired, _r) = mount(|value, on_change| {
        ui! { Switch(value = value, on_change = on_change, disabled = true) }
    });
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 0, "on_change must not fire while disabled");
    assert!(!h.world.enter(|| value.get()), "the value is unchanged");
}

#[test]
fn a_disabled_switch_is_dimmed_and_marked_disabled_for_the_host() {
    let (h, _value, _fired, _r) = mount(|value, on_change| {
        ui! { Switch(value = value, on_change = on_change, disabled = true) }
    });
    assert_eq!(track_opacity(&h), dimmed(), "the track carries the disabled dim");
    let ops = h.ops().join("\n");
    assert!(ops.contains("set_disabled") && ops.contains(" true"), "the host is told the track is disabled:\n{ops}");
}

#[test]
fn an_enabled_switch_toggles_and_is_not_dimmed() {
    let (h, value, fired, _r) = mount(|value, on_change| {
        ui! { Switch(value = value, on_change = on_change) }
    });
    assert_ne!(track_opacity(&h), dimmed(), "an enabled track is not dimmed");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 1, "an enabled switch fires on_change");
    assert!(h.world.enter(|| value.get()), "and flips on");
}

/// A live `disabled` enables/disables in place: the press block and the
/// dim both follow the signal without rebuilding the switch.
#[test]
fn a_live_disabled_follows_its_signal() {
    let locked: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));
    let (h, _value, fired, _r) = {
        let locked = locked.clone();
        mount(move |value, on_change| {
            let lock = signal(true);
            locked.set(Some(lock));
            ui! { Switch(value = value, on_change = on_change, disabled = rx!(lock.get())) }
        })
    };
    let lock = locked.get().unwrap();
    assert_eq!(track_opacity(&h), dimmed(), "starts disabled + dimmed");
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 0, "blocked while the signal is true");

    h.world.enter(|| lock.set(false));
    h.flush();
    assert_ne!(track_opacity(&h), dimmed(), "the dim clears in place");
    // The host is told the track is enabled again — that call is what puts
    // the track back into keyboard focus (web `tabindex`, macOS key-view
    // loop; see `form_controls_disabled.rs`).
    let last = h.ops().iter().rev().find(|o| o.starts_with("set_disabled ")).cloned();
    assert!(
        last.as_deref().is_some_and(|o| o.ends_with(" false")),
        "re-enabling tells the host (back into keyboard focus): {last:?}"
    );
    h.world.enter(|| (h.press_handler(0))());
    h.flush();
    assert_eq!(fired.get(), 1, "and the press goes through once enabled");
}
