//! Per-caps conformance battery for `host_mock::HostMock` — the direct
//! successor of `tests/bridge.rs`'s role: every one of the 30
//! `caps::*Ops` traits (plus the seven `Host` structural ops) is
//! exercised at least once THROUGH the trait (UFCS or a generic bound)
//! against the native mock, so the "all 30 caps implemented directly,
//! no `Backend`, no `LegacyBridge`" claim stays compile-and-run-proven
//! after the old trait's deletion wave. (`bridge.rs` remains the
//! LegacyBridge-specific delegation proof and dies with the bridge.)
//!
//! Ordered like `bridge.rs` so coverage is auditable side by side.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::{Harness, HostMock, Node, Shared};
use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority};
use runtime_shared::animation::AnimProp;
use runtime_shared::assets::TypefaceId;
use runtime_shared::primitives;
use runtime_shared::primitives::link::LinkConfig;
use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement, ViewportRect};
use runtime_shared::{Action, BackendBatch, BatchOp, Platform, SafeAreaSides, StyleRules};
use runtime_scene::Host;
use runtime_vocabulary::{caps, AllCaps};

// ---------------------------------------------------------------------------
// Fixture: a verbose-recording mock (every capability call logged).
// ---------------------------------------------------------------------------

fn mock() -> (HostMock, Rc<Shared>) {
    let shared: Rc<Shared> = Rc::new(Shared::default());
    shared.verbose.set(true);
    (HostMock::new(shared.clone()), shared)
}

fn take(s: &Rc<Shared>) -> Vec<String> {
    std::mem::take(&mut *s.log.borrow_mut())
}

fn a11y() -> AccessibilityProps {
    AccessibilityProps::default()
}

// ---------------------------------------------------------------------------
// Compile-time proof: the mock satisfies the whole capability surface.
// ---------------------------------------------------------------------------

const _: () = {
    const fn assert_all_caps<T: AllCaps>() {}
    assert_all_caps::<HostMock>()
};

// ---------------------------------------------------------------------------
// Host — the seven structural ops
// ---------------------------------------------------------------------------

#[test]
fn host_structural_ops_record() {
    let (mut m, s) = mock();

    let mut parent = <HostMock as Host>::create_anchor(&mut m);
    let child = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    <HostMock as Host>::insert(&mut m, &mut parent, child);
    <HostMock as Host>::insert_at(&mut m, &mut parent, child, 0);
    <HostMock as Host>::remove_child(&mut m, &parent, &child);
    <HostMock as Host>::clear_children(&mut m, &parent);
    assert!(!<HostMock as Host>::supports_splice(&m), "splice off by default");
    s.splice.set(true);
    assert!(<HostMock as Host>::supports_splice(&m), "splice flag is live");

    assert_eq!(
        take(&s),
        vec![
            "create n0 anchor",
            "create n1 view",
            "insert n0 <- n1",
            "insert_at n0 <- n1 @ 0",
            "remove_child n0 -x n1",
            "clear_children n0",
        ]
    );
}

