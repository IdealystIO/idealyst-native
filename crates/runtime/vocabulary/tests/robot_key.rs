//! The robot's `key` verb (`Robot::press_key`): a key goes through the
//! backend's real key path when it has one (`IntrospectionOps::native_key`),
//! and otherwise through the framework's own key paths — the focused
//! field's `on_key_down` and text edit, then the app-level `on_key`
//! listeners — so Enter submits, Backspace deletes and Escape reaches the
//! app on every platform.
//!
//! host-mock stands in for the toolkit: `native_keys` receives the injected
//! press, `shortcut_is_meta` is the platform's shortcut modifier.
#![cfg(feature = "robot")]

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{from_str, json, Value};
use runtime_shared::key_input;
use runtime_shared::primitives::key::{AppKeyEvent, KeyEvent, KeyOutcome, KeyPhase};
use runtime_vocabulary::builders::{overlay, text_area, text_input, view};
use runtime_vocabulary::caps::{KeyChord, KeyPress};
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{KeyMechanism, Query, Robot, RobotError, StateValue};

fn find(robot: &Robot, id: &str) -> runtime_vocabulary::robot::Element {
    robot.find(Query::test_id(id)).unwrap_or_else(|| panic!("{id} registered"))
}

fn text(robot: &Robot, id: &str) -> String {
    match robot.state(&find(robot, id)).unwrap().unwrap().value {
        Some(StateValue::Text(t)) => t,
        other => panic!("{id} has no text: {other:?}"),
    }
}

fn verb(args: Value) -> Result<Value, String> {
    invoke_command("key", &args).map(|out| from_str(&out).unwrap())
}

/// A login form: an email field whose `on_key_down` submits on Enter, and
/// a notes area.
fn form(h: &Harness, submits: Rc<RefCell<Vec<String>>>) -> runtime_scene::Realized<host_mock::Node> {
    h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(
                    text_input()
                        .test_id("email")
                        .value("")
                        .on_change(|_| {})
                        .on_key_down(move |e: &KeyEvent| {
                            if e.key == "Enter" {
                                submits.borrow_mut().push("submit".into());
                                return KeyOutcome::PreventDefault;
                            }
                            if e.key == "x" {
                                // A field that refuses one character.
                                return KeyOutcome::PreventDefault;
                            }
                            KeyOutcome::Default
                        })
                        .build(),
                )
                .child(text_area().test_id("notes").value("").on_change(|_| {}).build())
                .build(),
        )
    })
}

/// A backend with key injection gets the resolved press, run with the
/// backend NOT borrowed (delivering a key runs author code that may borrow
/// it), and the verb reports `native`. Nothing is dispatched by the robot
/// itself: the key is the toolkit's to deliver.
#[test]
fn a_native_key_is_injected_with_the_backend_free() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let submits = Rc::new(RefCell::new(Vec::new()));
    let _r = form(&h, submits.clone());
    let got: Rc<RefCell<Vec<KeyPress>>> = Rc::default();
    {
        let (got, backend) = (got.clone(), h.backend.clone());
        *h.shared.native_keys.borrow_mut() = Some(Rc::new(move |press: &KeyPress| {
            let _free = backend.try_borrow_mut().map_err(|_| "the backend is still borrowed".to_string())?;
            got.borrow_mut().push(press.clone());
            Ok(())
        }));
    }

    let out = verb(json!({ "key": "Meta+K" })).unwrap();
    assert_eq!(out, json!({ "mechanism": "native" }));
    let enter = verb(json!({ "key": "Enter", "element_id": find(&robot, "email").id.0 })).unwrap();
    assert_eq!(enter, json!({ "mechanism": "native" }));
    let got = got.borrow();
    assert_eq!(
        got[0],
        KeyPress { key: "k".into(), code: "KeyK".into(), shift: false, ctrl: false, alt: false, meta: true }
    );
    assert_eq!(got[1].key, "Enter");
    assert!(submits.borrow().is_empty(), "the toolkit delivers a native key, not the robot");

    // The platform refusing the key is the verb's error.
    *h.shared.native_keys.borrow_mut() = Some(Rc::new(|_: &KeyPress| Err("no key window".to_string())));
    assert_eq!(
        robot.press_key(None, &KeyChord::parse("Escape").unwrap()),
        Err(RobotError::Native("no key window".into()))
    );
    robot.reset();
}

