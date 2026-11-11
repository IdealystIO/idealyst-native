//! `Button(activation = PressActivation::Down)` — a menu-opening button
//! fires as the pointer goes down. The mode rides the PRESSABLE on both
//! the static build and every live-structure `switch` arm: a `loading` /
//! `disabled` signal rebuilds the pressable, and a rebuilt arm that fell
//! back to release would make the same button open its menu on press-down
//! before a flip and on release after it.

use std::cell::Cell;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Button};
use runtime_core::{signal, ui, Element, PressActivation, Signal};

fn mount<T>(build: impl FnOnce() -> (Element, T)) -> (host_mock::Harness, runtime_scene::Realized<u32>, T) {
    let h = host_mock::Harness::new();
    let (tree, extra) = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized, extra)
}

/// The pressable kinds created since the last drain, in order.
fn pressables_created(h: &host_mock::Harness) -> Vec<String> {
    h.take_log()
        .into_iter()
        .filter(|l| l.starts_with("create") && l.contains("pressable"))
        .map(|l| l.rsplit(' ').next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn button_press_down_mounts_a_press_down_pressable_and_fires_once() {
    let presses = Rc::new(Cell::new(0u32));
    let p = presses.clone();
    let (h, realized, ()) = mount(move || {
        let on_click = Rc::new(move || p.set(p.get() + 1)) as Rc<dyn Fn()>;
        (ui! { Button(label = "Actions", activation = PressActivation::Down, on_click = on_click) }, ())
    });
    assert_eq!(pressables_created(&h), vec!["pressable_on_down"]);
    h.world.enter(|| h.press_labelled("Actions"));
    h.flush();
    assert_eq!(presses.get(), 1);
    drop(realized);
}

#[test]
fn button_defaults_to_release_activation() {
    let (h, realized, ()) = mount(|| (ui! { Button(label = "Save") }, ()));
    assert_eq!(pressables_created(&h), vec!["pressable"]);
    drop(realized);
}

#[test]
fn regression_button_press_down_survives_structural_rebuild() {
    let (h, realized, busy) = mount(|| {
        let busy: Signal<bool> = signal(false);
        (
            ui! { Button(label = "Actions", activation = PressActivation::Down, loading = busy) },
            busy,
        )
    });
    assert_eq!(pressables_created(&h), vec!["pressable_on_down"], "first arm");
    h.world.enter(|| busy.set(true));
    h.flush();
    assert_eq!(pressables_created(&h), vec!["pressable_on_down"], "the rebuilt arm keeps the mode");
    drop(realized);
}
