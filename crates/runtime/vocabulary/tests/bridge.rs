//! LegacyBridge delegation tests: wrap a minimal mock `Backend` and
//! exercise at least one method per Ops trait THROUGH the trait (UFCS
//! on the caps trait, or a generic bound), proving the bridge routes
//! every capability to the wrapped Backend — including the seven Host
//! structural ops — and that `LegacyBridge<MockBackend>: AllCaps`
//! holds at compile time.
//!
//! The mock overrides exactly one observable method per trait (plus
//! the Backend-required methods); every other method keeps its
//! Backend default, which is itself part of what the bridge must
//! preserve (see `default_insert_many_fans_out_through_backend`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::{AccessibilityProps, LiveRegionPriority};
use runtime_core::animation::AnimProp;
use runtime_core::assets::TypefaceId;
use runtime_core::primitives;
use runtime_core::{
    Action, Backend, BackendBatch, BatchOp, Platform, SafeAreaSides, StyleRules,
};
use runtime_scene::Host;
use runtime_vocabulary::{caps, AllCaps, LegacyBridge};

// ---------------------------------------------------------------------------
// Mock backend
// ---------------------------------------------------------------------------

struct MockBackend {
    next_id: Cell<u32>,
    calls: Rc<RefCell<Vec<String>>>,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            next_id: Cell::new(0),
            calls: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn mint(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }
}

impl Backend for MockBackend {
    type Node = u32;

    // --- required methods -------------------------------------------------
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        self.record("create_view");
        self.mint()
    }
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        self.record(format!("create_text({content})"));
        self.mint()
    }
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.record(format!("create_button({label})"));
        self.mint()
    }
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        self.record(format!("insert({parent}, {child})"));
    }
    fn update_text(&mut self, node: &Self::Node, content: &str) {
        self.record(format!("update_text({node}, {content})"));
    }
    fn clear_children(&mut self, node: &Self::Node) {
        self.record(format!("clear_children({node})"));
    }
    fn apply_style(&mut self, node: &Self::Node, _style: &Rc<StyleRules>) {
        self.record(format!("apply_style({node})"));
    }
    fn finish(&mut self, root: Self::Node) {
        self.record(format!("finish({root})"));
    }

    // --- structural (Host seam) overrides ---------------------------------
    fn create_reactive_anchor(&mut self) -> Self::Node {
        self.record("create_reactive_anchor");
        self.mint()
    }
    fn supports_child_splice(&self) -> bool {
        true
    }
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        self.record(format!("remove_child({parent}, {child})"));
    }
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        self.record(format!("insert_at({parent}, {child}, {index})"));
    }

    // --- one observable override per Ops trait ----------------------------
    fn platform(&self) -> Platform {
        Platform::Custom("mock-backend")
    }
    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        self.record(format!("mark_preserves_focus({node})"));
    }
    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.record("create_pressable");
        self.mint()
    }
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        self.record(format!("update_image_src({node}, {src})"));
    }
    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        self.record(format!("update_icon_stroke({node}, {progress})"));
    }
    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        self.record(format!("update_link_url({node}, {url})"));
    }
    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        self.record(format!("update_text_input_value({node}, {value})"));
    }
    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        self.record(format!("update_toggle_value({node}, {value})"));
    }
    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        self.record(format!("update_slider_value({node}, {value})"));
    }
    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        self.record(format!("update_activity_indicator_size({node}, {size:?})"));
    }
    fn node_scroll(&self, _node: &Self::Node) -> (f32, f32) {
        (3.0, 4.0)
    }
    fn apply_safe_area_padding(&mut self, node: &Self::Node, _sides: SafeAreaSides) {
        self.record(format!("apply_safe_area_padding({node})"));
    }
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        self.record(format!("virtualizer_data_changed({node})"));
    }
    fn release_graphics(&mut self, node: &Self::Node) {
        self.record(format!("release_graphics({node})"));
    }
    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        self.record(format!("set_portal_hidden({node}, {hidden})"));
    }
    fn apply_presence(
        &mut self,
        node: &Self::Node,
        _state: primitives::presence::PresenceState,
        _transition: Option<(u32, runtime_core::Easing)>,
    ) {
        self.record(format!("apply_presence({node})"));
    }
    fn release_navigator(&mut self, node: &Self::Node) {
        self.record(format!("release_navigator({node})"));
    }
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.record(format!("create_external({type_name})"));
        self.mint()
    }
    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        self.record(format!("attach_html_class({node}, {class})"));
    }
    fn unregister_typeface(&mut self, id: TypefaceId) {
        self.record(format!("unregister_typeface({})", id.0));
    }
    fn announce_for_accessibility(&mut self, msg: &str, _priority: LiveRegionPriority) {
        self.record(format!("announce({msg})"));
    }
    fn set_animated_f32(&mut self, node: &Self::Node, _prop: AnimProp, value: f32) {
        self.record(format!("set_animated_f32({node}, {value})"));
    }
    fn supports_screenshot(&self) -> bool {
        true
    }
    fn supports_batched_repeat(&self) -> bool {
        true
    }
    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        self.record(format!("execute_batch({})", batch.node_count));
        (0..batch.node_count).map(|_| self.mint()).collect()
    }
    fn begin_slot_capture(&mut self) {
        self.record("begin_slot_capture");
    }
}

