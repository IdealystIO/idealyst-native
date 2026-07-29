//! End-to-end proof of the P3a macro lowering: the `ui!` +
//! `#[component]` authored app (src/app.rs) mounted against the
//! scene-parity mock host (`LegacyBridge<FullRecorder>`), driven by
//! recorded BUTTON HANDLERS (the real event path: macro → builders →
//! prims → registry handlers → backend calls) plus `world.flush()`,
//! asserting the recorded full-op log — structure at mount, in-place
//! updates, keyed reconcile behavior, and row-local state survival.
#![cfg(feature = "new-core")]

use std::cell::RefCell;
use std::rc::Rc;

use newcore_app::app::{build_app, build_demo, AppHandle, DemoHandle, Todo};
use runtime_vocabulary::glue::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, PNode, Recorder};

// ===========================================================================
// Manually pumped scheduler (the vocabulary presence-test pattern):
// presence enter/exit anims schedule frame + after_ms callbacks; queueing
// them keeps each step's op-log deterministic. The registry is a
// process-global first-install-wins, the queues are thread-local — each
// `#[test]` thread pumps only its own tasks. The TodoApp tests schedule
// nothing, so installing here changes nothing for them.
// ===========================================================================

type Queued = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

thread_local! {
    static FRAME_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
    static TIMER_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
}

struct PumpHandle(Queued);

impl ScheduleHandle for PumpHandle {
    fn cancel(&mut self) {
        self.0.borrow_mut().take();
    }
}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct PumpScheduler;

impl Scheduler for PumpScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce()>) {
        f();
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        FRAME_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        TIMER_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn raf_loop(&self, _f: Box<dyn FnMut()>) -> Box<dyn ScheduleHandle> {
        Box::new(PumpHandle(Rc::new(RefCell::new(None))))
    }
}

fn ensure_scheduler() {
    install_scheduler(Box::new(PumpScheduler)); // first call wins
}

fn pump(queue: &'static std::thread::LocalKey<RefCell<Vec<Queued>>>) {
    let tasks: Vec<Queued> = queue.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for slot in tasks {
        let f = slot.borrow_mut().take();
        if let Some(f) = f {
            f();
        }
    }
}

fn pump_frames() {
    pump(&FRAME_QUEUE);
}

fn pump_timers() {
    pump(&TIMER_QUEUE);
}

type Bridged = LegacyBridge<FullRecorder>;

struct Harness {
    rec: Recorder,
    backend: Rc<RefCell<Bridged>>,
    world: World,
    handle: AppHandle,
    // Held: dropping it IS unmount.
    _realized: Realized<PNode>,
}

impl Harness {
    /// Mount the app; returns the harness plus the mount-time op log.
    fn mount() -> (Harness, Vec<String>) {
        let rec = Recorder::default();
        let backend = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
            rec.clone(),
            Mode::Spliced,
        ))));
        let mut registry: Registry<Bridged> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();
        let (root, handle) = world.enter(build_app);
        let realized = world.enter(|| realize(&backend, &registry, root));
        let mount_ops = rec.take_ops();
        (
            Harness { rec, backend, world, handle, _realized: realized },
            mount_ops,
        )
    }

    /// Fire the recorded press handler of the button whose CREATION-time
    /// label was `label`, then flush the world; returns the ops the
    /// flush produced.
    fn press(&self, label: &str) -> Vec<String> {
        let fire = self.backend.borrow().0.button_action(label);
        fire();
        self.flush()
    }

    fn flush(&self) -> Vec<String> {
        self.world.flush();
        self.rec.take_ops()
    }
}

fn joined(ops: &[String]) -> String {
    ops.join("\n")
}