/// `ControlOrMeta` resolves with the backend's shortcut modifier: Command
/// where the platform's shortcuts use it, Control elsewhere.
#[test]
fn control_or_meta_follows_the_backend_shortcut_modifier() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let _r = form(&h, Rc::default());
    let got: Rc<RefCell<Vec<KeyPress>>> = Rc::default();
    {
        let got = got.clone();
        *h.shared.native_keys.borrow_mut() = Some(Rc::new(move |press: &KeyPress| {
            got.borrow_mut().push(press.clone());
            Ok(())
        }));
    }
    verb(json!({ "key": "a", "modifiers": ["ControlOrMeta"] })).unwrap();
    h.shared.shortcut_is_meta.set(true);
    verb(json!({ "key": "ControlOrMeta+a" })).unwrap();
    let got = got.borrow();
    assert!(got[0].ctrl && !got[0].meta, "Control off Apple: {:?}", got[0]);
    assert!(got[1].meta && !got[1].ctrl, "Command on Apple: {:?}", got[1]);
    robot.reset();
}

/// Without key injection the press runs the framework's key paths: the
/// focused field's `on_key_down` (Enter submits), its text edit (typed
/// characters, Backspace), and the app-level listeners, which see the down
/// and the up. The verb reports `dispatch`.
#[test]
fn without_native_keys_the_press_reaches_the_field_and_the_app() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let submits = Rc::new(RefCell::new(Vec::new()));
    let _r = form(&h, submits.clone());
    let app_keys: Rc<RefCell<Vec<AppKeyEvent>>> = Rc::default();
    let _listener = {
        let app_keys = app_keys.clone();
        key_input::add_listener(move |e| {
            app_keys.borrow_mut().push(e.clone());
            KeyOutcome::Default
        })
    };
    let email = find(&robot, "email");
    for key in ["h", "i", "!"] {
        assert_eq!(robot.press_key(Some(&email), &KeyChord::parse(key).unwrap()), Ok(KeyMechanism::Dispatch));
    }
    assert_eq!(text(&robot, "email"), "hi!");
    robot.press_key(Some(&email), &KeyChord::parse("Backspace").unwrap()).unwrap();
    assert_eq!(text(&robot, "email"), "hi", "Backspace deletes");
    robot.press_key(Some(&email), &KeyChord::parse("x").unwrap()).unwrap();
    assert_eq!(text(&robot, "email"), "hi", "on_key_down's PreventDefault stops the edit");

    let out = verb(json!({ "key": "Enter", "element_id": email.id.0 })).unwrap();
    assert_eq!(out, json!({ "mechanism": "dispatch" }));
    assert_eq!(submits.borrow().as_slice(), ["submit"], "Enter reaches the field's on_key_down");
    assert_eq!(text(&robot, "email"), "hi", "Enter types nothing in a single-line field");

    // The app hears every press: down then up, with the code and modifiers.
    let keys = app_keys.borrow();
    let downs: Vec<&str> = keys.iter().filter(|e| e.phase == KeyPhase::Down).map(|e| e.key.as_str()).collect();
    assert_eq!(downs, ["h", "i", "!", "Backspace", "x", "Enter"]);
    assert_eq!(keys.iter().filter(|e| e.phase == KeyPhase::Up).count(), 6);
    let bang = keys.iter().find(|e| e.key == "!").unwrap();
    assert_eq!((bang.code.as_str(), bang.shift), ("Digit1", true));
    assert!(!key_input::is_key_down("Enter"), "every press is released");
    drop(keys);

    // Enter types a newline in a text area.
    let notes = find(&robot, "notes");
    robot.press_key(Some(&notes), &KeyChord::parse("a").unwrap()).unwrap();
    robot.press_key(Some(&notes), &KeyChord::parse("Enter").unwrap()).unwrap();
    assert_eq!(text(&robot, "notes"), "a\n");
    robot.reset();
}