type Bridge = LegacyBridge<MockBackend>;

fn bridge() -> (Bridge, Rc<RefCell<Vec<String>>>) {
    let mock = MockBackend::new();
    let calls = mock.calls.clone();
    (LegacyBridge(mock), calls)
}

fn a11y() -> AccessibilityProps {
    AccessibilityProps::default()
}

// ---------------------------------------------------------------------------
// Compile-time proof: the bridge satisfies the whole capability surface.
// ---------------------------------------------------------------------------

const _: () = {
    const fn assert_all_caps<T: AllCaps>() {}
    assert_all_caps::<Bridge>()
};

// ---------------------------------------------------------------------------
// Host — structural ops through the P1 seam
// ---------------------------------------------------------------------------

#[test]
fn host_structural_ops_delegate_to_backend() {
    let (mut b, calls) = bridge();

    let mut parent = <Bridge as Host>::create_anchor(&mut b);
    let child = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    <Bridge as Host>::insert(&mut b, &mut parent, child);
    <Bridge as Host>::insert_at(&mut b, &mut parent, child, 0);
    <Bridge as Host>::remove_child(&mut b, &parent, &child);
    <Bridge as Host>::clear_children(&mut b, &parent);
    assert!(<Bridge as Host>::supports_splice(&b), "mock opted into splice");

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_reactive_anchor", // Host::create_anchor → Backend::create_reactive_anchor
            "create_view",
            "insert(0, 1)",
            "insert_at(0, 1, 0)",
            "remove_child(0, 1)",
            "clear_children(0)",
        ]
    );
}

/// `Host::insert_many` must route through `Backend::insert_many` — whose
/// (un-overridden) default fans out to the mock's `insert` — proving the
/// bridge preserves Backend-default behavior rather than substituting
/// Host's own default.
#[test]
fn default_insert_many_fans_out_through_backend() {
    let (mut b, calls) = bridge();
    let mut parent = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    let c1 = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    let c2 = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    <Bridge as Host>::insert_many(&mut b, &mut parent, vec![c1, c2]);
    assert_eq!(
        calls.borrow()[3..],
        ["insert(0, 1)".to_string(), "insert(0, 2)".to_string()]
    );
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

#[test]
fn app_env_and_lifecycle_delegate() {
    let (mut b, calls) = bridge();

    // AppEnvOps: the mock's platform override must win.
    assert_eq!(
        <Bridge as caps::AppEnvOps>::platform(&b),
        Platform::Custom("mock-backend")
    );

    // LifecycleOps: finish + the un-overridden policy defaults.
    let root = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    <Bridge as caps::LifecycleOps>::finish(&mut b, root);
    assert!(!<Bridge as caps::LifecycleOps>::is_hydrating(&b));
    assert!(<Bridge as caps::LifecycleOps>::renders_lazy_chunks(&b));

    assert_eq!(*calls.borrow(), vec!["create_view", "finish(0)"]);
}

// ---------------------------------------------------------------------------
// Primitive families — one observable method per trait, via the trait.
// ---------------------------------------------------------------------------

#[test]
fn primitive_ops_delegate() {
    let (mut b, calls) = bridge();
    let node = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());

    <Bridge as caps::InputOps>::mark_preserves_focus(&mut b, &node);
    <Bridge as caps::PressableOps>::create_pressable(&mut b, Rc::new(|| {}), &a11y());
    <Bridge as caps::TextOps>::update_text(&mut b, &node, "hi");
    <Bridge as caps::ButtonOps>::create_button(
        &mut b,
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
    <Bridge as caps::ImageOps>::update_image_src(&mut b, &node, "a.png");
    <Bridge as caps::IconOps>::update_icon_stroke(&mut b, &node, 0.5);
    <Bridge as caps::LinkOps>::update_link_url(&mut b, &node, "/docs");
    <Bridge as caps::TextInputOps>::update_text_input_value(&mut b, &node, "v");
    <Bridge as caps::ToggleOps>::update_toggle_value(&mut b, &node, true);
    <Bridge as caps::SliderOps>::update_slider_value(&mut b, &node, 0.25);
    <Bridge as caps::ActivityIndicatorOps>::update_activity_indicator_size(
        &mut b,
        &node,
        primitives::activity_indicator::ActivityIndicatorSize::Small,
    );

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view",
            "mark_preserves_focus(0)",
            "create_pressable",
            "update_text(0, hi)",
            "create_button(go)",
            "update_image_src(0, a.png)",
            "update_icon_stroke(0, 0.5)",
            "update_link_url(0, /docs)",
            "update_text_input_value(0, v)",
            "update_toggle_value(0, true)",
            "update_slider_value(0, 0.25)",
            "update_activity_indicator_size(0, Small)",
        ]
    );
}