/// Mount pins the authored structure: nested components (Section →
/// header + children), f-string memo text, controlled input, buttons,
/// the reactive-if else branch, and one keyed row per seed todo — all
/// realized through the vocabulary registry.
#[test]
fn mount_realizes_full_authored_structure() {
    let (_h, ops) = Harness::mount();
    let log = joined(&ops);

    // Section header (reactive title prop rendered by a closure) + the
    // static `highlighted` marker (default overridden at the call site).
    assert!(log.contains("\"Todos\""), "section title:\n{log}");
    assert!(log.contains("text \"*\""), "highlighted marker:\n{log}");
    // Defaulted `highlighted` on the About section: exactly one marker.
    assert_eq!(log.matches("text \"*\"").count(), 1, "one marker only:\n{log}");
    assert!(log.contains("text \"static footer\""), "footer child:\n{log}");

    // The memo-driven f-string ("{remaining} left"): 1 of 2 seeds open.
    assert!(log.contains("\"1 left\""), "memo f-string:\n{log}");

    // Controlled input with placeholder.
    assert!(log.contains("text_input") && log.contains("What next?"), "input:\n{log}");

    // Buttons: add + per-row bump/toggle/remove with initial labels.
    for label in [
        "\"add\"",
        "\"bump1:0\"",
        "\"bump2:0\"",
        "\"toggle-1\"",
        "\"remove-2\"",
    ] {
        assert!(log.contains(label), "expected button {label}:\n{log}");
    }

    // Rows render derived done-state + label.
    assert!(log.contains("\"[ ] write tests\""), "open row:\n{log}");
    assert!(log.contains("\"[x] ship it\""), "done row:\n{log}");

    // The empty-state branch is NOT mounted (list non-empty).
    assert!(!log.contains("nothing to do"), "empty state absent:\n{log}");
}

/// Regression (glue `StaticCond` FnOnce — the new-core E0507 class): a
/// `String` local moved into a STATIC `if` branch compiles (that's the
/// core of the regression — the fixture is same-source and the old-core
/// leg checks it too) and the taken branch mounts its moved text.
#[test]
fn regression_static_if_branch_moves_string_capture() {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
        rec.clone(),
        Mode::Spliced,
    ))));
    let mut registry: Registry<Bridged> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let root = world.enter(newcore_app::app::build_static_if_moved_string);
    let _realized = world.enter(|| realize(&backend, &registry, root));
    let log = joined(&rec.take_ops());
    assert!(log.contains("\"moved-kicker\""), "taken static branch mounts:\n{log}");
    assert!(log.contains("\"static-if-body\""), "sibling text mounts:\n{log}");
}

/// The add button's authored `on_click` (captured by the backend at
/// create, fired like a real press) reads the draft, appends a keyed
/// row, and clears the draft — all through the staged-commit flush.
#[test]
fn add_button_handler_appends_a_keyed_row() {
    let (h, _) = Harness::mount();

    // Type into the controlled input (signal write + flush), then press.
    h.handle.draft.set("new item".to_string());
    h.flush();
    let ops = h.press("add");
    let log = joined(&ops);

    assert!(log.contains("\"[ ] new item\""), "new row text:\n{log}");
    assert!(log.contains("\"bump3:0\""), "new row's local state starts fresh:\n{log}");
    assert!(log.contains("insert"), "structural insert for the new row:\n{log}");
    // Existing rows are keyed-reused: their nodes are not recreated.
    assert!(!log.contains("bump1:"), "row 1 untouched:\n{log}");
    assert!(!log.contains("\"[ ] write tests\""), "row 1 text not recreated:\n{log}");

    // The handler also cleared the draft (committed after flush).
    assert_eq!(h.handle.draft.get(), "");
    assert_eq!(h.handle.next_id.get(), 4);
}

/// Toggling a row via its authored handler updates text IN PLACE: the
/// per-row `done` memo re-fires, the row is keyed-reused (no create /
/// remove), and the top-level `remaining` memo text updates too.
#[test]
fn toggle_handler_updates_row_in_place() {
    let (h, _) = Harness::mount();
    let ops = h.press("toggle-1");
    let log = joined(&ops);

    assert!(log.contains("update_text") && log.contains("\"[x] write tests\""), "{log}");
    assert!(log.contains("\"0 left\""), "remaining memo propagated:\n{log}");
    assert!(!log.contains("create"), "in-place update, no rebuild:\n{log}");
    assert!(!log.contains("remove_child"), "no structural churn:\n{log}");
}

/// Removing a row drops exactly that row's subtree (structural
/// remove_child) and keeps the sibling untouched.
#[test]
fn remove_handler_unmounts_only_that_row() {
    let (h, _) = Harness::mount();
    let ops = h.press("remove-2");
    let log = joined(&ops);

    assert!(log.contains("remove_child"), "row unmount:\n{log}");
    assert!(!log.contains("create"), "no rebuilds of survivors:\n{log}");
    assert_eq!(h.handle.todos.get().len(), 1);
    // remaining is unchanged (removed row was done): no "left" update.
    assert!(!log.contains("left\""), "remaining unchanged:\n{log}");
}

