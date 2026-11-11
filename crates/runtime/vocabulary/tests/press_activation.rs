//! `pressable(..).activation(..)` — which backend constructor the mount
//! handler picks, and that the framework-side wrapping (press-block for
//! `disabled`) applies to a press-down pressable exactly as to a release one.
//!
//! The pointer half of the contract (fire at the down, swallow the rest of
//! the press) is each backend's, and is tested in each backend crate. What
//! is pinned here is the seam: `PressActivation::Down` must reach
//! `PressableOps::create_pressable_on_down`, and nothing else may change.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::PressActivation;
use runtime_vocabulary::builders::{pressable, view};

fn creates(log: &[String]) -> Vec<String> {
    log.iter().filter(|l| l.starts_with("create")).cloned().collect()
}

#[test]
fn press_down_pressable_mounts_through_create_pressable_on_down() {
    let h = Harness::new();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(pressable(|| {}).activation(PressActivation::Down).build())
                .child(pressable(|| {}).activation(PressActivation::Release).build())
                .child(pressable(|| {}).build())
                .build(),
        )
    });
    let created = creates(&h.take_log());
    assert!(
        created.iter().any(|l| l.ends_with("pressable_on_down")),
        "a Down pressable reaches create_pressable_on_down: {created:?}"
    );
    assert_eq!(
        created.iter().filter(|l| l.ends_with(" pressable")).count(),
        2,
        "explicit Release and the default both stay on create_pressable: {created:?}"
    );
    drop(realized);
}

#[test]
fn press_down_pressable_fires_its_action_once_per_activation() {
    let h = Harness::new();
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(move || f.set(f.get() + 1)).activation(PressActivation::Down).build(),
        )
    });
    (h.press_handler(0))();
    assert_eq!(fired.get(), 1);
    drop(realized);
}

/// The `disabled` press-block is applied by the mount handler around the
/// callback BEFORE the backend sees it, so it must guard the press-down
/// constructor too — otherwise a disabled menu button would still open its
/// menu on press.
#[test]
fn regression_disabled_blocks_a_press_down_pressable() {
    let h = Harness::new();
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            pressable(move || f.set(f.get() + 1))
                .activation(PressActivation::Down)
                .disabled(true)
                .build(),
        )
    });
    (h.press_handler(0))();
    assert_eq!(fired.get(), 0, "a disabled press-down pressable must not fire");
    drop(realized);
}
