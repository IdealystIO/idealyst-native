//! Headless runtime-server smoke tests.
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
//!    `robot` feature on for `runtime-core` (activated via
//!    dev-deps), the walker populates the thread-local registry.
//!    Tests construct a [`Robot`], look up an element by label,
//!    invoke `click(...)`, and assert the click closure fired by
//!    checking that the recorder emitted a follow-up command (the
//!    closure's signal mutation re-fires the relevant effect).
//!
//! Run with `cargo test -p dev-server`. The `robot` feature on
//! `runtime-core` is enabled automatically through this crate's
//! `[dev-dependencies]` block.

use std::cell::RefCell;
use std::rc::Rc;

use dev_server::WireRecordingBackend;
use runtime_core::primitives::portal::{PortalTarget, ViewportPlacement};
use runtime_core::robot::{Query, Robot};
use runtime_core::{
    render, signal, IntoAction, Element, SafeAreaSides, TextSource,
};
use wire::Command;

/// Build a small primitive tree by hand (no `ui!` macro — keeps the
/// test self-contained and explicit about every field). Returns a
/// `(tree, click_count_signal)` pair so the caller can both render
/// the tree and observe state mutated by the button's `on_click`.
fn sample_tree() -> (Element, runtime_core::Signal<i32>) {
    let click_count = signal!(0_i32);
    // `Element::Button.on_click` is `derive::Action` since the
    // generator migration — bare `Fn()` closures lift via the
    // blanket `IntoAction for F: Fn()` impl (which wraps the
    // closure into `Action.fire`).
    let on_click = (move || {
        click_count.set(click_count.get() + 1);
    })
    .into_action();

    let tree = Element::View {
        children: vec![
            Element::Text {
                source: TextSource::Static("Hello, runtime-server".into()),
                style: None,
                ref_fill: None,
                accessibility: Default::default(),
                test_id: Some("greeting"),
            },
            Element::Button {
                label: TextSource::Static("Tap me".into()),
                on_click,
                leading_icon: None,
                trailing_icon: None,
                style: None,
                ref_fill: None,
                disabled: None,
                accessibility: Default::default(),
                test_id: Some("tap-btn"),
            },
        ],
        style: None,
        ref_fill: None,
        safe_area_sides: SafeAreaSides::NONE,
        on_touch: None,
        accessibility: Default::default(),
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

/// **Regression test for the audit's wire-protocol `release_*` not-emitted
/// finding.** When a primitive whose backend `release_*` is wired (Portal,
/// Virtualizer, Navigator, …) unmounts on the dev side, the recorder must
/// emit a `Command::ReleaseNode` so the client tears down its mirror.
/// Without this, the dev-client's per-node bookkeeping leaks across every
/// hot-reload cycle.
#[test]
fn release_node_emitted_for_portal_when_owner_drops() {
    let portal = Element::Portal {
        children: vec![Element::Text {
            source: TextSource::Static("hello inside portal".into()),
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
            test_id: None,
        }],
        target: PortalTarget::Viewport(ViewportPlacement::Center),
        on_dismiss: None,
        trap_focus: false,
        style: None,
        ref_fill: None,
        accessibility: Default::default(),
    };

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let owner = render(backend_rc, portal);

    // Find the CreatePortal so we know which NodeId to expect on release.
    let pre_drop = recorder.drain_commands();
    let portal_id = pre_drop
        .iter()
        .find_map(|c| match c {
            Command::CreatePortal { id, .. } => Some(*id),
            _ => None,
        })
        .expect("CreatePortal must be emitted while the portal is mounted");

    // Pre-drop: no ReleaseNode for the portal yet.
    assert!(
        !pre_drop.iter().any(|c| matches!(c, Command::ReleaseNode { node } if *node == portal_id)),
        "ReleaseNode must not be emitted before the owner drops"
    );

    // Drop the owner — the framework's PortalHandleCleanup RAII guard fires
    // backend.release_portal(node), which must emit Command::ReleaseNode.
    drop(owner);

    let post_drop = recorder.drain_commands();
    assert!(
        post_drop.iter().any(|c| matches!(c, Command::ReleaseNode { node } if *node == portal_id)),
        "Command::ReleaseNode {{ node: {} }} must be emitted on Owner drop; \
         got {:#?}",
        portal_id,
        post_drop,
    );
}

/// Regression test for the audit's wire-protocol `reset_log_and_scene`
/// `next_node = 0` identity-collision finding. After a hot-patch / sidecar
/// respawn, the recorder resets `next_node` to 0 but keeps
/// `identity_to_node` populated. A walker that emits any *new* identity
/// after the reset would mint `NodeId(1)` — colliding with whatever
/// identity was already cached at `NodeId(1)` from the first walk.
///
/// The fix preserves `next_node` past the high-water mark so freshly
/// minted ids never overlap previously-cached identity ids.
#[test]
fn reset_log_and_scene_does_not_collide_minted_ids_with_cached_identities() {
    use std::collections::HashMap;

    fn extract_create_id(cmd: &Command) -> Option<wire::NodeId> {
        match cmd {
            Command::CreateView { id, .. }
            | Command::CreateText { id, .. }
            | Command::CreateButton { id, .. }
            | Command::CreateImage { id, .. }
            | Command::CreateToggle { id, .. }
            | Command::CreateSlider { id, .. } => Some(*id),
            _ => None,
        }
    }

    fn tree_with_n_text(n: usize) -> Element {
        let mut children = Vec::with_capacity(n);
        for i in 0..n {
            let id_str: &'static str = match i {
                0 => "a",
                1 => "b",
                2 => "c",
                _ => panic!("extend test_ids array"),
            };
            children.push(Element::Text {
                source: TextSource::Static(format!("row-{}", id_str).into()),
                style: None,
                ref_fill: None,
                accessibility: Default::default(),
                test_id: Some(id_str),
            });
        }
        Element::View {
            children,
            style: None,
            ref_fill: None,
            safe_area_sides: SafeAreaSides::NONE,
            on_touch: None,
            accessibility: Default::default(),
            test_id: Some("root"),
        }
    }

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Walk 1: 2 text rows. Walker mints next_node past 0 for the View
    // and the two Texts; `identity_to_node` caches those ids.
    let owner1 = render(backend_rc.clone(), tree_with_n_text(2));
    let _ = recorder.drain_commands();
    drop(owner1);

    // Sidecar respawn / hot patch.
    recorder.reset_log_and_scene();

    // Walk 2: the same View + first two texts (cached identities reuse
    // their ids) PLUS one new text emission. The new emission must NOT
    // collide with any previously-cached identity id.
    let _owner2 = render(backend_rc.clone(), tree_with_n_text(3));
    let walk2 = recorder.drain_commands();

    // No two `Create*` commands in walk 2 should share a NodeId. Pre-fix,
    // the third Text's emission lands on `NodeId(1)` — the cached View's id.
    let mut seen: HashMap<wire::NodeId, String> = HashMap::new();
    for cmd in &walk2 {
        if let Some(id) = extract_create_id(cmd) {
            let label = format!("{:?}", cmd);
            if let Some(prev) = seen.insert(id, label.clone()) {
                panic!(
                    "NodeId collision after reset_log_and_scene: id {id:?} \
                     emitted twice — first as `{prev}`, then as `{label}`. \
                     `next_node = 0` reset is recycling ids that \
                     `identity_to_node` already holds."
                );
            }
        }
    }
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
        Some("Hello, runtime-server"),
        "Text label captured into the registry"
    );

    let button = robot
        .find(Query::test_id("tap-btn"))
        .expect("Button registered with test_id 'tap-btn'");
    assert_eq!(button.label.as_deref(), Some("Tap me"));

    // Missing test_id → None.
    assert!(robot.find(Query::test_id("does-not-exist")).is_none());
}