/// Row-local state (the `taps` signal inside `TodoRow`) survives list
/// edits elsewhere — the keyed driver reuses the live row subtree, so
/// the bumped label persists and is NOT re-created at 0.
#[test]
fn row_local_state_survives_keyed_reconcile() {
    let (h, _) = Harness::mount();

    // Bump row 1's local counter twice (via its real handler).
    let ops = h.press("bump1:0");
    assert!(joined(&ops).contains("\"bump1:1\""), "label re-bound:\n{:?}", ops);
    let fire = h.backend.borrow().0.button_action("bump1:0"); // creation-time label
    fire();
    h.world.flush();

    // Edit the list elsewhere: add a new todo.
    h.handle.draft.set("later".to_string());
    h.flush();
    let ops = h.press("add");
    let log = joined(&ops);

    // The new row mounts; row 1 keeps its live subtree — its bumped
    // label is not reset (no create with a bump1 label at all).
    assert!(log.contains("\"bump3:0\""), "new row mounts fresh:\n{log}");
    assert!(!log.contains("bump1:"), "row 1 not rebuilt:\n{log}");

    // And the live label is still the bumped one: bump again and check
    // the next update goes 2 -> 3.
    let fire = h.backend.borrow().0.button_action("bump1:0");
    fire();
    let ops = h.flush();
    assert!(joined(&ops).contains("\"bump1:3\""), "state persisted:\n{:?}", ops);
}

/// The reactive empty-state `if` swaps branches when the list empties,
/// and back again when a row is added — the guarded Dyn hole: branch
/// swaps only when the predicate's VALUE changes.
#[test]
fn reactive_if_swaps_to_empty_state_and_back() {
    let (h, _) = Harness::mount();

    h.press("remove-2");
    let ops = h.press("remove-1");
    let log = joined(&ops);
    assert!(log.contains("text \"nothing to do\""), "empty branch mounts:\n{log}");
    assert!(log.contains("remove_child"), "list branch unmounts:\n{log}");

    // Unrelated flushes do NOT rebuild the branch (guard dedup).
    h.handle.draft.set("x".to_string());
    let ops = h.flush();
    assert!(
        !joined(&ops).contains("nothing to do"),
        "guard dedup: no branch rebuild:\n{:?}",
        ops
    );

    // Adding a todo swaps back to the list branch.
    let ops = h.press("add");
    let log = joined(&ops);
    assert!(log.contains("\"[ ] x\""), "list branch remounts:\n{log}");
}

/// Static content is Const: the footer text is created once at mount
/// and never updated by any later flush.
#[test]
fn static_text_never_rebinds() {
    let (h, _) = Harness::mount();
    h.press("toggle-1");
    h.handle.draft.set("y".to_string());
    h.flush();
    let ops = h.press("add");
    assert!(
        !joined(&ops).contains("static footer"),
        "Const content must not re-emit:\n{:?}",
        ops
    );
}

/// Dropping the `Realized` is the entire unmount story: the world can
/// then flush with nothing to do (no panics from dangling bindings).
#[test]
fn drop_realized_is_unmount() {
    let (h, _) = Harness::mount();
    let Harness { rec, world, handle, _realized, .. } = h;
    drop(_realized);
    rec.take_ops();
    // Signals still writable; no bindings left to fire.
    world.enter(|| handle.draft.set("after unmount".to_string()));
    world.flush();
    assert!(rec.take_ops().is_empty(), "no ops after unmount");
}

// ===========================================================================
// P3c: the stylesheet!-styled component through the sheet engine
// ===========================================================================

