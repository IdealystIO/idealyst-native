//! Demonstrates the mock-backend harness catching the classes of bug
//! that otherwise only show up as a blank emulator screen:
//!
//! - **framework-core walker** — does a real `Element` tree produce the
//!   right create/insert/finish calls?
//! - **wire codec** — do those commands survive `encode`→`decode`?
//! - **dev-client receiver** — does `WireBackend::apply_batch` rebuild
//!   the tree faithfully, including reactive `update_*` deltas?
//!
//! All in-process tests round-trip through the real `wire::codec`, so a
//! serialization regression fails here too. The socket test adds the
//! real WebSocket transport on top.

use std::time::Duration;

use mock_backend::{NodeKind, SocketHarness, WireHarness};
use runtime_core::{
    signal, Element, IntoAction, IntoTextSource, SafeAreaSides, TextSource,
};

// ---------------------------------------------------------------------------
// Tree builders (hand-rolled Elements — no `ui!`, keeps the test crate
// free of the macro/SDK dependency surface).
// ---------------------------------------------------------------------------

// `test_id` is present because this crate pins `runtime-core/robot`
// (see Cargo.toml) — that's what keeps the `Element` field set stable
// across build graphs.
fn text(s: &str) -> Element {
    Element::Text {
        source: TextSource::Static(s.to_string()),
        style: None,
        ref_fill: None,
        accessibility: Default::default(),
        test_id: None,
    }
}

fn reactive_text(f: impl Fn() -> String + 'static) -> Element {
    Element::Text {
        source: f.into_text_source(),
        style: None,
        ref_fill: None,
        accessibility: Default::default(),
        test_id: None,
    }
}

fn button(label: &str) -> Element {
    Element::Button {
        label: TextSource::Static(label.to_string()),
        on_click: (|| {}).into_action(),
        leading_icon: None,
        trailing_icon: None,
        style: None,
        ref_fill: None,
        disabled: None,
        accessibility: Default::default(),
        test_id: None,
    }
}

fn view(children: Vec<Element>) -> Element {
    Element::View {
        children,
        style: None,
        ref_fill: None,
        safe_area_sides: SafeAreaSides::NONE,
        on_touch: None,
        accessibility: Default::default(),
        test_id: None,
    }
}

// ---------------------------------------------------------------------------
// Structure: a real walker → wire → receiver reconstructs the tree.
// ---------------------------------------------------------------------------

#[test]
fn static_tree_reconstructs_structure_and_text() {
    let harness = WireHarness::mount(|| {
        view(vec![
            text("alpha"),
            button("Tap me"),
            view(vec![text("nested")]),
        ])
    });
    let scene = harness.scene();

    // Pre-order text content — proves both structure AND ordering
    // survived the walker + codec + receiver.
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

    // The root must have three children (text, button, nested view) in
    // insertion order.
    let root = scene.roots()[0];
    assert_eq!(scene.children(root).len(), 3, "tree:\n{}", scene.dump());
}

// ---------------------------------------------------------------------------
// Reactivity: a signal mutation reaches the client as an update_text.
// This is the framework-core reactive path end-to-end over the wire —
// exactly the kind of thing that silently breaks and leaves a stale or
// blank screen on device.
// ---------------------------------------------------------------------------

#[test]
fn reactive_text_update_propagates_over_wire() {
    let count = signal!(0_i32);
    let count_for_app = count;
    let mut harness = WireHarness::mount(move || {
        view(vec![reactive_text(move || format!("count: {}", count_for_app.get()))])
    });

    assert!(
        harness.scene().contains_text("count: 0"),
        "initial reactive text must render; got:\n{}",
        harness.scene().dump(),
    );

    // Mutate the signal on the dev side; the walker's Effect re-fires,
    // the recorder emits an UpdateText, and `sync` carries it across.
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
fn reactive_button_label_update_propagates_over_wire() {
    let label = signal!(0_i32);
    let label_for_app = label;
    let mut harness = WireHarness::mount(move || {
        Element::Button {
            label: (move || format!("clicked {}x", label_for_app.get())).into_text_source(),
            on_click: (|| {}).into_action(),
            leading_icon: None,
            trailing_icon: None,
            style: None,
            ref_fill: None,
            disabled: None,
            accessibility: Default::default(),
            test_id: None,
        }
    });

    assert!(harness.scene().contains_text("clicked 0x"));

    label.set(3);
    harness.sync();
    assert!(
        harness.scene().contains_text("clicked 3x"),
        "reactive button label must update via update_button_label over the wire; got:\n{}",
        harness.scene().dump(),
    );
}

// ---------------------------------------------------------------------------
// Transport fidelity: the same reconstruction, but over a real loopback
// WebSocket driven by the production `RuntimeServerShell`.
// ---------------------------------------------------------------------------

#[test]
fn socket_harness_renders_static_tree_over_real_websocket() {
    let harness = SocketHarness::mount(|| view(vec![text("over the socket")]));

    let ok = harness.pump_until(Duration::from_secs(5), |scene| {
        scene.contains_text("over the socket")
    });
    assert!(
        ok,
        "a real app must render over a real WebSocket into the mock backend; got:\n{}",
        harness.with_scene(|s| s.dump()),
    );

    harness.with_scene(|scene| {
        assert!(scene.count_kind(NodeKind::Text) >= 1);
        assert!(scene.finish_count >= 1, "Finish must have been replayed");
    });
}
