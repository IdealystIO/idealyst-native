//! `toggle(disabled = …)` / `slider(disabled = …)` — the inert-control
//! contract `text_input` already had (`tests/text_input_disabled.rs`).
//!
//! Regression: before this binding the `toggle` / `slider` payloads had no
//! `disabled` input at all, so the vocabulary never called the host's
//! `set_disabled` for them and every backend's native branch
//! (`UISwitch` / `UISlider.enabled`, `NSSwitch` / `NSSlider.enabled`,
//! GTK `set_sensitive` on a `GtkSwitch` / `GtkScale`, Win32 `EnableWindow`,
//! Android `Switch` / `SeekBar.setEnabled`, `<input type=checkbox|range
//! disabled>`) was unreachable. The mount handler now:
//!
//! 1. gates `on_change` so a disabled control never reports a change, on
//!    every backend;
//! 2. calls `set_disabled` — the native inert state (not flippable /
//!    draggable, not keyboard-focusable);
//! 3. flips the `DISABLED` state bit so a `state disabled { … }` overlay
//!    resolves.
//!
//! A live source drives all of it in place — no rebuild.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::{StyleApplication, StyleRules, StyleSheet, Tokenized};
use runtime_macros::ui;
use runtime_scene::realize;
use runtime_vocabulary::builders::{slider, toggle};
use runtime_world::signal;

const DIM: f32 = 0.5;

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h.set_style_line(|n, r| format!("apply_style n{n} opacity={:?}", r.opacity));
    h
}

/// A sheet whose only visible difference is the `disabled` state overlay.
fn sheet() -> StyleApplication {
    StyleApplication::new(Rc::new(
        StyleSheet::new(|_| StyleRules::default()).variant("__state_disabled", "on", |_| StyleRules {
            opacity: Some(Tokenized::Literal(DIM)),
            ..StyleRules::default()
        }),
    ))
}

fn ops_named(log: &[String], op: &str) -> Vec<String> {
    log.iter().filter(|l| l.starts_with(op)).cloned().collect()
}

fn last_opacity(log: &[String]) -> String {
    log.iter()
        .rev()
        .find_map(|l| l.strip_prefix("apply_style n0 ").map(str::to_string))
        .unwrap_or_else(|| panic!("n0 never styled:\n{}", log.join("\n")))
}

fn dimmed() -> String {
    format!("opacity={:?}", Some(Tokenized::Literal(DIM)))
}

fn sink<T: 'static>() -> (Rc<RefCell<Vec<T>>>, impl Fn(T) + 'static) {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    (got, move |v: T| g.borrow_mut().push(v))
}

#[test]
fn regression_toggle_disabled_never_reaches_set_disabled() {
    let h = harness();
    let (got, on_change) = sink::<bool>();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            toggle().value(false).on_change(on_change).style(sheet()).disabled(true).build(),
        )
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "set_disabled"),
        vec!["set_disabled n0 true".to_string()],
        "the host is told the switch is inert (native: not flippable, not focusable)"
    );
    assert_eq!(last_opacity(&log), dimmed(), "the `state disabled` overlay applies");

    // A platform flip (in flight when the switch went inert, or a backend
    // with no native inert state) never reaches the author.
    (h.toggle_change(0))(true);
    h.flush();
    assert!(got.borrow().is_empty(), "on_change must not fire while disabled");
    drop(realized);
}

#[test]
fn live_disabled_toggle_flips_in_place() {
    let h = harness();
    let world = h.world.clone();
    let (got, on_change) = sink::<bool>();
    let (realized, locked) = world.enter(|| {
        let locked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            toggle().on_change(on_change).style(sheet()).disabled(move || locked.get()).build(),
        );
        (realized, locked)
    });
    h.flush();
    assert_eq!(ops_named(&h.take_log(), "set_disabled"), vec!["set_disabled n0 true".to_string()]);
    (h.toggle_change(0))(true);
    assert!(got.borrow().is_empty());

    world.enter(|| locked.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 false".to_string()]);
    assert_ne!(last_opacity(&log), dimmed(), "the dim clears in place");
    assert!(ops_named(&log, "create").is_empty(), "the switch is not rebuilt: {log:?}");
    (h.toggle_change(0))(true);
    assert_eq!(*got.borrow(), vec![true], "flips flow once enabled");
    drop(realized);
}

/// No `disabled` at all attaches nothing: no `set_disabled`, and the
/// author's callback is passed through unwrapped.
#[test]
fn toggle_and_slider_without_disabled_attach_no_binding() {
    let h = harness();
    let (flips, on_flip) = sink::<bool>();
    let (moves, on_move) = sink::<f32>();
    let realized = h.world.enter(|| {
        (
            realize(&h.backend, &h.registry, toggle().on_change(on_flip).build()),
            realize(&h.backend, &h.registry, slider().on_change(on_move).build()),
        )
    });
    h.flush();
    assert!(ops_named(&h.take_log(), "set_disabled").is_empty());
    (h.toggle_change(0))(true);
    (h.slider_change(0))(0.25);
    assert_eq!(*flips.borrow(), vec![true]);
    assert_eq!(*moves.borrow(), vec![0.25]);
    drop(realized);
}

#[test]
fn regression_slider_disabled_never_reaches_set_disabled() {
    let h = harness();
    let world = h.world.clone();
    let (got, on_change) = sink::<f32>();
    let (realized, locked) = world.enter(|| {
        let locked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            slider()
                .range(0.0, 10.0)
                .step(1.0)
                .on_change(on_change)
                .style(sheet())
                .disabled(move || locked.get())
                .build(),
        );
        (realized, locked)
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 true".to_string()]);
    assert_eq!(last_opacity(&log), dimmed());
    (h.slider_change(0))(3.4);
    assert!(got.borrow().is_empty(), "a disabled slider drops drags");

    world.enter(|| locked.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 false".to_string()]);
    assert_ne!(last_opacity(&log), dimmed());
    (h.slider_change(0))(3.4);
    assert_eq!(*got.borrow(), vec![3.0], "enabled again, and still step-snapped");
    drop(realized);
}

/// The inline spelling an author writes — `toggle(disabled = …)` /
/// `slider(disabled = …)` in `ui!` — lowers to the same binding (the macro
/// rejects a prop it has no table entry for, so before this change these
/// were compile errors).
#[test]
fn disabled_spelled_inline_in_ui_reaches_set_disabled() {
    let h = harness();
    let world = h.world.clone();
    let (realized, locked) = world.enter(|| {
        let locked = signal(true);
        let tree: runtime_vocabulary::glue::Element = ui! {
            view() {
                toggle(value = false, on_change = |_| {}, disabled = true)
                slider(value = 0.5f32, on_change = |_| {}, min = 0.0, max = 1.0, disabled = move || locked.get())
            }
        };
        (realize(&h.backend, &h.registry, tree), locked)
    });
    h.flush();
    let mut set = ops_named(&h.take_log(), "set_disabled");
    set.sort();
    assert_eq!(set, vec!["set_disabled n1 true".to_string(), "set_disabled n2 true".to_string()]);
    world.enter(|| locked.set(false));
    h.flush();
    assert_eq!(ops_named(&h.take_log(), "set_disabled"), vec!["set_disabled n2 false".to_string()]);
    drop(realized);
}
