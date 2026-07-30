//! Headless runtime-server smoke tests.
//!
//! Exercises the dev-server core without the WebSocket transport: a
//! recorder backend, a real scene mounted through
//! `dev_server::newcore::SceneSession`, and the in-process Robot API.
//! The scenarios these tests pin down:
//!
//! 1. **Realize output is well-formed.** A small tree produces the
//!    expected `Command` stream — `CreateView` / `CreateText` /
//!    `CreateButton` / `Insert` / `Finish` etc. — so any future
//!    refactor that breaks the recorder shows up immediately.
//!
//! 2. **Robot can drive the server-side registry.** The mount handlers
//!    populate the thread-local vocabulary registry. Tests construct a
//!    [`Robot`], look up an element by label / `test_id`, invoke
//!    `click(...)`, and assert the handler fired.
//!
//! 3. **Teardown reaches the wire.** A released primitive emits
//!    `Command::ReleaseNode` so the client tears down its mirror.
//!
//! The Robot driver env (world-enter for queries, flush for actions) is
//! installed exactly as the sidecar session thread installs it —
//! `dev_server::newcore::install_robot_env`.
//!
//! Run with `cargo test -p dev-server`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dev_server::newcore::{clear_robot_env, install_robot_env, SceneSession};
use dev_server::WireRecordingBackend;
use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
use runtime_vocabulary::builders::{button, portal, text, view};
use runtime_vocabulary::robot::{Query, Robot, TreeNode};
use runtime_world::signal;
use wire::Command;

/// Mount `app` on a fresh recorder with the Robot driver env installed
/// (the sidecar's wiring). The returned holder keeps the session alive;
/// drop it to unmount.
fn boot(
    app: impl FnOnce() -> runtime_scene::Element + 'static,
) -> (WireRecordingBackend, Rc<RefCell<Option<SceneSession>>>) {
    let recorder = WireRecordingBackend::new();
    let session = SceneSession::mount(&recorder, |_r| {}, app);
    let holder: Rc<RefCell<Option<SceneSession>>> = Rc::new(RefCell::new(Some(session)));
    install_robot_env(&holder);
    (recorder, holder)
}

fn teardown(holder: Rc<RefCell<Option<SceneSession>>>) {
    clear_robot_env();
    holder.borrow_mut().take();
    Robot::new().reset();
}

/// A small tree: a view with a static text and a counting button.
/// Returns the click counter so the caller can observe the handler.
fn sample_tree(clicks: Rc<Cell<i32>>) -> runtime_scene::Element {
    view()
        .child(
            text()
                .content("Hello, runtime-server")
                .test_id("greeting")
                .build(),
        )
        .child(
            button()
                .label("Tap me")
                .on_press(move || clicks.set(clicks.get() + 1))
                .test_id("tap-btn")
                .build(),
        )
        .build()
}

/// **Test 1: headless realize produces a well-formed Command stream.**
/// No WebSocket transport, no client. Just the recorder + the scene.
#[test]
fn aas_realizes_tree_into_command_stream() {
    let (recorder, holder) = boot(|| sample_tree(Rc::new(Cell::new(0))));

    let commands = recorder.drain_commands();

    // Children are built first, then the parent View, then each child
    // is inserted, then `finish()`. Use counts (not order) so the test
    // isn't pinned to mount internals.
    let n = |pred: fn(&Command) -> bool| commands.iter().filter(|c| pred(c)).count();
    assert_eq!(
        n(|c| matches!(c, Command::CreateView { .. })),
        1,
        "exactly one View; got {commands:#?}"
    );
    assert_eq!(n(|c| matches!(c, Command::CreateText { .. })), 1, "one Text");
    assert_eq!(
        n(|c| matches!(c, Command::CreateButton { .. })),
        1,
        "one Button"
    );
    let attach_ops = commands
        .iter()
        .filter(|c| matches!(c, Command::Insert { .. } | Command::InsertMany { .. }))
        .count();
    assert!(
        attach_ops == 1 || attach_ops == 2,
        "the two children attach to the View (one InsertMany, or one Insert each); \
         got {attach_ops} attach ops in {commands:#?}"
    );
    assert_eq!(
        n(|c| matches!(c, Command::Finish { .. })),
        1,
        "the mount called finish(root) exactly once"
    );

    // Drain is destructive — a second drain returns nothing new.
    assert!(
        recorder.drain_commands().is_empty(),
        "drain_commands clears the queue"
    );
    teardown(holder);
}