/// A window-level press (no element) reaches the app's listeners, and the
/// field that reports focus gets it too; an app listener's PreventDefault
/// stops the field's edit. This is how Escape closes a search overlay
/// whose app listens for it, and how ⌘K opens one.
#[test]
fn a_window_press_reaches_the_app_and_the_focused_field() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let _r = form(&h, Rc::default());
    let closed = Rc::new(RefCell::new(0));
    let _listener = {
        let closed = closed.clone();
        key_input::add_listener(move |e| {
            if e.is_press() && e.key == "Escape" {
                *closed.borrow_mut() += 1;
            }
            if e.key == "k" && e.meta {
                return KeyOutcome::PreventDefault;
            }
            KeyOutcome::Default
        })
    };
    assert_eq!(verb(json!({ "key": "Escape" })).unwrap(), json!({ "mechanism": "dispatch" }));
    assert_eq!(*closed.borrow(), 1);

    // The field's notifier reports focus, as a native field would.
    let notifier = h.shared.focus_handlers.borrow()[0].1.clone();
    notifier(true);
    verb(json!({ "key": "q" })).unwrap();
    assert_eq!(text(&robot, "email"), "q", "the focused field takes the key");
    verb(json!({ "key": "Meta+K" })).unwrap();
    assert_eq!(text(&robot, "email"), "q", "a shortcut types nothing");
    robot.reset();
}

/// Bad keys are the verb's error, before anything is pressed; a disabled
/// field refuses focus, as it refuses a user.
#[test]
fn bad_keys_and_disabled_fields_are_refused() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().child(text_input().test_id("off").value("").disabled(true).build()).build(),
        )
    });
    assert!(verb(json!({ "key": "Return" })).unwrap_err().contains("unknown key 'Return'"));
    assert!(verb(json!({ "key": "Hyper+a" })).unwrap_err().contains("unknown modifier"));
    assert!(verb(json!({})).unwrap_err().contains("missing 'key'"));
    assert!(verb(json!({ "key": "a", "modifiers": "Shift" })).unwrap_err().contains("array"));
    assert_eq!(
        robot.press_key(Some(&find(&robot, "off")), &KeyChord::parse("a").unwrap()),
        Err(RobotError::Disabled("key"))
    );
    assert_eq!(text(&robot, "off"), "");
    robot.reset();
}

/// Two overlays open, one opened over the other — the shape of a confirm
/// dialog raised from a modal. Each closes through its `on_dismiss`; the
/// lower one holds a field.
struct Stacked {
    lower: runtime_world::Signal<bool>,
    upper: runtime_world::Signal<bool>,
    _root: runtime_scene::Realized<host_mock::Node>,
}

fn stacked(h: &Harness, field_key_down: impl Fn(&KeyEvent) -> KeyOutcome + 'static) -> Stacked {
    let (lower, upper) = h.world.enter(|| (runtime_world::signal(true), runtime_world::signal(true)));
    let field_key_down = Rc::new(field_key_down);
    let root = h.mount(
        view()
            .child(runtime_scene::dyn_keyed(
                move || lower.get(),
                move |&open| {
                    let field_key_down = field_key_down.clone();
                    if open {
                        overlay()
                            .on_dismiss(move || lower.set(false))
                            .child(
                                text_input()
                                    .test_id("lower-field")
                                    .value("")
                                    .on_change(|_| {})
                                    .on_key_down(move |e: &KeyEvent| field_key_down(e))
                                    .build(),
                            )
                            .build()
                    } else {
                        view().build()
                    }
                },
            ))
            .child(runtime_scene::dyn_keyed(
                move || upper.get(),
                move |&open| {
                    if open {
                        overlay().on_dismiss(move || upper.set(false)).child(view().test_id("upper")).build()
                    } else {
                        view().build()
                    }
                },
            ))
            .build(),
    );
    Stacked { lower, upper, _root: root }
}

