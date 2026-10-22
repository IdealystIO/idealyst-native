//! Style parity end to end: a scene realized through the dev-server
//! recorder, encoded over `wire::codec`, replayed by `dev-client` into
//! the mock backend, must style its nodes with the same rules the author
//! wrote — the properties `--local` would hand the backend directly.
//!
//! The two properties here are the ones a user reported missing in
//! runtime-server mode: a button's `align_self: FlexStart` (it stretched
//! full width instead) and a label's `text_transform: Uppercase` (it
//! rendered as typed). The exhaustive per-property round trip lives in
//! `wire/tests/style_parity.rs`; this one proves the whole recorder →
//! codec → replay path delivers them to a backend.
//!
//! The second half covers the overlay axes — interaction states,
//! breakpoints, container queries — which a browser client switches
//! itself and a native client has the sidecar resolve.

use std::rc::Rc;

use mock_backend::WireHarness;
use runtime_shared::style::{AlignSelf, TextTransform};
use runtime_shared::{
    Breakpoint, Color, StateBits, StyleApplication, StyleRules, StyleSheet, Tokenized,
};
use runtime_vocabulary::builders::{button, text, view};

#[test]
fn regression_align_self_and_text_transform_reach_the_client_backend() {
    let harness = WireHarness::mount(|| {
        view()
            .child(
                button()
                    .label("Save")
                    .on_press(|| {})
                    .style(StyleRules {
                        align_self: Some(AlignSelf::FlexStart),
                        ..StyleRules::default()
                    }),
            )
            .child(text().content("shouting").style(StyleRules {
                text_transform: Some(TextTransform::Uppercase),
                ..StyleRules::default()
            }))
            .build()
    });
    let scene = harness.scene();

    let style_of = |needle: &str| {
        let id = scene.find_by_text(needle).unwrap_or_else(|| panic!("{needle} replayed"));
        scene
            .node(id)
            .and_then(|n| n.last_style.clone())
            .unwrap_or_else(|| panic!("{needle}'s style crossed the wire:\n{}", scene.dump()))
    };

    assert_eq!(
        style_of("Save").align_self,
        Some(AlignSelf::FlexStart),
        "a button's align_self must reach the client, or it stretches full width",
    );
    assert_eq!(
        style_of("shouting").text_transform,
        Some(TextTransform::Uppercase),
        "a text's text_transform must reach the client",
    );
}

// ---------------------------------------------------------------------------
// Overlay axes: interaction states, breakpoints, container queries.
// ---------------------------------------------------------------------------

fn color(hex: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(hex.to_string())))
}

/// `base`, `state hovered`, `md`, and `container (min_width: 400px)`
/// (`0x43c80000` is 400.0's bit pattern — the axis name the
/// `stylesheet!` macro emits).
fn chip() -> StyleApplication {
    StyleApplication::new(Rc::new(
        StyleSheet::new(|_| StyleRules { background: color("#101010"), ..StyleRules::default() })
            .variant("__state_hovered", "on", |_| StyleRules {
                background: color("#202020"),
                ..StyleRules::default()
            })
            .variant("__bp_md", "on", |_| StyleRules {
                align_self: Some(AlignSelf::FlexStart),
                ..StyleRules::default()
            })
            .variant("__cq_minw_43c80000", "on", |_| StyleRules {
                text_transform: Some(TextTransform::Uppercase),
                ..StyleRules::default()
            }),
    ))
}

/// A browser client switches hover / press / focus, `@media` and
/// `@container` in CSS, and its `attach_states` is a deliberate no-op.
///
/// Regression: the recorder answered `handles_states_natively() ==
/// false` whatever the client was, so the engine took the event-driven
/// path — apply the base, then wait for state reports this client never
/// sends. Hover, press and focus styling never showed in wire mode, and
/// container overlays resolved at width 0. The recorder now answers with
/// the client's capability (from its Hello) and ships every overlay.
#[test]
fn regression_a_browser_client_gets_every_overlay_axis() {
    let harness = WireHarness::mount_native_states(|| {
        view()
            .container()
            .child(button().label("Hover me").on_press(|| {}).style(chip()))
            .build()
    });
    let scene = harness.scene();
    let id = scene.find_by_text("Hover me").expect("button replayed");
    let node = scene.node(id).expect("node");

    assert_eq!(
        node.last_style.as_ref().and_then(|s| s.background.clone()),
        color("#101010"),
        "the base applies",
    );
    assert!(
        node.state_overlays.iter().any(|(bits, rules)| *bits == StateBits::HOVERED
            && rules.background == color("#202020")),
        "the hover overlay reaches the client to switch on :hover; got {:?}",
        node.state_overlays,
    );
    assert!(
        node.breakpoint_overlays.iter().any(|(bp, rules)| *bp == Breakpoint::Md
            && rules.align_self == Some(AlignSelf::FlexStart)),
        "the md overlay reaches the client for @media; got {:?}",
        node.breakpoint_overlays,
    );
    assert!(
        node.container_overlays.iter().any(|(min, rules)| *min == 400.0
            && rules.text_transform == Some(TextTransform::Uppercase)),
        "the container overlay reaches the client for @container; got {:?}",
        node.container_overlays,
    );
    let container = scene.parent_of(id).expect("button's parent");
    assert!(scene.node(container).unwrap().is_container, "mark_container crosses the wire");
}

/// A native client keeps the event-driven path: base rules only, the
/// overlays resolved in the sidecar as the client reports states.
#[test]
fn a_native_client_keeps_event_driven_states() {
    let harness = WireHarness::mount(|| {
        view().child(button().label("Tap me").on_press(|| {}).style(chip())).build()
    });
    let scene = harness.scene();
    let node = scene.node(scene.find_by_text("Tap me").unwrap()).unwrap();
    assert_eq!(node.last_style.as_ref().and_then(|s| s.background.clone()), color("#101010"));
    assert!(node.state_overlays.is_empty(), "no declarative overlays for a native client");
}

/// A state overlay keyed on several bits applies only while ALL of them
/// hold, so the client must rebuild the full bit set — two single-bit
/// overlays would light the style on hover alone.
#[test]
fn a_composite_state_overlay_keeps_all_its_bits() {
    use wire::{Command, NodeId, StyleId, WireStateBit, WireStyleRules};

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut client = dev_client::WireBackend::new(mock_backend::MockBackend::with_native_states(), tx);
    client
        .apply_batch(vec![
            Command::CreateView { id: NodeId(1), a11y: Default::default() },
            Command::RegisterStyle { id: StyleId(1), rules: WireStyleRules::default() },
            Command::RegisterStyle { id: StyleId(2), rules: WireStyleRules::default() },
            Command::ApplyStyledVariants {
                node: NodeId(1),
                base: StyleId(1),
                states: vec![(vec![WireStateBit::Hovered, WireStateBit::Focused], StyleId(2))],
                breakpoints: vec![],
                containers: vec![],
            },
            Command::Finish { root: NodeId(1) },
        ])
        .expect("replay");
    let scene = client.backend().borrow();
    let root = scene.roots()[0];
    let overlays = &scene.node(root).unwrap().state_overlays;
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].0, StateBits::HOVERED.with(StateBits::FOCUSED));
}