#[test]
fn scroll_safe_area_and_virtualizer_delegate() {
    let (mut b, calls) = bridge();
    let node = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());

    // ScrollOps: the mock's node_scroll override must win.
    assert_eq!(<Bridge as caps::ScrollOps>::node_scroll(&b, &node), (3.0, 4.0));
    <Bridge as caps::SafeAreaOps>::apply_safe_area_padding(&mut b, &node, SafeAreaSides(0b1111));
    <Bridge as caps::VirtualizerOps>::virtualizer_data_changed(&mut b, &node);

    // SafeAreaOps' scroll-view default must reach the mock's padding
    // override through the frozen fallback chain.
    <Bridge as caps::SafeAreaOps>::apply_scroll_view_safe_area_inset(
        &mut b,
        &node,
        SafeAreaSides(0b0001),
    );

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view",
            "apply_safe_area_padding(0)",
            "virtualizer_data_changed(0)",
            "apply_safe_area_padding(0)",
        ]
    );
}

#[test]
fn overlay_navigator_external_ops_delegate() {
    let (mut b, calls) = bridge();
    let node = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());

    <Bridge as caps::GraphicsOps>::release_graphics(&mut b, &node);
    <Bridge as caps::PortalOps>::set_portal_hidden(&mut b, &node, true);
    <Bridge as caps::PresenceOps>::apply_presence(
        &mut b,
        &node,
        primitives::presence::PresenceState::rest(),
        None,
    );
    <Bridge as caps::NavigatorOps>::release_navigator(&mut b, &node);

    struct FakeExternal;
    let payload: Rc<dyn std::any::Any> = Rc::new(());
    <Bridge as caps::ExternalOps>::create_external(
        &mut b,
        std::any::TypeId::of::<FakeExternal>(),
        "FakeExternal",
        &payload,
        &a11y(),
    );
    <Bridge as caps::DocumentOps>::attach_html_class(&b, &node, "idealyst-nav-root");

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view",
            "release_graphics(0)",
            "set_portal_hidden(0, true)",
            "apply_presence(0)",
            "release_navigator(0)",
            "create_external(FakeExternal)",
            "attach_html_class(0, idealyst-nav-root)",
        ]
    );
}

#[test]
fn style_asset_a11y_animation_ops_delegate() {
    let (mut b, calls) = bridge();
    let node = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());

    <Bridge as caps::StyleOps>::apply_style(&mut b, &node, &Rc::new(StyleRules::default()));
    <Bridge as caps::AssetOps>::unregister_typeface(&mut b, TypefaceId(7));
    <Bridge as caps::A11yOps>::announce_for_accessibility(&mut b, "Saved", LiveRegionPriority::Polite);
    <Bridge as caps::AnimationOps>::set_animated_f32(&mut b, &node, AnimProp::Opacity, 0.5);

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view",
            "apply_style(0)",
            "unregister_typeface(7)",
            "announce(Saved)",
            "set_animated_f32(0, 0.5)",
        ]
    );
}

#[test]
fn introspection_batch_and_wire_ops_delegate() {
    let (mut b, calls) = bridge();

    // IntrospectionOps: the mock's capability override must win over
    // the trait default (`false`).
    assert!(<Bridge as caps::IntrospectionOps>::supports_screenshot(&b));

    // BatchOps: execute_batch_with_attach's Backend default must run
    // the mock's execute_batch override, then fan the attach through
    // insert (Backend::insert_many default).
    assert!(<Bridge as caps::BatchOps>::supports_batched_repeat(&b));
    let mut parent = <Bridge as caps::ViewOps>::create_view(&mut b, &a11y());
    let mut batch = BackendBatch::with_capacity(2, 0);
    let id_a = batch.next_id();
    let id_b = batch.next_id();
    batch.ops.push(BatchOp::CreateView { local_id: id_a });
    batch.ops.push(BatchOp::CreateView { local_id: id_b });
    let nodes =
        <Bridge as caps::BatchOps>::execute_batch_with_attach(&mut b, batch, &mut parent, &[id_b]);
    assert_eq!(nodes.len(), 2);

    <Bridge as caps::WireBindingOps>::begin_slot_capture(&mut b);

    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view".to_string(),
            "execute_batch(2)".to_string(),
            format!("insert(0, {})", nodes[id_b as usize]),
            "begin_slot_capture".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// Generic-bound composition — the P2b handler shape.
// ---------------------------------------------------------------------------

/// A vocabulary handler will bound on exactly the capabilities it
/// needs; prove such a bound composes over the bridge and the node
/// types unify through the shared Host supertrait.
fn mount_styled_view<H: caps::ViewOps + caps::StyleOps + caps::TextOps>(h: &mut H) -> H::Node {
    let mut view = h.create_view(&AccessibilityProps::default());
    let text = h.create_text("hello", &AccessibilityProps::default());
    h.apply_style(&view, &Rc::new(StyleRules::default()));
    h.insert(&mut view, text);
    view
}

#[test]
fn generic_handler_bounds_compose_over_the_bridge() {
    let (mut b, calls) = bridge();
    let root = mount_styled_view(&mut b);
    assert_eq!(root, 0);
    assert_eq!(
        *calls.borrow(),
        vec![
            "create_view",
            "create_text(hello)",
            "apply_style(0)",
            "insert(0, 1)",
        ]
    );
}