#[test]
fn host_insert_many_is_one_op_and_links_the_tree() {
    let (mut m, s) = mock();
    let mut parent = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    let c1 = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    let c2 = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    <HostMock as Host>::insert_many(&mut m, &mut parent, vec![c1, c2]);
    assert_eq!(
        take(&s)[3..],
        ["insert_many n0 <- [n1, n2]".to_string()]
    );
    assert_eq!(*s.children.borrow().get(&parent).unwrap(), vec![c1, c2]);
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

#[test]
fn app_env_and_lifecycle() {
    let (mut m, s) = mock();

    // AppEnvOps
    assert_eq!(
        <HostMock as caps::AppEnvOps>::platform(&m),
        Platform::Custom("host-mock")
    );
    <HostMock as caps::AppEnvOps>::set_app_key_handler(&mut m, None);

    // LifecycleOps: finish + the policy flags (both live Cells).
    let root = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    <HostMock as caps::LifecycleOps>::finish(&mut m, root);
    assert!(!<HostMock as caps::LifecycleOps>::is_hydrating(&m));
    assert!(<HostMock as caps::LifecycleOps>::renders_lazy_chunks(&m));
    s.hydrating.set(true);
    s.renders_lazy_chunks.set(false);
    assert!(<HostMock as caps::LifecycleOps>::is_hydrating(&m));
    assert!(!<HostMock as caps::LifecycleOps>::renders_lazy_chunks(&m));

    assert_eq!(
        take(&s),
        vec![
            "set_app_key_handler none",
            "create n0 view",
            "finish n0",
        ]
    );
}

// ---------------------------------------------------------------------------
// Primitive families — one observable method per trait, via the trait.
// ---------------------------------------------------------------------------

#[test]
fn primitive_ops_record() {
    let (mut m, s) = mock();
    let node = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());

    <HostMock as caps::InputOps>::mark_preserves_focus(&mut m, &node);
    <HostMock as caps::PressableOps>::create_pressable(&mut m, Rc::new(|| {}), &a11y());
    <HostMock as caps::TextOps>::update_text(&mut m, &node, "hi");
    <HostMock as caps::ButtonOps>::create_button(
        &mut m,
        "go",
        &Action {
            method: "noop",
            inputs: Vec::new(),
            initial: Vec::new(),
            output: None,
            fire: Rc::new(|| {}),
        },
        None,
        None,
        &a11y(),
    );
    <HostMock as caps::ButtonOps>::update_button_label(&mut m, &node, "went");
    <HostMock as caps::ImageOps>::update_image_src(&mut m, &node, "a.png");
    <HostMock as caps::IconOps>::update_icon_stroke(&mut m, &node, 0.5);
    <HostMock as caps::LinkOps>::create_link(
        &mut m,
        LinkConfig {
            route: "docs",
            url: "/docs".into(),
            external: false,
            on_activate: Rc::new(|| {}),
        },
        &a11y(),
    );
    <HostMock as caps::TextInputOps>::update_text_input_value(&mut m, &node, "v");
    <HostMock as caps::ToggleOps>::update_toggle_value(&mut m, &node, true);
    <HostMock as caps::SliderOps>::update_slider_value(&mut m, &node, 0.25);
    <HostMock as caps::ActivityIndicatorOps>::update_activity_indicator_size(
        &mut m,
        &node,
        primitives::activity_indicator::ActivityIndicatorSize::Small,
    );

    assert_eq!(
        take(&s),
        vec![
            "create n0 view",
            "mark_preserves_focus n0",
            "create n1 pressable",
            "update_text n0 \"hi\"",
            "create n2 button \"go\"",
            "update_button_label n0 \"went\"",
            "update_image_src n0 \"a.png\"",
            "update_icon_stroke n0 0.5",
            "create n3 link route=\"docs\" url=\"/docs\"",
            "update_text_input_value n0 \"v\"",
            "update_toggle_value n0 true",
            "update_slider_value n0 0.25",
            "update_activity_indicator_size n0 Small",
        ]
    );
    // The pressable's on_click and the link's on_activate were captured.
    assert_eq!(s.press_handlers.borrow().len(), 1);
    assert_eq!(s.link_activations.borrow().len(), 1);
    assert_eq!(s.button_presses.borrow().len(), 1);
}

#[test]
fn scroll_safe_area_and_virtualizer_record() {
    let (mut m, s) = mock();
    let node = <HostMock as caps::ScrollOps>::create_scroll_view(&mut m, false, None, &a11y());

    // ScrollOps round-trip: set writes the offset map the read serves.
    assert_eq!(<HostMock as caps::ScrollOps>::node_scroll(&m, &node), (0.0, 0.0));
    <HostMock as caps::ScrollOps>::set_node_scroll(&mut m, &node, 3.0, 4.0);
    assert_eq!(<HostMock as caps::ScrollOps>::node_scroll(&m, &node), (3.0, 4.0));

    <HostMock as caps::SafeAreaOps>::apply_safe_area_padding(&mut m, &node, SafeAreaSides(0b1111));
    // The scroll-view default must reach apply_safe_area_padding
    // through the frozen fallback chain.
    <HostMock as caps::SafeAreaOps>::apply_scroll_view_safe_area_inset(
        &mut m,
        &node,
        SafeAreaSides(0b0001),
    );
    <HostMock as caps::VirtualizerOps>::virtualizer_data_changed(&mut m, &node);
    <HostMock as caps::VirtualizerOps>::release_virtualizer(&mut m, &node);

    assert_eq!(
        take(&s),
        vec![
            "create n0 scroll_view",
            "set_node_scroll n0 3 4",
            "apply_safe_area_padding n0",
            "apply_safe_area_padding n0",
            "virtualizer_data_changed n0",
            "release_virtualizer n0",
        ]
    );
}