/// **Test 7: hot-patch keeps primitive HandlerIds stable.**
///
/// Covers the same identity-keyed `HandlerId` dedup as test 6, but
/// for `Element::Button`'s `on_click` — the broader migration
/// after the header_left fix. The same property must hold for any
/// primitive whose backend `create_*` registers an event handler
/// (Toggle, Slider, TextInput, Link, overlay `on_dismiss`); the
/// pattern is mechanical so testing one primitive proves the
/// generalized fix.
///
/// Without identity-keyed registration, the post-reset render
/// would mint a fresh `HandlerId` for the button and the client's
/// leaked native click-listener (which captured the original id on
/// install) would resolve to a dead slot on the server — clicks
/// silently dropped after every hot-patch.
#[test]
fn aas_hot_patch_preserves_button_handler_id() {
    let click_count = signal!(0_i32);
    let on_click = (move || {
        click_count.set(click_count.get() + 1);
    })
    .into_action();

    // Wrap in a thunk so we re-walk the same primitive tree after
    // the reset — the walker's identity-per-emission-site machinery
    // is what gives the Button a stable Identity across rebuilds,
    // which the recorder then uses to recycle the HandlerId.
    let build = || Element::Button {
        label: TextSource::Static("Tap".into()),
        on_click: on_click.clone(),
        leading_icon: None,
        trailing_icon: None,
        style: None,
        ref_fill: None,
        disabled: None,
        accessibility: Default::default(),
        test_id: Some("btn"),
    };

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    let owner_a = render(backend_rc.clone(), build());
    let cmds_a = recorder.drain_commands();
    let handler_a = cmds_a.iter().find_map(|c| match c {
        Command::CreateButton { on_click, .. } => Some(*on_click),
        _ => None,
    });
    assert!(handler_a.is_some(), "initial render must emit CreateButton");
    drop(owner_a);

    recorder.reset_log_and_scene();
    let _owner_b = render(backend_rc, build());
    let cmds_b = recorder.drain_commands();
    let handler_b = cmds_b.iter().find_map(|c| match c {
        Command::CreateButton { on_click, .. } => Some(*on_click),
        _ => None,
    });

    assert_eq!(
        handler_a, handler_b,
        "Button.on_click HandlerId MUST be stable across reset_log_and_scene \
         (identity-keyed via the Button's node Identity) — otherwise leaked \
         client-side click listeners go stale on every hot-patch."
    );
}

