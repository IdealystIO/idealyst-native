//! A `Menu` shown with `if open.get() { Menu(…) { … } }` animates out:
//! when `open` turns false the `if` keeps the menu mounted for the exit
//! (`runtime_core::hold_on_exit`, requested by the menu's panel) and
//! removes it when the hold runs out. Before, the branch was torn down in
//! the same turn and the menu could only vanish.
//!
//! The tween itself cannot be watched here (host-mock's frame loop never
//! ticks); what is pinned is the lifetime — still mounted during the hold,
//! gone after it — and that a held menu does not act on a second press.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme, Menu, MenuItem};
use runtime_core::{component, on_scope_drop, signal, ui, AnchorTarget, Element, PressableHandle, Ref, Signal};

/// Marks `gone` when the branch it was built in is finally torn down.
#[component]
fn Probe(#[prop(static)] gone: Option<Rc<Cell<bool>>>) -> Element {
    let gone = gone.expect("probe flag");
    on_scope_drop(move || gone.set(true));
    ui! { view {} }
}

struct Mounted {
    h: Harness,
    _realized: runtime_scene::Realized<host_mock::Node>,
    open: Signal<bool>,
    gone: Rc<Cell<bool>>,
    picks: Rc<Cell<u32>>,
}

fn mount() -> Mounted {
    let h = Harness::new();
    let gone = Rc::new(Cell::new(false));
    let picks = Rc::new(Cell::new(0u32));
    let (tree, open) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let open = signal(true);
        let trigger: Ref<PressableHandle> = Ref::new();
        let gone = gone.clone();
        let picks = picks.clone();
        let on_select: Rc<dyn Fn()> = Rc::new(move || picks.set(picks.get() + 1));
        let tree = ui! {
            view {
                if open.get() {
                    Probe(gone = Some(gone.clone()))
                    Menu(target = Some(AnchorTarget::from(trigger))) {
                        MenuItem(label = "Rename", on_select = on_select.clone())
                    }
                }
            }
        };
        (tree, open)
    });
    let realized = h.mount(tree);
    h.flush();
    Mounted { h, _realized: realized, open, gone, picks }
}

#[test]
fn regression_closing_menu_stays_mounted_for_its_exit_then_leaves() {
    host_mock::pump::install_scheduler();
    let m = mount();

    m.h.world.enter(|| m.open.set(false));
    m.h.flush();
    assert!(!m.gone.get(), "still mounted while the exit plays");

    host_mock::pump::pump_timers();
    m.h.flush();
    assert!(m.gone.get(), "removed when the hold runs out");
}

/// A held menu is still mounted and its rows still take presses. A second
/// click landing on a row while it fades must not select again.
#[test]
fn regression_a_row_pressed_while_its_menu_fades_out_does_not_select() {
    host_mock::pump::install_scheduler();
    let m = mount();
    let press = m.h.pressable_labelled("Rename");

    press();
    assert_eq!(m.picks.get(), 1, "an open menu's row selects");

    m.h.world.enter(|| m.open.set(false));
    m.h.flush();
    press();
    assert_eq!(m.picks.get(), 1, "a leaving menu's row is inert");
    host_mock::pump::pump_timers();
}