/// The `stylesheet!` card mounts through the sheet engine: the `large`
/// variant resolves (padding 16 over the base 8), the background rides
/// the `color-surface` theme token, and — because the sheet declares
/// `state hovered` — the STATIC application diverts to the state
/// machine on this event-driven mock (attach_states present; the
/// static-divert regression, authored-app edition).
#[test]
fn styled_card_resolves_variant_and_token_through_the_sheet_engine() {
    let (_h, ops) = Harness::mount();
    let log = joined(&ops);
    assert!(
        log.contains("Token { name: \"color-surface\""),
        "background must reference the theme token:\n{log}"
    );
    assert!(
        log.contains("padding_top: Some(Literal(Px(16.0)))"),
        "the large variant's padding must win over the base:\n{log}"
    );
    assert_eq!(
        log.matches("attach_states").count(),
        1,
        "exactly the state-overlay-bearing card hooks the state machine:\n{log}"
    );
}

/// A theme swap re-applies the styled card: one backend `update_tokens`
/// then a re-apply — driven by the per-world theme version signal, not
/// a per-node subscription.
#[test]
fn theme_swap_reapplies_styled_card() {
    let (h, _) = Harness::mount();
    h.world.enter(|| {
        runtime_vocabulary::theme::update_tokens(&[runtime_vocabulary::glue::TokenEntry {
            name: "color-surface",
            value: runtime_vocabulary::glue::TokenValue::Color(runtime_vocabulary::glue::Color("#fefefe".into())),
        }]);
    });
    let ops = h.flush();
    let log = joined(&ops);
    assert!(
        log.contains("update_tokens [\"color-surface\"]"),
        "the swap reaches the backend:\n{log}"
    );
    assert!(
        log.contains("Token { name: \"color-surface\""),
        "the styled card re-applies on the swap:\n{log}"
    );
}

// ===========================================================================
// P3-set primitives: overlay / anchored_overlay / presence / flat_list
// (the formerly macro-deferred tags), driven through the same authored
// path: macro → glue wrappers → builders → prims → handlers → backend.
// ===========================================================================

struct DemoHarness {
    rec: Recorder,
    backend: Rc<RefCell<Bridged>>,
    world: World,
    handle: DemoHandle,
    _realized: Realized<PNode>,
}

impl DemoHarness {
    /// Mount the P3-set demo; returns the harness plus the mount-time
    /// op log. Installs the pumped scheduler first (presence anims).
    fn mount() -> (DemoHarness, Vec<String>) {
        ensure_scheduler();
        let rec = Recorder::default();
        let backend = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
            rec.clone(),
            Mode::Spliced,
        ))));
        let mut registry: Registry<Bridged> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();
        let (root, handle) = world.enter(build_demo);
        let realized = world.enter(|| realize(&backend, &registry, root));
        let mount_ops = rec.take_ops();
        (
            DemoHarness { rec, backend, world, handle, _realized: realized },
            mount_ops,
        )
    }

    fn press(&self, label: &str) -> Vec<String> {
        let fire = self.backend.borrow().0.button_action(label);
        fire();
        self.flush()
    }

    fn flush(&self) -> Vec<String> {
        self.world.flush();
        self.rec.take_ops()
    }
}

/// Mount gates everything correctly: portals absent while the `if` is
/// false, the presence placeholder mounted but its child unbuilt while
/// `present` is false, and the virtualizer created with NO rows (rows
/// are lazy — the platform window drives them).
#[test]
fn demo_mount_gates_portals_presence_and_rows() {
    let (_h, ops) = DemoHarness::mount();
    let log = joined(&ops);

    assert!(log.contains("\"open-modal\""), "trigger button:\n{log}");
    assert!(log.contains("\"toggle-toast\""), "toast button:\n{log}");
    assert!(!log.contains("portal"), "no portal while the if is false:\n{log}");
    assert!(log.contains("presence_placeholder"), "presence placeholder:\n{log}");
    assert!(!log.contains("toast body"), "presence child unbuilt while absent:\n{log}");
    assert!(log.contains("virtualizer"), "flat_list lowers to a virtualizer:\n{log}");
    assert!(!log.contains("alpha"), "rows are window-driven, none at mount:\n{log}");
}