/// **Test 2: the Robot API drives the server-side registry.**
///
/// The mount handlers (running on this thread) populated the
/// thread-local Robot registry. We use that registry to find the button
/// by its label and invoke its press handler — the same path an
/// external MCP client would take, just without the JSON-over-TCP
/// bridge in the middle.
#[test]
fn robot_finds_and_clicks_button_via_server_registry() {
    let clicks = Rc::new(Cell::new(0));
    let clicks_for_app = clicks.clone();
    let (_recorder, holder) = boot(move || sample_tree(clicks_for_app));

    let robot = Robot::new();
    let btn = robot
        .find(Query::label("Tap me"))
        .expect("button registered with label 'Tap me'");
    assert_eq!(btn.label.as_deref(), Some("Tap me"));

    assert_eq!(clicks.get(), 0, "click count starts at 0");
    robot.click(&btn).expect("click dispatches");
    assert_eq!(clicks.get(), 1, "handler fired exactly once via Robot");
    robot.click(&btn).expect("second click dispatches");
    assert_eq!(clicks.get(), 2, "handler is re-invocable");

    teardown(holder);
}

/// **Test 3: `test_id` queries work.** Locked-in semantics: `find` by
/// `test_id` returns the element regardless of label / kind. Useful in
/// larger trees where labels collide or are localized.
#[test]
fn robot_finds_element_by_test_id() {
    let (_recorder, holder) = boot(|| sample_tree(Rc::new(Cell::new(0))));
    let robot = Robot::new();

    let greeting = robot
        .find(Query::test_id("greeting"))
        .expect("Text registered with test_id 'greeting'");
    assert_eq!(
        greeting.label.as_deref(),
        Some("Hello, runtime-server"),
        "Text label captured into the registry"
    );

    let btn = robot
        .find(Query::test_id("tap-btn"))
        .expect("Button registered with test_id 'tap-btn'");
    assert_eq!(btn.label.as_deref(), Some("Tap me"));

    assert!(robot.find(Query::test_id("does-not-exist")).is_none());
    teardown(holder);
}

/// **Regression: robot snapshot/find must reflect LIVE reactive text,
/// not the value captured at mount.**
///
/// Before the `label_fn` fix the registry cached the reactive text's
/// string once at registration. A bound signal could change (the
/// reactive effect updated the backend's view), but `find(...)` and
/// `get_snapshot` kept reporting the mount-time string — an MCP / AI
/// client reading the snapshot to verify a UI update would see stale
/// text. This mutates the signal a reactive `text(...)` reads and
/// asserts every robot read path reports the new value.
#[test]
fn regression_robot_snapshot_reflects_reactive_text() {
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let slot_for_app = slot.clone();
    let (_recorder, holder) = boot(move || {
        let count = signal(0_i32);
        slot_for_app.set(Some(count));
        view()
            .child(
                text()
                    .content(move || format!("count: {}", count.get()))
                    .test_id("count-label")
                    .build(),
            )
            .build()
    });

    let robot = Robot::new();
    let el = robot
        .find(Query::test_id("count-label"))
        .expect("reactive text registered");
    assert_eq!(el.label.as_deref(), Some("count: 0"), "initial find()");
    assert_eq!(
        tree_label(&robot.snapshot(), "count-label").as_deref(),
        Some("count: 0"),
        "initial snapshot()"
    );

    // Mutate the signal the reactive text reads, then commit — the
    // driver env's settle hook is what a bridge verb would run.
    let count = slot.get().expect("signal captured during mount");
    count.set(3);
    holder.borrow().as_ref().expect("session").flush();

    assert_eq!(
        robot
            .find(Query::test_id("count-label"))
            .and_then(|e| e.label)
            .as_deref(),
        Some("count: 3"),
        "find() must report the live reactive label, not the mount-time value"
    );
    assert_eq!(
        tree_label(&robot.snapshot(), "count-label").as_deref(),
        Some("count: 3"),
        "snapshot() must report the live reactive label"
    );
    assert!(
        robot.find(Query::label("count: 3")).is_some(),
        "find(Label) resolves the live text"
    );
    assert!(
        robot.find(Query::label("count: 0")).is_none(),
        "the stale mount-time label no longer matches"
    );

    teardown(holder);
}