#[test]
fn overlay_navigator_external_ops_record() {
    let (mut m, s) = mock();

    <HostMock as caps::GraphicsOps>::create_graphics(
        &mut m,
        Box::new(|_| {}),
        Box::new(|_| {}),
        Box::new(|| {}),
        &a11y(),
    );
    <HostMock as caps::GraphicsOps>::release_graphics(&mut m, &0);

    let portal = <HostMock as caps::PortalOps>::create_portal(
        &mut m,
        PortalTarget::Viewport(ViewportPlacement::Center),
        None,
        false,
        &a11y(),
    );
    <HostMock as caps::PortalOps>::set_portal_hidden(&mut m, &portal, true);
    <HostMock as caps::PortalOps>::release_portal(&mut m, &portal);

    let ph = <HostMock as caps::PresenceOps>::create_presence_placeholder(&mut m, &a11y());
    <HostMock as caps::PresenceOps>::apply_presence(
        &mut m,
        &ph,
        primitives::presence::PresenceState::rest(),
        None,
    );

    <HostMock as caps::NavigatorOps>::release_navigator(&mut m, &ph);

    struct FakeExternal;
    let payload: Rc<dyn std::any::Any> = Rc::new(());
    let ext = <HostMock as caps::ExternalOps>::create_external(
        &mut m,
        std::any::TypeId::of::<FakeExternal>(),
        "FakeExternal",
        &payload,
        &a11y(),
    );
    <HostMock as caps::ExternalOps>::release_external(&mut m, &ext);

    let el = <HostMock as caps::DocumentOps>::create_element(&mut m, "pre");
    <HostMock as caps::DocumentOps>::attach_html_class(&m, &el, "idealyst-nav-root");

    assert_eq!(
        take(&s),
        vec![
            "create n0 graphics",
            "release_graphics n0",
            "create n1 portal",
            "set_portal_hidden n1 true",
            "release_portal n1",
            "create n2 presence_placeholder",
            "apply_presence n2 rest snap",
            "release_navigator n2",
            "create n3 external FakeExternal",
            "release_external n3",
            "create n4 element \"pre\"",
            "attach_html_class n4 idealyst-nav-root",
        ]
    );
    assert_eq!(s.graphics.borrow().len(), 1, "graphics closures captured");
    assert_eq!(s.portal_dismissals.borrow().len(), 1);
}

#[test]
fn style_asset_a11y_animation_ops_record() {
    let (mut m, s) = mock();
    let node = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());

    <HostMock as caps::StyleOps>::apply_style(&mut m, &node, &Rc::new(StyleRules::default()));
    <HostMock as caps::StyleOps>::attach_states(&mut m, &node, Rc::new(|_, _| {}));
    <HostMock as caps::StyleOps>::set_disabled(&mut m, &node, true);
    <HostMock as caps::StyleOps>::on_node_unstyled(&mut m, &node);
    assert_eq!(
        <HostMock as caps::StyleOps>::mint_style_class(&mut m, &Rc::new(StyleRules::default())),
        None,
        "no class model unless a mint hook is installed"
    );
    <HostMock as caps::AssetOps>::unregister_typeface(&mut m, TypefaceId(7));
    <HostMock as caps::A11yOps>::announce_for_accessibility(&mut m, "Saved", LiveRegionPriority::Polite);
    <HostMock as caps::AnimationOps>::set_animated_f32(&mut m, &node, AnimProp::Opacity, 0.5);

    assert_eq!(
        take(&s),
        vec![
            "create n0 view".to_string(),
            "apply_style n0 width=none".to_string(),
            "attach_states n0".to_string(),
            "set_disabled n0 true".to_string(),
            "on_node_unstyled n0".to_string(),
            "unregister_typeface TypefaceId(7)".to_string(),
            "announce_for_accessibility \"Saved\" Polite".to_string(),
            "set_animated_f32 n0 Opacity 0.5".to_string(),
        ]
    );
    assert_eq!(s.state_setters.borrow().len(), 1, "state setter captured");
}

