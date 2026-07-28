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

use newcore_app::app::{build_app, AppHandle};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, PNode, Recorder};

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