/// Depth-first search a robot `snapshot()` tree for the node with the
/// given `test_id`, returning its (live-resolved) label.
fn tree_label(nodes: &[TreeNode], test_id: &str) -> Option<String> {
    for n in nodes {
        if n.test_id == Some(test_id) {
            return n.label.clone();
        }
        if let Some(found) = tree_label(&n.children, test_id) {
            return Some(found);
        }
    }
    None
}

/// Does the robot `snapshot()` tree contain a node with this `test_id`
/// anywhere in the hierarchy (root or descendant)?
fn tree_has_test_id(nodes: &[TreeNode], test_id: &str) -> bool {
    nodes
        .iter()
        .any(|n| n.test_id == Some(test_id) || tree_has_test_id(&n.children, test_id))
}

/// **Regression: a reactive branch swap must remove the old branch's
/// nodes from the robot registry.**
///
/// Field report (§2.4): after `onboarded` flips, `get_snapshot` showed
/// BOTH the onboarding subtree AND the main screen as live nodes in the
/// AAS host. The robot registry tracks every mounted primitive in a
/// thread-local map and never had its entries removed on scope
/// teardown — `deregister` had zero callers. So when the branch driver
/// dropped the old branch's scope (freeing its signals/effects and
/// clearing the backend's children), the registry kept the stale
/// entries forever, and `snapshot()` reported a phantom second screen.
#[test]
fn regression_branch_swap_disposes_old_branch_from_robot_registry() {
    Robot::new().reset();

    let slot: Rc<Cell<Option<runtime_world::Signal<bool>>>> = Rc::new(Cell::new(None));
    let slot_for_app = slot.clone();
    let (_recorder, holder) = boot(move || {
        let onboarded = signal(false);
        slot_for_app.set(Some(onboarded));
        runtime_vocabulary::glue::when(
            move || onboarded.get(),
            || {
                view()
                    .child(text().content("Welcome").test_id("main-screen").build())
                    .build()
            },
            || {
                view()
                    .child(
                        text()
                            .content("Skip for now")
                            .test_id("onboarding-screen")
                            .build(),
                    )
                    .build()
            },
        )
    });

    let robot = Robot::new();
    let snap = robot.snapshot();
    assert!(
        tree_has_test_id(&snap, "onboarding-screen"),
        "onboarding branch should be live before the flip; got {snap:#?}"
    );
    assert!(
        !tree_has_test_id(&snap, "main-screen"),
        "main branch must NOT be built while onboarded == false; got {snap:#?}"
    );

    slot.get().expect("signal").set(true);
    holder.borrow().as_ref().expect("session").flush();

    let snap = robot.snapshot();
    assert!(
        tree_has_test_id(&snap, "main-screen"),
        "main branch must be live after the flip; got {snap:#?}"
    );
    assert!(
        !tree_has_test_id(&snap, "onboarding-screen"),
        "the disposed onboarding branch must NOT linger in the robot snapshot \
         after the condition flips — a stale registry entry is a phantom \
         second live root; got {snap:#?}"
    );
    assert!(
        robot.find(Query::test_id("onboarding-screen")).is_none(),
        "find() must not resolve the disposed onboarding branch"
    );
    assert!(
        robot.find(Query::test_id("main-screen")).is_some(),
        "find() must resolve the live main branch"
    );

    teardown(holder);
}

