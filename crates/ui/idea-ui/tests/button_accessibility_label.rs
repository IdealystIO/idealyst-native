//! CrewForge report (idea-ui 16.1, A1): a Button's accessible name was
//! always its visible label. The kiosk keypad's backspace key is
//! `Button(label = "⌫", …)`, so VoiceOver/TalkBack announced "⌫" and a
//! robot test had to find it by the glyph.
//!
//! `accessibility_label` names the PRESSABLE — the node assistive tech
//! focuses and the one a robot `label = …` query must resolve to take the
//! press — while the drawn label text stays the glyph. Pinned on both the
//! static build and the live-structure `switch` arms (a `loading` signal
//! rebuilds the pressable; the name must ride every rebuild).

use std::cell::Cell;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Button};
use runtime_core::{signal, ui, Element, Signal};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};

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

/// The robot finds the key as "Backspace" on the pressable that takes the
/// press, and the glyph is still what's drawn.
fn assert_named_backspace(h: &host_mock::Harness, presses: &Cell<u32>) {
    let robot = Robot::new();
    let el = robot
        .find(Query::label("Backspace"))
        .expect("the robot finds the key by its accessible name");
    assert_eq!(el.kind, ElementKind::Pressable, "the name rides the pressable, not the label text");
    assert_eq!(el.label.as_deref(), Some("Backspace"), "the robot reports the override as `label`");

    // A text node's robot label is its content: the glyph is what's drawn.
    let text = |s: &str| Query::label(s).and(Query::kind(ElementKind::Text));
    assert!(robot.find(text("⌫")).is_some(), "the drawn label is still the glyph");
    assert!(robot.find(text("Backspace")).is_none(), "the override is not drawn as text");

    let before = presses.get();
    h.world.enter(|| robot.click(&el)).expect("clicking the named pressable");
    h.flush();
    assert_eq!(presses.get(), before + 1, "the named node is the one that takes the press");
}

#[test]
fn regression_button_accessibility_label_names_the_pressable() {
    let presses = Rc::new(Cell::new(0u32));
    let p = presses.clone();
    let (h, realized, ()) = mount(move || {
        let on_click = Rc::new(move || p.set(p.get() + 1)) as Rc<dyn Fn()>;
        (
            ui! {
                Button(
                    label = "⌫",
                    accessibility_label = Some("Backspace".to_string()),
                    on_click = on_click,
                )
            },
            (),
        )
    });
    assert_named_backspace(&h, &presses);
    drop(realized);
}

#[test]
fn regression_button_accessibility_label_survives_structural_rebuild() {
    let presses = Rc::new(Cell::new(0u32));
    let p = presses.clone();
    let (h, realized, busy) = mount(move || {
        let busy: Signal<bool> = signal(false);
        let on_click = Rc::new(move || p.set(p.get() + 1)) as Rc<dyn Fn()>;
        (
            ui! {
                Button(
                    label = "⌫",
                    accessibility_label = Some("Backspace".to_string()),
                    loading = busy,
                    on_click = on_click,
                )
            },
            busy,
        )
    });
    assert_named_backspace(&h, &presses);
    // Flip `loading` on and off: each flip rebuilds the pressable arm.
    h.world.enter(|| busy.set(true));
    h.flush();
    h.world.enter(|| busy.set(false));
    h.flush();
    assert_named_backspace(&h, &presses);
    drop(realized);
}

#[test]
fn button_without_override_leaves_the_pressable_unnamed() {
    let (_h, realized, ()) = mount(|| (ui! { Button(label = "Save") }, ()));
    let robot = Robot::new();
    let pressables = robot.find_all(Query::kind(ElementKind::Pressable));
    assert_eq!(pressables.len(), 1);
    assert_eq!(pressables[0].label, None, "no override → no pressable name; the label text speaks for itself");
    drop(realized);
}