/// The authored open handler mounts BOTH portal compositions (the
/// centered overlay with its dismissable backdrop, and the anchored
/// overlay), and the close handler releases them again.
#[test]
fn overlay_open_close_mounts_and_releases_both_portals() {
    let (h, _) = DemoHarness::mount();

    let ops = h.press("open-modal");
    let log = joined(&ops);
    assert_eq!(
        log.matches("portal target=").count(),
        2,
        "overlay + anchored_overlay each lower to one portal:\n{log}"
    );
    assert!(log.contains("pressable"), "Dismiss backdrop mounts a pressable scrim:\n{log}");
    assert!(log.contains("\"modal body\""), "overlay content:\n{log}");
    assert!(log.contains("\"close-modal\""), "overlay button:\n{log}");
    assert!(log.contains("\"anchored tip\""), "anchored content:\n{log}");

    let ops = h.press("close-modal");
    let log = joined(&ops);
    assert_eq!(
        log.matches("release_portal").count(),
        2,
        "closing releases both portals:\n{log}"
    );
    assert!(!log.contains("portal target="), "no portal re-created:\n{log}");
    // The false branch of the gating `if` swaps in the layout-neutral
    // empty placeholder — the ONLY create the close may produce.
    assert!(
        log.contains("{position: Some(Absolute)}"),
        "false branch is the absolute-positioned empty view:\n{log}"
    );
}

/// Presence enter/exit through the authored toggle: flipping on builds
/// the child and applies the enter fade (from-state now, rest on the
/// next frame); flipping off applies the exit fade and — only when the
/// exit's timer elapses — removes the still-attached child (the scene
/// retire-hook contract: exit anims run on ATTACHED nodes).
#[test]
fn presence_toggle_enter_exit_cycle() {
    let (h, _) = DemoHarness::mount();

    // Enter: child mounts, enter-from state applies, rest is queued.
    let ops = h.press("toggle-toast");
    let log = joined(&ops);
    assert!(log.contains("\"toast body\""), "presence child mounts on flip-on:\n{log}");
    assert!(log.contains("apply_presence"), "enter anim applies:\n{log}");
    pump_frames();
    let log = joined(&h.rec.take_ops());
    assert!(
        log.contains("apply_presence") && log.contains("rest"),
        "animate-to-rest fires on the next frame:\n{log}"
    );

    // Exit: the fade applies but the child STAYS attached until the
    // exit duration elapses.
    let ops = h.press("toggle-toast");
    let log = joined(&ops);
    assert!(log.contains("apply_presence"), "exit anim applies:\n{log}");
    assert!(
        !log.contains("remove_child"),
        "child stays attached while the exit runs:\n{log}"
    );
    pump_frames();
    pump_timers();
    let log = joined(&h.rec.take_ops());
    assert!(
        log.contains("remove_child"),
        "exit completion detaches and drops the child:\n{log}"
    );
}

/// The flat_list contract end to end: the platform window mounts rows
/// (detached, keyed by the authored `key`), and a data edit through the
/// authored signal notifies the backend exactly once per flush.
#[test]
fn flat_list_window_mounts_rows_and_data_edits_notify() {
    let (h, _) = DemoHarness::mount();
    let sim = h.backend.borrow().0.virt_sim(0);

    // Rows realize with the world ambient — the documented host-driver
    // contract for virtualizer callback invocation.
    h.world.enter(|| sim.set_window(0..2));
    let log = joined(&h.rec.take_ops());
    assert!(log.contains("\"alpha\""), "row 0 renders its label:\n{log}");
    assert!(log.contains("\"beta\""), "row 1 renders its label:\n{log}");
    assert!(!log.contains("\"gamma\""), "row 2 outside the window:\n{log}");
    assert!(log.contains("size=24"), "fixed_size(24.0) reaches the sim:\n{log}");

    // Reactive data: appending a row fires the data-changed effect.
    let mut rows = h.world.enter(|| h.handle.rows.get());
    rows.push(Todo { id: 4, label: "delta".to_string(), done: false });
    h.world.enter(|| h.handle.rows.set(rows));
    let log = joined(&h.flush());
    assert!(
        log.contains("virtualizer_data_changed"),
        "data edit notifies the backend:\n{log}"
    );

    // The grown window mounts ONLY the new row (keyed reuse of 0/1).
    h.world.enter(|| sim.set_window(0..4));
    let log = joined(&h.rec.take_ops());
    assert!(log.contains("\"gamma\""), "row 2 mounts when windowed in:\n{log}");
    assert!(log.contains("\"delta\""), "appended row mounts:\n{log}");
    assert!(
        !log.contains("\"alpha\""),
        "windowed-in growth keeps mounted rows (keyed reuse):\n{log}"
    );
}