/// **Regression for the wire-protocol `release_*` not-emitted finding.**
/// When a primitive whose backend `release_*` is wired (Portal,
/// Virtualizer, …) unmounts on the dev side, the recorder must emit a
/// `Command::ReleaseNode` so the client tears down its mirror. Without
/// this, the dev-client's per-node bookkeeping leaks across every
/// hot-reload cycle.
#[test]
fn release_node_emitted_for_portal_when_session_drops() {
    let recorder = WireRecordingBackend::new();
    let session = SceneSession::mount(&recorder, |_r| {}, || {
        view()
            .child(
                portal(PortalTarget::Viewport(ViewportPlacement::Center))
                    .child(text().content("hello inside portal"))
                    .build(),
            )
            .build()
    });

    let pre_drop = recorder.drain_commands();
    let portal_id = pre_drop
        .iter()
        .find_map(|c| match c {
            Command::CreatePortal { id, .. } => Some(*id),
            _ => None,
        })
        .expect("CreatePortal must be emitted while the portal is mounted");
    assert!(
        !pre_drop
            .iter()
            .any(|c| matches!(c, Command::ReleaseNode { node } if *node == portal_id)),
        "ReleaseNode must not be emitted before teardown"
    );

    // Unmount — the portal's release hook fires
    // `caps::PortalOps::release_portal`, which must emit ReleaseNode.
    drop(session);

    let post_drop = recorder.drain_commands();
    assert!(
        post_drop
            .iter()
            .any(|c| matches!(c, Command::ReleaseNode { node } if *node == portal_id)),
        "Command::ReleaseNode {{ node: {portal_id:?} }} must be emitted on teardown; \
         got {post_drop:#?}",
    );
}

/// Regression for the recorder's `reset_log_and_scene` `next_node = 0`
/// identity-collision finding. After a sidecar respawn the recorder
/// resets its command log and scene but KEEPS `identity_to_node`
/// populated — so a freshly minted id must never land on a `NodeId` an
/// identity already owns.
///
/// The mount path does not currently set ambient identities (the named
/// gap in `dev_server::newcore`'s module docs — remounts rebuild rather
/// than patch), so this drives the recorder's identity lane DIRECTLY
/// via `with_current_identity`, which is the seam a future
/// identity-setting mount path will use.
#[test]
fn reset_log_and_scene_does_not_collide_minted_ids_with_cached_identities() {
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::identity::{with_current_identity, Identity};
    use runtime_vocabulary::caps::{TextOps, ViewOps};

    fn ident(slot: u32) -> Identity {
        Identity::node(Identity::ROOT_SCOPE, slot, None, None)
    }

    let mut recorder = WireRecordingBackend::new();

    // Walk 1: three identified nodes. `identity_to_node` caches their
    // ids; `next_node` advances past them.
    with_current_identity(ident(0), || {
        ViewOps::create_view(&mut recorder, &AccessibilityProps::default())
    });
    with_current_identity(ident(1), || {
        TextOps::create_text(&mut recorder, "row-a", &AccessibilityProps::default())
    });
    with_current_identity(ident(2), || {
        TextOps::create_text(&mut recorder, "row-b", &AccessibilityProps::default())
    });
    let _ = recorder.drain_commands();

    // Sidecar respawn / hot patch.
    recorder.reset_log_and_scene();

    // Walk 2: the same three identities (which must reuse their cached
    // ids) plus one NEW, unidentified emission. Pre-fix, `next_node =
    // 0` made the new emission land on the cached View's id.
    let a = with_current_identity(ident(0), || {
        ViewOps::create_view(&mut recorder, &AccessibilityProps::default())
    });
    let b = with_current_identity(ident(1), || {
        TextOps::create_text(&mut recorder, "row-a", &AccessibilityProps::default())
    });
    let c = with_current_identity(ident(2), || {
        TextOps::create_text(&mut recorder, "row-b", &AccessibilityProps::default())
    });
    let fresh = TextOps::create_text(&mut recorder, "row-c", &AccessibilityProps::default());

    for (name, cached) in [("view", a), ("row-a", b), ("row-b", c)] {
        assert_ne!(
            fresh, cached,
            "NodeId collision after reset_log_and_scene: the freshly minted id \
             {fresh:?} equals the cached identity id for {name} — `next_node = 0` \
             is recycling ids that `identity_to_node` already holds."
        );
    }
}
