//! Headless AAS smoke tests.
//!
//! Exercises the dev-server core without the WebSocket transport:
//! a recorder backend, a synthetic primitive tree, the real
//! framework walker, and the in-process Robot API. The two scenarios
//! these tests pin down:
//!
//! 1. **Render output is well-formed.** A small tree produces the
//!    expected `Command` stream — `CreateView` / `CreateText` /
//!    `CreateButton` / `Insert` / `Finish` etc. — so any future
//!    refactor that breaks the recorder shows up immediately.
//!
//! 2. **Robot can drive the server-side registry.** With the
//!    `robot` feature on for `framework-core` (activated via
//!    dev-deps), the walker populates the thread-local registry.
//!    Tests construct a [`Robot`], look up an element by label,
//!    invoke `click(...)`, and assert the click closure fired by
//!    checking that the recorder emitted a follow-up command (the
//!    closure's signal mutation re-fires the relevant effect).
//!
//! Run with `cargo test -p dev-server`. The `robot` feature on
//! `framework-core` is enabled automatically through this crate's
//! `[dev-dependencies]` block.

use std::cell::RefCell;
use std::rc::Rc;

use dev_server::WireRecordingBackend;
use framework_core::robot::{Query, Robot};
use framework_core::{render, signal, Primitive, SafeAreaSides, TextSource};
use wire::Command;

/// Build a small primitive tree by hand (no `ui!` macro — keeps the
/// test self-contained and explicit about every field). Returns a
/// `(tree, click_count_signal)` pair so the caller can both render
/// the tree and observe state mutated by the button's `on_click`.
fn sample_tree() -> (Primitive, framework_core::Signal<i32>) {
    let click_count = signal!(0_i32);
    let on_click: Rc<dyn Fn()> = Rc::new(move || {
        click_count.set(click_count.get() + 1);
    });

    let tree = Primitive::View {
        children: vec![
            Primitive::Text {
                source: TextSource::Static("Hello, AAS".into()),
                style: None,
                ref_fill: None,
                test_id: Some("greeting"),
            },
            Primitive::Button {
                label: TextSource::Static("Tap me".into()),
                on_click,
                leading_icon: None,
                trailing_icon: None,
                style: None,
                ref_fill: None,
                disabled: None,
                test_id: Some("tap-btn"),
            },
        ],
        style: None,
        ref_fill: None,
        safe_area_sides: SafeAreaSides::NONE,
        test_id: None,
    };

    (tree, click_count)
}

/// **Test 1: headless render produces a well-formed Command stream.**
/// No WebSocket transport, no client. Just the recorder + walker.
#[test]
fn aas_renders_tree_into_command_stream() {
    let (tree, _click_count) = sample_tree();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    // `_owner` MUST be held — dropping it tears down every reactive
    // scope (the same shape every consumer of `render` uses).
    let _owner = render(backend_rc, tree);

    let commands = recorder.drain_commands();

    // Walker builds children first then the parent View, then
    // inserts each child, then `finish()`. Use counts (not order)
    // so we don't pin the test to walker internals.
    let n = |pred: fn(&Command) -> bool| commands.iter().filter(|c| pred(c)).count();
    assert_eq!(
        n(|c| matches!(c, Command::CreateView { .. })),
        1,
        "exactly one View"
    );
    assert_eq!(
        n(|c| matches!(c, Command::CreateText { .. })),
        1,
        "exactly one Text"
    );
    assert_eq!(
        n(|c| matches!(c, Command::CreateButton { .. })),
        1,
        "exactly one Button"
    );
    assert_eq!(
        n(|c| matches!(c, Command::Insert { .. })),
        2,
        "Text + Button each inserted into the View"
    );
    assert_eq!(
        n(|c| matches!(c, Command::Finish { .. })),
        1,
        "render() called finish(root) exactly once"
    );

    // Drain is destructive — a second drain returns nothing new.
    assert!(
        recorder.drain_commands().is_empty(),
        "drain_commands clears the queue"
    );
}

/// **Test 2: Robot API drives the server-side registry.**
///
/// The walker (running on this thread) populated the thread-local
/// Robot registry while building the tree. We use that registry to
/// find the button by its label and invoke its `on_click` — the
/// same path an external MCP client would take, just without the
/// JSON-over-TCP bridge in the middle.
///
/// Asserts: the click handler fires (the captured signal increments).
#[test]
fn robot_finds_and_clicks_button_via_server_registry() {
    let (tree, click_count) = sample_tree();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let robot = Robot::new();

    // Find by label (exact). The label is the static button text
    // the walker captured.
    let btn = robot
        .find(Query::label("Tap me"))
        .expect("button registered with label 'Tap me'");

    assert!(
        btn.label.as_deref() == Some("Tap me"),
        "found element has the expected label"
    );

    // Pre-condition: click handler hasn't fired yet.
    assert_eq!(click_count.get(), 0, "click count starts at 0");

    // The click invokes the registered `on_click` closure on this
    // thread (same thread the registry lives on — same posture as
    // `BridgeHandle::poll` running on the dev-server's main loop).
    robot.click(&btn).expect("click dispatches");

    assert_eq!(
        click_count.get(),
        1,
        "click handler fired exactly once via Robot"
    );

    // A second click — same closure, same observation.
    robot.click(&btn).expect("second click dispatches");
    assert_eq!(click_count.get(), 2, "click handler is re-invocable");
}

/// **Test 3: test_id queries work.** Locked-in semantics: `find` by
/// `test_id` returns the element regardless of label / kind. Useful
/// in larger trees where labels collide or are localized.
#[test]
fn robot_finds_element_by_test_id() {
    let (tree, _) = sample_tree();

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let robot = Robot::new();

    let greeting = robot
        .find(Query::test_id("greeting"))
        .expect("Text registered with test_id 'greeting'");
    assert_eq!(
        greeting.label.as_deref(),
        Some("Hello, AAS"),
        "Text label captured into the registry"
    );

    let button = robot
        .find(Query::test_id("tap-btn"))
        .expect("Button registered with test_id 'tap-btn'");
    assert_eq!(button.label.as_deref(), Some("Tap me"));

    // Missing test_id → None.
    assert!(robot.find(Query::test_id("does-not-exist")).is_none());
}
