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
    render, signal, Color, DrawerNavigator, IntoAction,
    Element, Route, SafeAreaSides, TextSource, TokenEntry,
};
use idea_ui::{active_theme, install_theme, set_theme, ThemeTokens};
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

/// **Test 4: theme toggle re-emits navigator chrome + body style.**
///
/// Pins down the runtime-server reactive bridge for the
/// `.header_background(...)` / `.title_color(...)` / `.header_tint(...)`
/// / `.background_color(...)` builder methods on `DrawerNavigator`.
///
/// Each callback is a `Fn() -> Color` whose body reads a reactive
/// source (typically `active_theme()` via idea-ui). On the local
/// path the walker wraps each in an `Effect`, so a theme swap
/// re-fires the closure and re-applies the resolved color directly
/// on the backend. On the runtime-server path the same Effect runs on the
/// **server**, but its application target is `WireRecordingBackend`
/// — which must (a) emit a new `ApplyNavigator{Header,Title,Button,
/// Body}Style` command for each cohort firing, and (b) emit a
/// **distinct** `StyleId` per theme so the client recognizes it as
/// new content (content-addressed `intern_style` makes this work
/// when the wire's resolved `WireColor` string differs).
///
/// Bug profile: if the cohort never fires on the server, drain
/// after the toggle returns no new style commands → assertion fails.
/// If it fires but `intern_style` returns the same `StyleId` (e.g.
/// the resolved colors hash identically), the StyleIds compare
/// equal → assertion fails. Either failure narrows the search.
#[test]
fn aas_theme_toggle_re_emits_navigator_style_commands() {
    // Theme is a single Color signal — toggling it stands in for a
    // light/dark theme swap and exercises the same reactive path.
    let bg = signal!(Color("#ffffff".into()));
    let title = signal!(Color("#222222".into()));
    let tint = signal!(Color("#222222".into()));
    let body = signal!(Color("#fafafa".into()));

    const ROUTE: Route<()> = Route::<()>::new("home", "/");

    // Build the navigator. `.header_background(...)` / `.title_color(...)`
    // / `.header_tint(...)` / `.background_color(...)` are exactly the
    // setters the docs example's `.header(idea_header(...))` helper
    // fans out to (see DrawerNavigator::header). Capturing the
    // signals here means a `.set(...)` on any of them mimics the
    // "theme cohort fires" event without requiring a full theme
    // registration in the test.
    let tree = Element::from(
        DrawerNavigator::new(&ROUTE)
            .screen(ROUTE, |_| {
                Element::View {
                    children: vec![],
                    style: None,
                    ref_fill: None,
                    safe_area_sides: SafeAreaSides::NONE,
                    on_touch: None,
                    accessibility: Default::default(),
                    test_id: None,
                }
            })
            .header_background(move || bg.get())
            .title_color(move || title.get())
            .header_tint(move || tint.get())
            .background_color(move || body.get()),
    );

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    // --- Initial render: each slot should have emitted exactly one
    // Apply* command. Stash their StyleIds so we can compare against
    // the post-toggle emissions. ---
    let initial = recorder.drain_commands();
    let pick = |cmds: &[Command], slot: &str| -> Option<wire::StyleId> {
        cmds.iter().find_map(|c| match (slot, c) {
            ("header", Command::ApplyNavigatorHeaderStyle { style, .. }) => Some(*style),
            ("title", Command::ApplyNavigatorTitleStyle { style, .. }) => Some(*style),
            ("button", Command::ApplyNavigatorButtonStyle { style, .. }) => Some(*style),
            ("body", Command::ApplyNavigatorBodyStyle { style, .. }) => Some(*style),
            _ => None,
        })
    };
    let sid_header_a = pick(&initial, "header").expect("initial ApplyNavigatorHeaderStyle");
    let sid_title_a = pick(&initial, "title").expect("initial ApplyNavigatorTitleStyle");
    let sid_button_a = pick(&initial, "button").expect("initial ApplyNavigatorButtonStyle");
    let sid_body_a = pick(&initial, "body").expect("initial ApplyNavigatorBodyStyle");

    // --- Toggle each signal to fresh colors. The Effects the walker
    // installed should re-fire, the recorder should emit fresh
    // Apply* commands, and each StyleId must differ from the
    // initial (intern_style is content-addressed on the wire
    // representation, so distinct WireColor strings → distinct
    // sids). ---
    bg.set(Color("#000000".into()));
    title.set(Color("#eeeeee".into()));
    tint.set(Color("#eeeeee".into()));
    body.set(Color("#050505".into()));

    let after = recorder.drain_commands();
    let sid_header_b = pick(&after, "header")
        .expect("theme toggle MUST re-emit ApplyNavigatorHeaderStyle");
    let sid_title_b = pick(&after, "title")
        .expect("theme toggle MUST re-emit ApplyNavigatorTitleStyle");
    let sid_button_b = pick(&after, "button")
        .expect("theme toggle MUST re-emit ApplyNavigatorButtonStyle");
    let sid_body_b = pick(&after, "body")
        .expect("theme toggle MUST re-emit ApplyNavigatorBodyStyle");

    assert_ne!(sid_header_a, sid_header_b, "header StyleId must change across theme swap");
    assert_ne!(sid_title_a, sid_title_b, "title StyleId must change across theme swap");
    assert_ne!(sid_button_a, sid_button_b, "button StyleId must change across theme swap");
    assert_ne!(sid_body_a, sid_body_b, "body StyleId must change across theme swap");
}

