//! New-core leg of the wire-behavior gate (`wire_behavior.rs` is the
//! old-core twin): the SAME logical scenes, mounted through the
//! recorder's new-core adoption (`dev_server::newcore::SceneSession` —
//! per-session `World` + `runtime_vocabulary::register_builtins` +
//! `realize`), must produce wire command streams that reconstruct the
//! SAME client tree over the real `wire::codec`.
//!
//! The wire protocol is the compatibility contract between the dev
//! server and every runtime-server client — these tests are what lets
//! `idealyst dev --web --new-core` trust that a browser replaying the
//! stream can't tell which core produced it. The cross-core test at the
//! bottom pins the strongest form: the recorder's canonical catch-up
//! snapshot for the same scene is **identical JSON** across the cores.

use mock_backend::{NewCoreWireHarness, NodeKind, WireHarness};
use runtime_scene::Element as SceneElement;
use runtime_vocabulary::builders::{button, text, view};

// ---------------------------------------------------------------------------
// Scene builders (vocabulary builders — the new-core Element form).
// ---------------------------------------------------------------------------

fn static_tree() -> SceneElement {
    view()
        .child(text().content("alpha"))
        .child(button().label("Tap me").on_press(|| {}))
        .child(view().child(text().content("nested")))
        .build()
}

// ---------------------------------------------------------------------------
// Structure: realize → wire → receiver reconstructs the tree.
// ---------------------------------------------------------------------------

#[test]
fn newcore_static_tree_reconstructs_structure_and_text() {
    let harness = NewCoreWireHarness::mount(static_tree);
    let scene = harness.scene();

    // Pre-order text content — proves both structure AND ordering
    // survived realize + codec + receiver. Same assertion set as the
    // old-core `static_tree_reconstructs_structure_and_text`.
    assert_eq!(
        scene.texts(),
        vec!["alpha".to_string(), "Tap me".to_string(), "nested".to_string()],
        "reconstructed text (pre-order) is wrong:\n{}",
        scene.dump(),
    );

    assert_eq!(scene.count_kind(NodeKind::View), 2, "outer + nested view");
    assert_eq!(scene.count_kind(NodeKind::Text), 2);
    assert_eq!(scene.count_kind(NodeKind::Button), 1);
    assert_eq!(scene.roots().len(), 1, "exactly one finished root");

    let root = scene.roots()[0];
    assert_eq!(scene.children(root).len(), 3, "tree:\n{}", scene.dump());
}

// ---------------------------------------------------------------------------
// Reactivity: a world-signal mutation reaches the client as an
// update over the wire — the new-core reactive path end-to-end.
// ---------------------------------------------------------------------------

#[test]
fn newcore_reactive_text_update_propagates_over_wire() {
    use std::cell::Cell;
    use std::rc::Rc;

    // The signal is created INSIDE the session world (the mount closure
    // runs under `World::enter`); smuggle the Copy handle out so the
    // test can write it — the kernel routes the write to the signal's
    // own world, and `sync()`'s `World::flush` commits it (the same
    // dispatch-then-flush the sidecar's event loop performs).
    let count_slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let slot_for_app = count_slot.clone();
    let mut harness = NewCoreWireHarness::mount(move || {
        let count = runtime_world::signal(0_i32);
        slot_for_app.set(Some(count));
        view()
            .child(text().content(move || format!("count: {}", count.get())))
            .build()
    });

    assert!(
        harness.scene().contains_text("count: 0"),
        "initial reactive text must render; got:\n{}",
        harness.scene().dump(),
    );

    let count = count_slot.get().expect("signal captured during mount");
    count.set(5);
    let applied = harness.sync();
    assert!(applied >= 1, "a signal change must produce at least one wire command");

    let scene = harness.scene();
    assert!(
        scene.contains_text("count: 5"),
        "reactive update must reach the client; got:\n{}",
        scene.dump(),
    );
    assert!(
        !scene.contains_text("count: 0"),
        "stale text must have been replaced, not duplicated",
    );
}