/// Flipping the hover bit (as a native event source would, through the
/// captured attach_states setter) applies the `state hovered` overlay
/// (border width 4) and removes it again on release.
#[test]
fn hover_flip_applies_the_state_overlay() {
    let (h, _) = Harness::mount();
    let setter = h.backend.borrow().0.state_setter(0);
    setter(runtime_vocabulary::glue::StateBits::HOVERED, true);
    let log = joined(&h.flush());
    assert!(
        log.contains("border_top_width: Some(Literal(4.0))"),
        "hover overlay applies:\n{log}"
    );
    setter(runtime_vocabulary::glue::StateBits::HOVERED, false);
    let log = joined(&h.flush());
    assert!(
        log.contains("apply_style") && !log.contains("border_top_width"),
        "hover release restores the base digest:\n{log}"
    );
}

// ===========================================================================
// P5 identity seam: the authored `test_id = ...` anchors drive the app
// through the vocabulary robot registry — the same find/act surface the
// conformance app's robot bridge will adapt (see
// runtime_vocabulary::robot's module docs for the transport contract).
// ===========================================================================

use runtime_vocabulary::robot::{Query, Robot};

/// `test_id = ...` on the authored source registers at mount, and the
/// robot's `click` drives the REAL add handler end to end (signal
/// mutations + a structural insert on flush) — identical behavior to
/// the backend-recorded `press()` path.
#[test]
fn robot_finds_by_test_id_and_clicks_the_real_handler() {
    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();

    let input = robot
        .find(Query::test_id("draft-input"))
        .expect("text_input registered under its authored test_id");
    // type_text routes to the authored on_change (the controlled write).
    h.world.enter(|| robot.type_text(&input, "from robot").unwrap());
    h.flush();
    assert_eq!(h.handle.draft.get(), "from robot");

    let add = robot.find(Query::test_id("add-btn")).expect("button registered");
    h.world.enter(|| robot.click(&add).unwrap());
    let ops = h.flush();
    let log = joined(&ops);
    assert!(log.contains("\"[ ] from robot\""), "row mounted via robot drive:\n{log}");
    assert_eq!(h.handle.next_id.get(), 4);
}

/// Duplicate test_id across keyed rows: `find_all` returns one hit per
/// row (the toHaveCount pattern), and a keyed-row removal deregisters
/// exactly that row's entry — registration follows the row's lifetime,
/// not the list's.
#[test]
fn robot_row_affordances_count_and_deregister_with_their_rows() {
    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();

    assert_eq!(robot.find_all(Query::test_id("row-del")).len(), 2);

    // Remove row 2 through its own affordance (find is last-wins; both
    // rows' buttons are returned by find_all, so pick by clicking each
    // until the list shrinks — here: the recorded handler for clarity).
    h.press("remove-2");
    assert_eq!(
        robot.find_all(Query::test_id("row-del")).len(),
        1,
        "the removed row's registry entry must deregister with it"
    );

    // Empty the list entirely: the branch swap to the empty state drops
    // the remaining row's entries too.
    h.press("remove-1");
    assert_eq!(robot.find_all(Query::test_id("row-del")).len(), 0);
}

/// The reactive `remaining` text reports its LIVE label through the
/// registry (label_fn), so a robot assertion sees post-toggle state.
#[test]
fn robot_reads_live_reactive_label() {
    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();

    h.world.enter(|| {
        let t = robot.find(Query::test_id("remaining")).expect("text registered");
        assert_eq!(t.label.as_deref(), Some("1 left"));
    });
    h.press("toggle-1");
    h.world.enter(|| {
        let t = robot.find(Query::test_id("remaining")).unwrap();
        assert_eq!(t.label.as_deref(), Some("0 left"), "label_fn resolves live");
    });
}

/// Full-app unmount clears the registry — no stale entries survive the
/// realized subtree (regression guard at app scope; the vocabulary
/// suite pins the branch-level case).
#[test]
fn robot_registry_empties_on_app_unmount() {
    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();
    assert!(robot.find(Query::test_id("add-btn")).is_some());
    drop(h);
    assert!(
        robot.find(Query::test_id("add-btn")).is_none(),
        "unmount must deregister"
    );
    assert!(robot.elements().is_empty(), "no stale entries after unmount");
}

