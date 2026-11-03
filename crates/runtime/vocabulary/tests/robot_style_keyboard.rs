//! The robot reports a text field's keyboard kind and an element's resolved
//! style (CrewForge 2026-11-03, B6 / B7): a test could not check that an
//! email field shows the email keyboard, or that a completed to-do is struck
//! through, without the app exposing a signal for it.
//!
//! host-mock stands in for the toolkit: `keyboard_kinds` is what the native
//! widget holds, `computed_styles` a toolkit that resolves style itself.
#![cfg(feature = "robot")]

use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::__serde_json::{from_str, json, Value};
use runtime_shared::primitives::text_input::KeyboardKind;
use runtime_shared::{FontWeight, StyleRules};
use runtime_vocabulary::builders::{text, text_area, text_input, view};
use runtime_vocabulary::caps::ResolvedStyle;
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot::{Query, Robot, StyleSource};
use runtime_vocabulary::style_attach::StyleProp;
use runtime_world::signal;

fn call(verb: &str, robot: &Robot, test_id: &str) -> Value {
    let id = robot.find(Query::test_id(test_id)).unwrap_or_else(|| panic!("{test_id} registered")).id.0;
    from_str(&invoke_command(verb, &json!({ "element_id": id })).unwrap()).unwrap()
}

/// A text input reports the keyboard kind the author asked for, and the one
/// the native widget holds; a text area takes no keyboard kind and reports
/// neither.
#[test]
fn a_text_input_reports_its_keyboard_kind_and_the_native_one() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(text_input().test_id("email").keyboard(KeyboardKind::Email).build())
                .child(text_input().test_id("plain").build())
                .child(text_area().test_id("area").build())
                .build(),
        )
    });
    // n1 is the email field: the toolkit shows the email keyboard.
    h.shared.keyboard_kinds.borrow_mut().insert(1, KeyboardKind::Email);

    let email = call("get_state", &robot, "email");
    assert_eq!(email["keyboard"], json!("email"), "{email}");
    assert_eq!(email["native_keyboard"], json!("email"), "{email}");
    let plain = call("get_state", &robot, "plain");
    assert_eq!(plain["keyboard"], json!("default"), "{plain}");
    assert!(plain.get("native_keyboard").is_none(), "this mock reads no kind for it: {plain}");
    let area = call("get_state", &robot, "area");
    assert!(area.get("keyboard").is_none() && area.get("native_keyboard").is_none(), "{area}");
    // `find_element` carries it in `state`.
    let found: Value = from_str(&invoke_command("find_element", &json!({ "test_id": "email" })).unwrap()).unwrap();
    assert_eq!(found["state"]["keyboard"], json!("email"), "{found}");
    robot.reset();
}

/// A live keyboard kind is reported as it changes.
#[test]
fn a_live_keyboard_kind_is_reported_as_it_changes() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let kind = h.world.enter(|| signal(KeyboardKind::Phone));
    let _r = h.world.enter(|| {
        realize(&h.backend, &h.registry, text_input().test_id("f").keyboard_dyn(move || kind.get()).build())
    });
    assert_eq!(call("get_state", &robot, "f")["keyboard"], json!("phone"));
    h.world.enter(|| kind.set(KeyboardKind::Url));
    h.world.flush();
    assert_eq!(call("get_state", &robot, "f")["keyboard"], json!("url"));
    robot.reset();
}

fn struck(done: bool) -> StyleRules {
    StyleRules {
        strikethrough: Some(done),
        font_weight: Some(FontWeight::Bold),
        color: Some("#ff0000".into()),
        ..Default::default()
    }
}

/// With no style read of the toolkit's own, `get_style` reports the rules
/// the framework resolved and handed the backend — and follows a reactive
/// style as it re-applies.
#[test]
fn get_style_reports_the_framework_rules_and_follows_a_reactive_style() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let done = h.world.enter(|| signal(false));
    let _r = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(
                    text()
                        .content("Buy milk")
                        .test_id("item")
                        .style(StyleProp::Dynamic(Box::new(move || Rc::new(struck(done.get())))))
                        .build(),
                )
                .child(text().content("plain").test_id("plain").build())
                .build(),
        )
    });

    let out = call("get_style", &robot, "item");
    assert_eq!(out["source"], json!("framework"), "{out}");
    assert_eq!(out["style"]["text_decoration"], json!("none"), "{out}");
    assert_eq!(out["style"]["font_weight"], json!(700), "{out}");
    assert_eq!(out["style"]["color"], json!([1.0, 0.0, 0.0, 1.0]), "{out}");

    h.world.enter(|| done.set(true));
    h.world.flush();
    let el = robot.find(Query::test_id("item")).unwrap();
    let (source, style) = robot.style(&el).unwrap().expect("styled");
    assert_eq!(source, StyleSource::Framework);
    assert_eq!(style.line_through, Some(true));
    assert_eq!(call("get_style", &robot, "item")["style"]["text_decoration"], json!("line-through"));
    robot.reset();
}

/// A toolkit that resolves style itself (the web's `getComputedStyle`)
/// answers instead of the framework's rules.
#[test]
fn get_style_prefers_the_toolkits_resolved_style() {
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let _r = h.world.enter(|| {
        realize(&h.backend, &h.registry, text().content("x").test_id("t").style(struck(false)).build())
    });
    h.shared.computed_styles.borrow_mut().insert(
        0,
        ResolvedStyle { line_through: Some(true), underline: Some(false), font_size: Some(14.0), ..Default::default() },
    );
    let out = call("get_style", &robot, "t");
    assert_eq!(
        out,
        json!({ "source": "native", "style": { "text_decoration": "line-through", "font_size": 14.0 } })
    );
    robot.reset();
}
