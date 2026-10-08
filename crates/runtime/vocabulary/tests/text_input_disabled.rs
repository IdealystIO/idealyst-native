//! `text_input(disabled = …)` / `text_area(disabled = …)` — the inert-field
//! contract the idea-ui `Field` / `Textarea` `disabled` props ride on.
//!
//! Mirrors `pressable`'s uniform-disable wiring (`vocab.rs::
//! pressable_disabled_blocks_press_uniformly`): the mount handler
//!
//! 1. gates `on_change` so a disabled field never reports an edit, on every
//!    backend (the gate is the backend-independent half);
//! 2. calls the host's `set_disabled` — the native inert state, which is
//!    what makes the field non-editable AND non-keyboard-focusable on the
//!    real backends (`<input disabled>`, `NSTextField.enabled`,
//!    `UITextField.enabled`, `View.setEnabled`);
//! 3. flips the `DISABLED` state bit so a `state disabled { … }` overlay
//!    (`__state_disabled`) resolves;
//! 4. skips `autofocus` when the field mounts disabled.
//!
//! A live source drives all of it in place — no rebuild.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::{StyleApplication, StyleRules, StyleSheet, Tokenized};
use runtime_scene::realize;
use runtime_vocabulary::builders::{text_area, text_input};
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

/// The author's `on_change` sink.
fn sink() -> (Rc<RefCell<Vec<String>>>, impl Fn(String) + 'static) {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    (got, move |v: String| g.borrow_mut().push(v))
}

#[test]
fn disabled_text_input_is_inert_dimmed_and_drops_edits() {
    let h = harness();
    let (got, on_change) = sink();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().value("frozen").on_change(on_change).style(sheet()).disabled(true).build(),
        )
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(
        ops_named(&log, "set_disabled"),
        vec!["set_disabled n0 true".to_string()],
        "the host is told the field is inert (native: not editable, not focusable)"
    );
    assert_eq!(last_opacity(&log), dimmed(), "the `state disabled` overlay applies");

    // A platform edit event (an in-flight keystroke, a backend with no
    // native inert state) never reaches the author.
    (h.text_input_change(0))("typed".into());
    h.flush();
    assert!(got.borrow().is_empty(), "on_change must not fire while disabled");
    drop(realized);
}

#[test]
fn enabled_text_input_with_disabled_false_reports_edits() {
    let h = harness();
    let (got, on_change) = sink();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input().on_change(on_change).style(sheet()).disabled(false).build(),
        )
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 false".to_string()]);
    assert_ne!(last_opacity(&log), dimmed());
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()]);
    drop(realized);
}

/// No `disabled` at all attaches nothing: no `set_disabled`, and the
/// author's callback is passed through unwrapped.
#[test]
fn text_input_without_disabled_attaches_no_binding() {
    let h = harness();
    let (got, on_change) = sink();
    let realized =
        h.world.enter(|| realize(&h.backend, &h.registry, text_input().on_change(on_change).build()));
    h.flush();
    assert!(ops_named(&h.take_log(), "set_disabled").is_empty());
    (h.text_input_change(0))("x".into());
    assert_eq!(*got.borrow(), vec!["x".to_string()]);
    drop(realized);
}

#[test]
fn live_disabled_text_input_toggles_in_place() {
    let h = harness();
    let world = h.world.clone();
    let (got, on_change) = sink();
    let (realized, locked) = world.enter(|| {
        let locked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            text_input().on_change(on_change).style(sheet()).disabled(move || locked.get()).build(),
        );
        (realized, locked)
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 true".to_string()]);
    assert_eq!(last_opacity(&log), dimmed());
    (h.text_input_change(0))("blocked".into());
    assert!(got.borrow().is_empty());

    world.enter(|| locked.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 false".to_string()]);
    assert_ne!(last_opacity(&log), dimmed(), "the dim clears in place");
    assert!(ops_named(&log, "create").is_empty(), "the field is not rebuilt: {log:?}");
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()], "edits flow once enabled");

    world.enter(|| locked.set(true));
    h.flush();
    assert_eq!(ops_named(&h.take_log(), "set_disabled"), vec!["set_disabled n0 true".to_string()]);
    (h.text_input_change(0))("again".into());
    assert_eq!(got.borrow().len(), 1, "blocked again once re-disabled");
    drop(realized);
}

/// A field that mounts disabled is not focusable, so it must not take the
/// autofocus either — the same answer `<input disabled autofocus>` gives.
#[test]
fn disabled_text_input_skips_autofocus() {
    let h = harness();
    host_mock::take_handle_log();
    let realized = h.world.enter(|| {
        realize(&h.backend, &h.registry, text_input().autofocus(true).disabled(true).build())
    });
    h.flush();
    let focused = host_mock::take_handle_log();
    assert!(
        !focused.iter().any(|l| l.starts_with("focus")),
        "a disabled field must not be focused at mount: {focused:?}"
    );

    // Control: the same field enabled does autofocus.
    let h2 = harness();
    let realized2 = h2.world.enter(|| {
        realize(&h2.backend, &h2.registry, text_input().autofocus(true).disabled(false).build())
    });
    h2.flush();
    assert_eq!(host_mock::take_handle_log(), vec!["focus n0".to_string()], "control: enabled autofocus focuses");
    drop((realized, realized2));
}

#[test]
fn disabled_text_area_is_inert_dimmed_and_drops_edits() {
    let h = harness();
    let world = h.world.clone();
    let (got, on_change) = sink();
    let (realized, locked) = world.enter(|| {
        let locked = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            text_area().on_change(on_change).style(sheet()).disabled(move || locked.get()).build(),
        );
        (realized, locked)
    });
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 true".to_string()]);
    assert_eq!(last_opacity(&log), dimmed());
    (h.text_input_change(0))("blocked".into());
    assert!(got.borrow().is_empty(), "a disabled text_area drops edits");

    world.enter(|| locked.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(ops_named(&log, "set_disabled"), vec!["set_disabled n0 false".to_string()]);
    assert_ne!(last_opacity(&log), dimmed());
    (h.text_input_change(0))("typed".into());
    assert_eq!(*got.borrow(), vec!["typed".to_string()]);
    drop(realized);
}
