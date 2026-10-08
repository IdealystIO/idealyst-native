//! Bug: a `Select(disabled = true)` still showed its hover border when the
//! pointer rested on it. The trigger sheet's `state disabled` only dims
//! (`opacity`), so it never overrode `state hovered`'s `border_color` —
//! and the resolver merged states alphabetically, putting `hovered` above
//! `disabled` anyway. The framework rule now (`StateBits::PRECEDENCE`):
//! while DISABLED is on, the interaction states (hovered / pressed /
//! focused) take no part in style resolution.
//!
//! Mounted through the real `realize` path against `host-mock` (an
//! event-driven host, like every native backend): the trigger's
//! `attach_states` setter is the one a native pointer-enter would call.
//! The web half (`:hover:not([disabled])`) is pinned in `css`'s
//! `regression_disabled_state_overrides_hovered_in_css`.

use idea_ui::{install_idea_theme, light_theme, Select, SelectOption};
use runtime_core::{rx, signal, ui, Element, Signal, StateBits};

fn harness() -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.record_all();
    h.set_style_line(|n, r| {
        format!(
            "apply_style n{n} border={:?} opacity={:?}",
            r.border_left_color.as_ref().and_then(|c| c.name()),
            r.opacity
        )
    });
    h
}

fn options() -> Vec<SelectOption> {
    vec![SelectOption::new("a", "Apple"), SelectOption::new("b", "Pear")]
}

/// Mount a Select whose `disabled` follows a live signal.
fn mount(disabled: bool) -> (host_mock::Harness, Signal<bool>, runtime_scene::Realized<u32>) {
    let h = harness();
    let mut live = None;
    let tree: Element = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        let off = signal(disabled);
        live = Some(off);
        ui! { Select(value = value, options = options(), disabled = rx!(off.get())) }
    });
    let realized = h.mount(tree);
    h.flush();
    (h, live.expect("signal created"), realized)
}

/// The trigger pressable's node id (`"n3"`).
fn trigger(h: &host_mock::Harness) -> String {
    h.ops()
        .iter()
        .filter_map(|o| o.strip_prefix("create "))
        .find_map(|r| {
            let (id, k) = r.split_once(' ')?;
            k.starts_with("pressable").then(|| id.to_string())
        })
        .unwrap_or_else(|| panic!("no trigger pressable:\n{}", h.ops().join("\n")))
}

/// Flip one interaction-state bit on `node`, as its native host would.
fn set_state(h: &host_mock::Harness, node: &str, bit: StateBits, on: bool) {
    let idx = h
        .ops()
        .iter()
        .filter(|o| o.starts_with("attach_states "))
        .position(|o| o == &format!("attach_states {node}"))
        .unwrap_or_else(|| panic!("{node} attached no state setter:\n{}", h.ops().join("\n")));
    let setter = h.state_setter(idx);
    h.world.enter(|| setter(bit, on));
    h.flush();
}

/// The last `border=… opacity=…` the host painted `node` with.
fn paint(h: &host_mock::Harness, node: &str) -> String {
    let prefix = format!("apply_style {node} ");
    let ops = h.ops();
    ops.iter()
        .rev()
        .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("{node} never styled:\n{}", ops.join("\n")))
}

fn border(p: &str) -> &str {
    p.split(" opacity=").next().unwrap_or(p)
}

#[test]
fn regression_disabled_select_shows_no_hover_border() {
    let (h, _off, _r) = mount(true);
    let t = trigger(&h);
    let resting = paint(&h, &t);
    assert!(resting.contains("opacity=Some(Literal(0.55))"), "disabled dim at rest: {resting}");

    set_state(&h, &t, StateBits::HOVERED, true);
    let hovered = paint(&h, &t);
    assert_eq!(border(&hovered), border(&resting), "a disabled Select must not paint its hover border");
    assert!(hovered.contains("opacity=Some(Literal(0.55))"), "still dimmed while hovered: {hovered}");
}

/// Control + live re-enable: an enabled Select hovers, and a Select that
/// goes disabled under a resting pointer drops the hover border, then gets
/// it straight back when re-enabled.
#[test]
fn a_select_disabled_under_the_pointer_drops_and_regains_its_hover_border() {
    let (h, off, _r) = mount(false);
    let t = trigger(&h);
    let resting = paint(&h, &t);

    set_state(&h, &t, StateBits::HOVERED, true);
    let hovered = paint(&h, &t);
    assert_eq!(border(&hovered), "border=Some(\"color-border-hover\")", "control: an enabled Select hovers");
    assert_ne!(border(&hovered), border(&resting));

    h.world.enter(|| off.set(true));
    h.flush();
    assert_eq!(border(&paint(&h, &t)), border(&resting), "going disabled drops the hover border");

    h.world.enter(|| off.set(false));
    h.flush();
    assert_eq!(border(&paint(&h, &t)), border(&hovered), "re-enabled under the pointer, the hover returns");
}