/// REGRESSION (iOS robot E2E, `escape_closes_an_overlay_with_on_dismiss`):
/// on the dispatch mechanism (iOS, Linux) Escape reached the app's
/// listeners and stopped there, so an overlay with an `on_dismiss` stayed
/// open — every native backend closes it. An Escape nothing consumed now
/// closes the topmost overlay, and only that one; the next Escape closes
/// the one beneath.
#[test]
fn regression_dispatch_escape_dismisses_only_the_topmost_overlay() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let s = stacked(&h, |_| KeyOutcome::Default);
    let heard = Rc::new(RefCell::new(0));
    let _listener = {
        let heard = heard.clone();
        key_input::add_listener(move |e| {
            if e.is_press() && e.key == "Escape" {
                *heard.borrow_mut() += 1;
            }
            KeyOutcome::Default
        })
    };

    assert_eq!(verb(json!({ "key": "Escape" })).unwrap(), json!({ "mechanism": "dispatch" }));
    h.flush();
    assert!(!h.world.enter(|| s.upper.get()), "Escape closes the topmost overlay");
    assert!(h.world.enter(|| s.lower.get()), "and leaves the one beneath open");
    assert_eq!(*heard.borrow(), 1, "the app hears the Escape too");

    verb(json!({ "key": "Escape" })).unwrap();
    h.flush();
    assert!(!h.world.enter(|| s.lower.get()), "the next Escape closes the one beneath");
    assert!(!runtime_vocabulary::escape_dismiss::has_dismissable(), "closed overlays leave the stack");
    // Nothing left to close: Escape is only the app's.
    verb(json!({ "key": "Escape" })).unwrap();
    assert_eq!(*heard.borrow(), 3);
    robot.reset();
}

/// REGRESSION (iOS robot E2E, `escape_closes_a_modal_while_its_field_has_
/// focus`): Escape with a field inside the modal focused goes to the field
/// first and, unconsumed, closes the modal. A field or app listener that
/// prevents it keeps the overlay open, as on the native backends.
#[test]
fn regression_dispatch_escape_from_a_focused_field_closes_its_modal_unless_consumed() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let field_takes_escape = Rc::new(std::cell::Cell::new(true));
    let s = {
        let takes = field_takes_escape.clone();
        stacked(&h, move |e| {
            if e.key == "Escape" && takes.get() {
                KeyOutcome::PreventDefault
            } else {
                KeyOutcome::Default
            }
        })
    };
    h.world.enter(|| s.upper.set(false));
    h.flush();
    let field = find(&robot, "lower-field");
    robot.press_key(Some(&field), &KeyChord::parse("a").unwrap()).unwrap();
    assert_eq!(text(&robot, "lower-field"), "a");

    // The field's on_key_down takes it: nothing closes.
    robot.press_key(Some(&field), &KeyChord::parse("Escape").unwrap()).unwrap();
    h.flush();
    assert!(h.world.enter(|| s.lower.get()), "a field that consumes Escape keeps its modal open");

    // An app listener that takes it: nothing closes.
    field_takes_escape.set(false);
    let app_takes = key_input::add_listener(|e| {
        if e.key == "Escape" {
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    });
    robot.press_key(Some(&field), &KeyChord::parse("Escape").unwrap()).unwrap();
    h.flush();
    assert!(h.world.enter(|| s.lower.get()), "an app listener that consumes Escape keeps the modal open");
    drop(app_takes);

    robot.press_key(Some(&field), &KeyChord::parse("Escape").unwrap()).unwrap();
    h.flush();
    assert!(!h.world.enter(|| s.lower.get()), "an unconsumed Escape closes the modal");
    robot.reset();
}

/// The dispatch half of iOS's Backspace bug (robot E2E
/// `pressed_keys_edit_a_field_and_enter_submits_it` read "Hix" after a
/// window-level Backspace): a window press edits only a field whose focus
/// notifier reported focus. iOS installed no `text_input` focus notifier,
/// so no field ever did; the failing-before test for that is
/// `backend-ios-mobile`'s simulator test `text_input_focus`.
#[test]
fn a_window_backspace_edits_only_the_field_that_reports_focus() {
    let robot = Robot::new();
    robot.reset();
    key_input::reset_for_tests();
    let h = Harness::new();
    let _r = form(&h, Rc::default());
    let email = find(&robot, "email");
    for key in ["H", "i", "y"] {
        robot.press_key(Some(&email), &KeyChord::parse(key).unwrap()).unwrap();
    }
    assert_eq!(text(&robot, "email"), "Hiy");
    // Without a focus report there is no field to edit.
    verb(json!({ "key": "Backspace" })).unwrap();
    assert_eq!(text(&robot, "email"), "Hiy", "no field reports focus");
    (h.shared.focus_handlers.borrow()[0].1.clone())(true);
    verb(json!({ "key": "Backspace" })).unwrap();
    assert_eq!(text(&robot, "email"), "Hi", "Backspace edits the focused field");
    robot.reset();
}