#[test]
fn introspection_batch_and_wire_ops_record() {
    let (mut m, s) = mock();

    // IntrospectionOps: flags + the frames map.
    assert!(!<HostMock as caps::IntrospectionOps>::supports_screenshot(&m));
    s.screenshot.set(true);
    assert!(<HostMock as caps::IntrospectionOps>::supports_screenshot(&m));
    assert!(<HostMock as caps::IntrospectionOps>::frame(&m, &0).is_none());
    s.frames.borrow_mut().insert(
        0,
        ViewportRect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        },
    );
    assert_eq!(
        <HostMock as caps::IntrospectionOps>::frame(&m, &0).map(|r| r.width),
        Some(3.0)
    );

    // BatchOps: execute_batch_with_attach's trait default must run the
    // mock's execute_batch, then attach through Host::insert_many.
    s.batched_repeat.set(true);
    assert!(<HostMock as caps::BatchOps>::supports_batched_repeat(&m));
    let mut parent = <HostMock as caps::ViewOps>::create_view(&mut m, &a11y());
    let mut batch = BackendBatch::with_capacity(2, 0);
    let id_a = batch.next_id();
    let id_b = batch.next_id();
    batch.ops.push(BatchOp::CreateView { local_id: id_a });
    batch.ops.push(BatchOp::CreateView { local_id: id_b });
    let nodes = <HostMock as caps::BatchOps>::execute_batch_with_attach(
        &mut m,
        batch,
        &mut parent,
        &[id_b],
    );
    assert_eq!(nodes.len(), 2);

    <HostMock as caps::WireBindingOps>::begin_slot_capture(&mut m);
    <HostMock as caps::WireBindingOps>::end_slot_capture(&mut m, &parent);

    assert_eq!(
        take(&s),
        vec![
            "create n0 view".to_string(),
            "execute_batch nodes=2 ops=[cv#0 cv#1]".to_string(),
            format!("insert_many n0 <- [n{}]", nodes[id_b as usize]),
            "begin_slot_capture".to_string(),
            "end_slot_capture n0".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// Generic-bound composition — the handler shape.
// ---------------------------------------------------------------------------

/// A vocabulary handler bounds on exactly the capabilities it needs;
/// prove such a bound composes over the native mock and the node types
/// unify through the shared Host supertrait.
fn mount_styled_view<H: caps::ViewOps + caps::StyleOps + caps::TextOps>(h: &mut H) -> H::Node {
    let mut view = h.create_view(&AccessibilityProps::default());
    let text = h.create_text("hello", &AccessibilityProps::default());
    h.apply_style(&view, &Rc::new(StyleRules::default()));
    h.insert(&mut view, text);
    view
}

#[test]
fn generic_handler_bounds_compose_over_the_mock() {
    let (mut m, s) = mock();
    let root = mount_styled_view(&mut m);
    assert_eq!(root, 0);
    assert_eq!(
        take(&s),
        vec![
            "create n0 view",
            "create n1 text \"hello\"",
            "apply_style n0 width=none",
            "insert n0 <- n1",
        ]
    );
}

// ---------------------------------------------------------------------------
// Harness smoke: the registry mount path + tree snapshot helpers.
// ---------------------------------------------------------------------------

#[test]
fn harness_mounts_through_the_registry_and_snapshots_the_tree() {
    use runtime_vocabulary::builders::{text, view};

    let h = Harness::new();
    let _realized = h.mount(
        view()
            .child(text().content("hi"))
            .child(view().build())
            .build(),
    );
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view",
            "create n1 text \"hi\"",
            "insert n0 <- n1",
            "create n2 view",
            "insert n0 <- n2",
        ]
    );
    assert_eq!(h.children_of(0), vec![1, 2]);
    assert_eq!(h.kind_of(1).as_deref(), Some("text \"hi\""));
    assert_eq!(h.tree(0), "n0 view\n  n1 text \"hi\"\n  n2 view");
}

// Keep the imports honest even when the RefCell isn't otherwise used.
#[allow(dead_code)]
type _Unused = (RefCell<Vec<Node>>,);