/// Trivial theme used in test 5. Just enough to satisfy
/// `ThemeTokens`; the navigator callbacks below read the theme
/// via `active_theme()` (the same path `idea_header(...)` takes)
/// rather than via tokens, so the empty `tokens()` list is fine.
#[derive(Clone)]
struct TinyTheme {
    bg: Color,
    fg: Color,
}
impl ThemeTokens for TinyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

/// **Test 5: `set_theme()` (real-world path) re-emits navigator style commands.**
///
/// The path the docs example actually exercises differs from test 4
/// in one place: instead of mutating a signal the closure captured,
/// the closure reads `active_theme()` and the user calls
/// `set_theme(...)` to swap themes. That hits a different signal
/// (the thread-local `ACTIVE_THEME`) than test 4's per-color
/// signals — so this test pins down that the Effect installed by
/// `attach_navigator_color_callback` correctly subscribes to
/// `active_theme()` (transitively, via the closure's body).
///
/// If this test passes but the device still doesn't retint, the
/// bug is downstream of the server — in the wire transport or the
/// Android client's replay.
#[test]
fn aas_set_theme_re_emits_navigator_style_commands() {
    install_theme(TinyTheme {
        bg: Color("#ffffff".into()),
        fg: Color("#111111".into()),
    });

    const ROUTE: Route<()> = Route::<()>::new("home", "/");

    // The closures read `active_theme()` exactly like
    // `idea_header(|t| HeaderStyle { ... })` does. When wrapped in
    // an Effect by the walker, the Effect's tracked deps include
    // the `ACTIVE_THEME` signal so `set_theme(...)` re-fires it.
    let tree = Element::from(
        DrawerNavigator::new(&ROUTE)
            .screen(ROUTE, |_| {
                Element::View {
                    children: vec![],
                    style: None,
                    ref_fill: None,
                    safe_area_sides: SafeAreaSides::NONE,
                    on_touch: None,
                    accessibility: Default::default(),
                    test_id: None,
                }
            })
            .header_background(|| {
                let t = active_theme();
                t.downcast_ref::<TinyTheme>().unwrap().bg.clone()
            })
            .title_color(|| {
                let t = active_theme();
                t.downcast_ref::<TinyTheme>().unwrap().fg.clone()
            })
            .header_tint(|| {
                let t = active_theme();
                t.downcast_ref::<TinyTheme>().unwrap().fg.clone()
            })
            .background_color(|| {
                let t = active_theme();
                t.downcast_ref::<TinyTheme>().unwrap().bg.clone()
            }),
    );

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let initial = recorder.drain_commands();
    let pick = |cmds: &[Command], slot: &str| -> Option<wire::StyleId> {
        cmds.iter().find_map(|c| match (slot, c) {
            ("header", Command::ApplyNavigatorHeaderStyle { style, .. }) => Some(*style),
            ("title", Command::ApplyNavigatorTitleStyle { style, .. }) => Some(*style),
            ("button", Command::ApplyNavigatorButtonStyle { style, .. }) => Some(*style),
            ("body", Command::ApplyNavigatorBodyStyle { style, .. }) => Some(*style),
            _ => None,
        })
    };
    let sid_header_a = pick(&initial, "header").expect("initial header style");
    let sid_title_a = pick(&initial, "title").expect("initial title style");
    let sid_button_a = pick(&initial, "button").expect("initial button style");
    let sid_body_a = pick(&initial, "body").expect("initial body style");

    // Swap to a clearly different theme. `set_theme` writes to the
    // global `ACTIVE_THEME` signal; the walker's Effects whose
    // closures called `active_theme()` should re-fire.
    set_theme(TinyTheme {
        bg: Color("#000000".into()),
        fg: Color("#eeeeee".into()),
    });

    let after = recorder.drain_commands();
    let sid_header_b = pick(&after, "header")
        .expect("set_theme MUST re-emit ApplyNavigatorHeaderStyle");
    let sid_title_b = pick(&after, "title")
        .expect("set_theme MUST re-emit ApplyNavigatorTitleStyle");
    let sid_button_b = pick(&after, "button")
        .expect("set_theme MUST re-emit ApplyNavigatorButtonStyle");
    let sid_body_b = pick(&after, "body")
        .expect("set_theme MUST re-emit ApplyNavigatorBodyStyle");

    assert_ne!(sid_header_a, sid_header_b, "header StyleId must change on set_theme");
    assert_ne!(sid_title_a, sid_title_b, "title StyleId must change on set_theme");
    assert_ne!(sid_button_a, sid_button_b, "button StyleId must change on set_theme");
    assert_ne!(sid_body_a, sid_body_b, "body StyleId must change on set_theme");
}