// ===========================================================================
// P5 robot remainder: `#[method]` components + `watch_signal` on the
// new core — driven through the same registry/bridge surfaces the
// conformance app and robot-test client use.
// ===========================================================================

/// The `#[method]` component registers its methods at mount, links to
/// its root element (the realize-time `__component_root` arm), invokes
/// by name + JSON args (writes settle through the driver env), and
/// deregisters on unmount.
#[test]
fn robot_method_component_registers_invokes_and_deregisters() {
    use runtime_vocabulary::robot::{invoke_method, list_components};

    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();
    // Driver env: queries enter the world (label_fn), actions settle
    // (invoke_method commits its staged writes before returning).
    {
        let enter_world = h.world.clone();
        let settle_world = h.world.clone();
        runtime_vocabulary::robot::install_driver_env(
            move |f| enter_world.enter(|| f()),
            move || settle_world.flush(),
        );
    }

    let comps = list_components();
    let tally = comps
        .iter()
        .find(|c| c.name == "MethodTally")
        .expect("MethodTally registered its methods at mount");
    // Element↔component link: the instance resolves to the SAME element
    // the `method-tally` test_id resolves to (root view).
    let root_el = robot.find(Query::test_id("method-tally")).expect("root registered");
    assert_eq!(
        tally.element_id,
        Some(root_el.id),
        "realize-time pending link landed on the component's root primitive"
    );

    let val = robot.find(Query::test_id("method-tally-val")).unwrap();
    assert_eq!(val.label.as_deref(), Some("tally: 5"), "mounted initial");

    invoke_method(
        tally.id,
        "bump_by",
        &runtime_vocabulary::glue::__serde_json::json!({ "n": 3 }),
    )
    .expect("bump_by(3)");
    let val = robot.find(Query::test_id("method-tally-val")).unwrap();
    assert_eq!(val.label.as_deref(), Some("tally: 8"), "invoke settled synchronously");

    invoke_method(tally.id, "reset", &runtime_vocabulary::glue::__serde_json::json!({}))
        .expect("reset()");
    let val = robot.find(Query::test_id("method-tally-val")).unwrap();
    assert_eq!(val.label.as_deref(), Some("tally: 5"));

    runtime_vocabulary::robot::clear_driver_env();
    drop(h);
    assert!(
        list_components().iter().all(|c| c.name != "MethodTally"),
        "unmount must deregister the component's methods (keepalive died with the Owned)"
    );
}

/// `watch_signal("remaining", memo)` in the authored source exposes the
/// live memo value to the watch registry (robot-test's `assert_signal`
/// rides the same reads via the `read_signal` bridge verb), and the
/// entry dies with the component scope on unmount.
#[test]
fn robot_watch_signal_reads_live_memo_and_dies_with_scope() {
    use runtime_vocabulary::glue::__serde_json::json;
    use runtime_vocabulary::robot::bridge::invoke_command;
    use runtime_vocabulary::robot::read_watched_by_name;

    let robot = Robot::new();
    robot.reset();
    let (h, _) = Harness::mount();
    {
        let enter_world = h.world.clone();
        let settle_world = h.world.clone();
        runtime_vocabulary::robot::install_driver_env(
            move |f| enter_world.enter(|| f()),
            move || settle_world.flush(),
        );
    }

    // Seed list: 2 todos, 1 undone → remaining == 1.
    assert_eq!(read_watched_by_name("remaining"), Some(json!("1")));
    // The bridge verb serves the same read (robot-test's path).
    assert_eq!(
        invoke_command("read_signal", &json!({ "name": "remaining" })).unwrap(),
        "\"1\""
    );
    let list = invoke_command("list_watched_signals", &json!({})).unwrap();
    assert!(list.contains("\"name\":\"remaining\""), "{list}");

    // Toggle the undone todo through the real handler → memo recomputes.
    h.press("toggle-1");
    assert_eq!(
        read_watched_by_name("remaining"),
        Some(json!("0")),
        "watched read sees the post-flush memo value"
    );

    runtime_vocabulary::robot::clear_driver_env();
    drop(h);
    assert_eq!(
        read_watched_by_name("remaining"),
        None,
        "watch entry must die with the component scope (a stale read would hit a freed slot)"
    );
}