#[test]
fn newcore_reactive_button_label_update_propagates_over_wire() {
    use std::cell::Cell;
    use std::rc::Rc;

    let label_slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let slot_for_app = label_slot.clone();
    let mut harness = NewCoreWireHarness::mount(move || {
        let label = runtime_world::signal(0_i32);
        slot_for_app.set(Some(label));
        button()
            .label(move || format!("clicked {}x", label.get()))
            .on_press(|| {})
            .build()
    });

    assert!(harness.scene().contains_text("clicked 0x"));

    let label = label_slot.get().expect("signal captured during mount");
    label.set(3);
    harness.sync();
    assert!(
        harness.scene().contains_text("clicked 3x"),
        "reactive button label must update over the wire; got:\n{}",
        harness.scene().dump(),
    );
}

// ---------------------------------------------------------------------------
// THE cross-core compatibility gate: same logical scene, both cores,
// identical canonical wire snapshot. A late-joining client receives the
// recorder's `SceneModel::snapshot_commands` — if these are equal JSON,
// clients cannot distinguish the cores.
// ---------------------------------------------------------------------------

#[test]
fn newcore_snapshot_matches_old_core_for_same_scene() {
    // Old core: hand-rolled `runtime_core::Element` tree (the walker
    // form), exactly as `wire_behavior.rs` builds it.
    fn old_text(s: &str) -> runtime_core::Element {
        runtime_core::Element::Text {
            source: runtime_core::TextSource::Static(s.to_string()),
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
            test_id: None,
        }
    }
    fn old_tree() -> runtime_core::Element {
        runtime_core::Element::View {
            children: vec![
                old_text("alpha"),
                runtime_core::Element::Button {
                    label: runtime_core::TextSource::Static("Tap me".to_string()),
                    on_click: runtime_core::IntoAction::into_action(|| {}),
                    leading_icon: None,
                    trailing_icon: None,
                    style: None,
                    ref_fill: None,
                    disabled: None,
                    accessibility: Default::default(),
                    test_id: None,
                },
                runtime_core::Element::View {
                    children: vec![old_text("nested")],
                    style: None,
                    ref_fill: None,
                    safe_area_sides: runtime_core::SafeAreaSides::NONE,
                    on_touch: None,
                    on_wheel: None,
                    preserves_focus: false,
                    on_file_drop: None,
                    on_hover: None,
                    is_container: false,
                    accessibility: Default::default(),
                    test_id: None,
                },
            ],
            style: None,
            ref_fill: None,
            safe_area_sides: runtime_core::SafeAreaSides::NONE,
            on_touch: None,
            on_wheel: None,
            preserves_focus: false,
            on_file_drop: None,
            on_hover: None,
            is_container: false,
            accessibility: Default::default(),
            test_id: None,
        }
    }

    let old = WireHarness::mount(old_tree);
    let new = NewCoreWireHarness::mount(static_tree);

    let old_snap = serde_json::to_value(old.snapshot()).expect("serialize old snapshot");
    let new_snap = serde_json::to_value(new.snapshot()).expect("serialize new snapshot");
    assert_eq!(
        old_snap,
        new_snap,
        "the canonical catch-up snapshot for the same logical scene must be \
         byte-identical across the cores — a late-joining client must not be \
         able to tell which core recorded the session.\nold: {}\nnew: {}",
        serde_json::to_string_pretty(&old_snap).unwrap(),
        serde_json::to_string_pretty(&new_snap).unwrap(),
    );

    // Belt-and-braces: the reconstructed client trees agree too (this
    // stays meaningful even if snapshot canonicalization ever changes).
    assert_eq!(old.scene().texts(), new.scene().texts());
    assert_eq!(
        old.scene().count_kind(NodeKind::View),
        new.scene().count_kind(NodeKind::View),
    );
    assert_eq!(
        old.scene().count_kind(NodeKind::Button),
        new.scene().count_kind(NodeKind::Button),
    );
}