/// **Test 6: hot-patch keeps the header `HandlerId` stable.**
///
/// Pins down the identity-keyed `HandlerId` dedup that makes the
/// Android Toolbar's hamburger button (and any other client-leaked
/// callback pointer) survive a sidecar respawn.
///
/// Bug profile before the fix: every `reset_log_and_scene` rebuilt
/// `state.handlers` from scratch — fresh `HandlerId`s were minted
/// starting at 1 again. The client's idempotent
/// `NavigatorAttachInitial` dedup (URL-based) meant the Android
/// Toolbar was never re-attached, so its leaked
/// `HeaderButtonCallback` kept pointing at the *old* id. Taps fired
/// `EventOccurred { handler: OLD }` → server lookup miss → click
/// dropped. The visible symptom was "hamburger stops opening the
/// drawer after the first hot-patch."
///
/// The fix routes `header_button_to_wire` through
/// `HandlerTable::register_unit_for_identity`, deriving an identity
/// from the ambient walker identity (the navigator's, which is
/// already structurally stable across rebuilds) plus a left/right
/// slot. The `identity_to_id` map persists across
/// `clear_closures`, so the post-reset re-register lands on the
/// same `HandlerId` with the freshly-walked closure (capturing the
/// new `Rc<NavigatorControl>`). This test pins that property.
#[test]
fn aas_hot_patch_preserves_header_handler_id() {
    const ROUTE: Route<()> = Route::<()>::new("home", "/");

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Wrap the build in a thunk so we can re-run it after the
    // reset, mirroring what the sidecar's `ApplyPatch` arm does.
    // `header_left` is set explicitly with a no-op closure so the
    // backend's `header_button_to_wire` registers a handler we can
    // compare. (Drawer auto-injects `header_left` for the menu
    // hamburger too — same identity-keyed path — but explicit is
    // easier to observe.)
    let build = || {
        Element::from(
            DrawerNavigator::new(&ROUTE).screen(ROUTE, |_| {
                runtime_core::primitives::navigator::Screen::new(Element::View {
                    children: vec![],
                    style: None,
                    ref_fill: None,
                    safe_area_sides: SafeAreaSides::NONE,
                    on_touch: None,
                    accessibility: Default::default(),
                    test_id: None,
                })
                .header_left(runtime_core::primitives::navigator::HeaderButton::new(
                    "line.3.horizontal",
                    || {}, // no-op; we only care about the HandlerId
                ))
            }),
        )
    };

    // --- Initial render: capture the header_left HandlerId. ---
    let owner = render(backend_rc.clone(), build());
    let cmds_a = recorder.drain_commands();
    let header_handler_a = cmds_a.iter().find_map(|c| match c {
        Command::NavigatorAttachInitial { options, .. } => options
            .header_left
            .as_ref()
            .map(|hb| hb.on_press),
        _ => None,
    });
    assert!(
        header_handler_a.is_some(),
        "initial render must emit a NavigatorAttachInitial with header_left"
    );
    drop(owner);

    // --- Simulate hot-patch: drop the owner, reset, re-render. ---
    // Mirrors the sidecar's `ApplyPatch` flow in
    // `crates/build/runtime-server/src/lib.rs`. Without identity-keyed
    // `HandlerId`s, the post-reset render would mint a *new* id
    // for the header_left, and the test asserts on equality
    // would fail.
    recorder.reset_log_and_scene();
    let _owner_b = render(backend_rc, build());
    let cmds_b = recorder.drain_commands();
    let header_handler_b = cmds_b.iter().find_map(|c| match c {
        Command::NavigatorAttachInitial { options, .. } => options
            .header_left
            .as_ref()
            .map(|hb| hb.on_press),
        _ => None,
    });

    assert_eq!(
        header_handler_a, header_handler_b,
        "header_left HandlerId MUST be stable across reset_log_and_scene — \
         otherwise client-side leaked callbacks (Android Toolbar hamburger \
         etc.) capture the old id at install time and silently fail to \
         dispatch after a hot-patch."
    );
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

