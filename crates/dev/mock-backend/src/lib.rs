//! A headless, queryable [`Backend`] plus over-the-wire test harnesses.
//!
//! The point: exercise the **runtime-server / hot-reload pipeline**
//! without a real device. A bug in the framework-core walker, the
//! `wire` codec, or the `dev-client` receiver normally only shows up as
//! a blank iOS/Android screen — impossible to unit-test. [`MockBackend`]
//! is a stand-in platform backend that reconstructs a queryable scene
//! tree from the commands it's told to apply, so those bugs surface as a
//! wrong/missing node in an assertion instead.
//!
//! Two harnesses tie it to the real pipeline:
//!
//! - [`WireHarness`] (in-process): mounts a real app against the
//!   dev-side [`WireRecordingBackend`], ships the recorded commands
//!   through the **real `wire::codec`** (JSON encode→decode, so
//!   serialization bugs surface), and replays them into a
//!   `WireBackend<MockBackend>`. `sync()` propagates reactive deltas
//!   after a signal mutation, so you can assert that a `signal.set(...)`
//!   reaches the client as the right `update_*` call. Deterministic and
//!   fast — no socket, no threads.
//!
//! - [`SocketHarness`] (real loopback WebSocket): mounts the app on a
//!   real `dev_server::serve` loop and connects a real
//!   `RuntimeServerShell<MockBackend>` over `ws://`. Lower-level
//!   transport fidelity (Hello exchange, snapshot, worker thread); use
//!   it to pin the transport, and `WireHarness` for reactive logic.
//!
//! ```ignore
//! let h = WireHarness::mount(|| ui! { view { text("hello") } });
//! assert!(h.scene().contains_text("hello"));
//! ```

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::animation::AnimProp;
use runtime_core::{Backend, Color, Element, Owner, StateBits, StyleRules};
use wire::{AppToDev, DevToApp};

use dev_client::WireBackend;
use dev_server::WireRecordingBackend;

// ---------------------------------------------------------------------------
// Scene model
// ---------------------------------------------------------------------------

/// The kind of primitive a [`MockNode`] represents. Mirrors the
/// `create_*` calls the `dev-client` receiver makes during replay.
/// `External` collapses to `View` because the receiver replays
/// `CreateExternal` as `create_view` (the platform overlay is a host
/// concern the mock doesn't model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    View,
    Text,
    Button,
    Pressable,
    ReactiveAnchor,
    Image,
    Icon,
    TextInput,
    TextArea,
    Toggle,
    Slider,
    ScrollView,
    ActivityIndicator,
    Link,
    Portal,
    Graphics,
}

/// One reconstructed node in the mock scene.
#[derive(Debug, Clone)]
pub struct MockNode {
    pub id: u64,
    pub kind: NodeKind,
    /// Textual content: a `Text`'s string, a `Button`'s label, a
    /// `TextInput`/`TextArea`'s value. `None` for structural nodes.
    pub text: Option<String>,
    /// `accessibility.label` captured at create time (the receiver
    /// carries it over the wire). Useful for finding nodes that have no
    /// visible text.
    pub a11y_label: Option<String>,
    /// Toggle on/off, if this is a `Toggle`.
    pub toggle_value: Option<bool>,
    /// Password-masking flag captured at create time for a
    /// `TextInput`. Lets tests assert `secure` crossed the wire.
    pub secure: bool,
    /// Slider value, if this is a `Slider`.
    pub slider_value: Option<f32>,
    /// Image `src`, if this is an `Image`.
    pub image_src: Option<String>,
    /// Children in insertion order (the rendered child list).
    pub children: Vec<u64>,
    /// How many times `apply_style` / `apply_styled_states` landed on
    /// this node — a cheap proxy for "did styling reach the client."
    pub styles_applied: u32,
    /// Per-frame animated writes (`set_animated_*`), as
    /// `("{prop:?}", value)`. Lets animation-over-wire tests assert that
    /// tween deltas arrive.
    pub animated: Vec<(String, f32)>,
    /// Latest safe-area opt-in applied to this node (`.safe_area(sides)`),
    /// and how many times it's been (re)applied. Lets tests assert the
    /// opt-in crossed the wire AND that a device-insets change re-applies.
    pub safe_area_sides: Option<runtime_core::SafeAreaSides>,
    pub safe_area_apply_count: u32,
}

impl MockNode {
    fn new(id: u64, kind: NodeKind) -> Self {
        Self {
            id,
            kind,
            text: None,
            a11y_label: None,
            toggle_value: None,
            secure: false,
            slider_value: None,
            image_src: None,
            children: Vec::new(),
            styles_applied: 0,
            animated: Vec::new(),
            safe_area_sides: None,
            safe_area_apply_count: 0,
        }
    }
}

/// A headless [`Backend`] that records the structural + content calls a
/// real platform backend would receive and exposes them as a queryable
/// tree. `Node = u64` (ids minted internally; the receiver maps wire
/// `NodeId`s onto them).
#[derive(Default)]
pub struct MockBackend {
    next: u64,
    nodes: HashMap<u64, MockNode>,
    /// Roots passed to `finish`, in order, deduplicated.
    roots: Vec<u64>,
    /// Total `finish` calls — a hot-reload re-render bumps this.
    pub finish_count: usize,
    /// Registered native navigator handler factories, keyed by the SDK
    /// presentation's `TypeId` (e.g. `DrawerPresentation`). Lets the mock
    /// exercise the dev-client's NATIVE navigator reconstruction path
    /// (`create_drawer_navigator_native`) — the path real iOS/Android/web
    /// backends take — instead of only the structural fallback. Empty by
    /// default, so tests that don't register a handler still hit the
    /// fallback exactly as before.
    #[allow(clippy::type_complexity)]
    nav_factories: HashMap<
        std::any::TypeId,
        Rc<dyn Fn() -> Box<dyn runtime_core::NavigatorHandler<MockBackend>>>,
    >,
    /// Live handler instances keyed by their navigator node id, so
    /// `navigator_attach_initial` / `release_navigator` can route back to
    /// the handler that owns the node.
    #[allow(clippy::type_complexity)]
    nav_instances:
        HashMap<u64, Rc<RefCell<Box<dyn runtime_core::NavigatorHandler<MockBackend>>>>>,
    /// `Element::External` payloads the dev-client reconstructed from the
    /// wire and dispatched here, keyed by node id as `(type_name,
    /// payload)`. Lets tests assert the External-over-wire serde round-trip
    /// landed with the right concrete payload. Stored in a side map (not
    /// `MockNode`) so `MockNode` stays `Debug` (`Rc<dyn Any>` isn't).
    #[allow(clippy::type_complexity)]
    external_payloads: HashMap<u64, (String, Rc<dyn std::any::Any>)>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native navigator handler factory, keyed by the SDK
    /// presentation type `P` (mirrors `WireRecordingBackend::register_navigator`
    /// and the real backends' registries). Call this on the client's
    /// MockBackend BEFORE the first wire `sync`, alongside the SDK's
    /// `register_wire_*_factory`, so `create_navigator` finds the handler
    /// and the dev-client takes its native reconstruction path.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<MockBackend>> + 'static,
    {
        self.nav_factories
            .insert(std::any::TypeId::of::<P>(), Rc::new(factory));
    }

    fn mint(&mut self, kind: NodeKind) -> u64 {
        self.next += 1;
        let id = self.next;
        self.nodes.insert(id, MockNode::new(id, kind));
        id
    }

    fn node_mut(&mut self, id: u64) -> Option<&mut MockNode> {
        self.nodes.get_mut(&id)
    }

    // ----- Query API ------------------------------------------------------

    /// Root node ids, in `finish` order.
    pub fn roots(&self) -> &[u64] {
        &self.roots
    }

    /// Look up a node by id.
    pub fn node(&self, id: u64) -> Option<&MockNode> {
        self.nodes.get(&id)
    }

    /// The first node (if any) that had a safe-area opt-in applied, as
    /// `(sides, apply_count)`. Tests opt in on exactly one node, so this
    /// is unambiguous; `None` means the opt-in never reached the client.
    pub fn safe_area_applied(&self) -> Option<(runtime_core::SafeAreaSides, u32)> {
        self.nodes
            .values()
            .find_map(|n| n.safe_area_sides.map(|s| (s, n.safe_area_apply_count)))
    }

    /// The reconstructed `Element::External` payload for the first node
    /// whose `type_name` contains `needle` (e.g. `"CodeBlockProps"`).
    /// `None` means no External with that type reached the client with a
    /// deserialized payload — i.e. the over-the-wire serde didn't round-
    /// trip. Downcast the returned `Rc<dyn Any>` to the SDK's payload type
    /// to assert its contents.
    pub fn external_payload(&self, needle: &str) -> Option<&Rc<dyn std::any::Any>> {
        self.external_payloads
            .values()
            .find_map(|(name, payload)| name.contains(needle).then_some(payload))
    }

    /// Child ids of `id`, in render order.
    pub fn children(&self, id: u64) -> Vec<u64> {
        self.nodes.get(&id).map(|n| n.children.clone()).unwrap_or_default()
    }

    /// Total nodes currently in the map (including any not reachable
    /// from a root — useful for leak checks).
    pub fn total_nodes(&self) -> usize {
        self.nodes.len()
    }

    /// All textual content reachable from the roots, in pre-order. This
    /// is "what the user would read on screen."
    pub fn texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for &root in &self.roots {
            self.collect_texts(root, &mut out, &mut visited);
        }
        out
    }

    fn collect_texts(
        &self,
        id: u64,
        out: &mut Vec<String>,
        visited: &mut std::collections::HashSet<u64>,
    ) {
        if !visited.insert(id) {
            return;
        }
        if let Some(node) = self.nodes.get(&id) {
            if let Some(t) = &node.text {
                out.push(t.clone());
            }
            for &c in &node.children {
                self.collect_texts(c, out, visited);
            }
        }
    }

    /// Whether any reachable node carries exactly this text.
    pub fn contains_text(&self, needle: &str) -> bool {
        self.texts().iter().any(|t| t == needle)
    }

    /// First reachable node whose text equals `needle`.
    pub fn find_by_text(&self, needle: &str) -> Option<u64> {
        let mut visited = std::collections::HashSet::new();
        for &root in &self.roots {
            if let Some(id) = self.find_text_rec(root, needle, &mut visited) {
                return Some(id);
            }
        }
        None
    }

    fn find_text_rec(
        &self,
        id: u64,
        needle: &str,
        visited: &mut std::collections::HashSet<u64>,
    ) -> Option<u64> {
        if !visited.insert(id) {
            return None;
        }
        let node = self.nodes.get(&id)?;
        if node.text.as_deref() == Some(needle) {
            return Some(id);
        }
        for &c in &node.children {
            if let Some(found) = self.find_text_rec(c, needle, visited) {
                return Some(found);
            }
        }
        None
    }

    /// Count reachable-from-roots nodes of a given kind.
    pub fn count_kind(&self, kind: NodeKind) -> usize {
        let mut n = 0;
        let mut visited = std::collections::HashSet::new();
        for &root in &self.roots {
            self.count_kind_rec(root, kind, &mut n, &mut visited);
        }
        n
    }

    fn count_kind_rec(
        &self,
        id: u64,
        kind: NodeKind,
        n: &mut usize,
        visited: &mut std::collections::HashSet<u64>,
    ) {
        if !visited.insert(id) {
            return;
        }
        if let Some(node) = self.nodes.get(&id) {
            if node.kind == kind {
                *n += 1;
            }
            for &c in &node.children {
                self.count_kind_rec(c, kind, n, visited);
            }
        }
    }

    /// Render the reachable tree as an indented ASCII string. Handy in
    /// assertion failure messages.
    pub fn dump(&self) -> String {
        let mut s = String::new();
        let mut visited = std::collections::HashSet::new();
        for &root in &self.roots {
            self.dump_rec(root, 0, &mut s, &mut visited);
        }
        s
    }

    fn dump_rec(
        &self,
        id: u64,
        depth: usize,
        s: &mut String,
        visited: &mut std::collections::HashSet<u64>,
    ) {
        if !visited.insert(id) {
            return;
        }
        let Some(node) = self.nodes.get(&id) else { return };
        for _ in 0..depth {
            s.push_str("  ");
        }
        s.push_str(&format!("{:?}#{}", node.kind, node.id));
        if let Some(t) = &node.text {
            s.push_str(&format!(" {:?}", t));
        }
        s.push('\n');
        for &c in &node.children {
            self.dump_rec(c, depth + 1, s, visited);
        }
    }
}

// ---------------------------------------------------------------------------
// Backend impl — only the methods the dev-client receiver actually
// invokes during replay. Every `create_*` the receiver can call is
// implemented because the trait defaults for those panic.
// ---------------------------------------------------------------------------

impl Backend for MockBackend {
    type Node = u64;

    fn create_view(&mut self, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::View);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Text);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(content.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Button);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(label.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_pressable(&mut self, _on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Pressable);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_reactive_anchor(&mut self) -> u64 {
        self.mint(NodeKind::ReactiveAnchor)
    }

    fn create_image(&mut self, src: &str, _alt: Option<&str>, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Image);
        let n = self.nodes.get_mut(&id).unwrap();
        n.image_src = Some(src.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Icon);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::TextInput);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(initial_value.to_string());
        n.secure = secure;
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _wrap: bool,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::TextArea);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(initial_value.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Toggle);
        let n = self.nodes.get_mut(&id).unwrap();
        n.toggle_value = Some(initial_value);
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Slider);
        let n = self.nodes.get_mut(&id).unwrap();
        n.slider_value = Some(initial_value);
        n.a11y_label = a11y.label.clone();
        id
    }

    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::ScrollView);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::ActivityIndicator);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_link(
        &mut self,
        _config: runtime_core::primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Link);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Portal);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Graphics);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

    // ----- structure ------------------------------------------------------

    fn insert(&mut self, parent: &mut u64, child: u64) {
        if let Some(p) = self.node_mut(*parent) {
            if !p.children.contains(&child) {
                p.children.push(child);
            }
        }
    }

    fn insert_many(&mut self, parent: &mut u64, children: Vec<u64>) {
        for c in children {
            self.insert(parent, c);
        }
    }

    fn insert_at(&mut self, parent: &mut u64, child: u64, index: usize) {
        if let Some(p) = self.node_mut(*parent) {
            if !p.children.contains(&child) {
                let i = index.min(p.children.len());
                p.children.insert(i, child);
            }
        }
    }

    fn remove_child(&mut self, parent: &u64, child: &u64) {
        if let Some(p) = self.node_mut(*parent) {
            p.children.retain(|c| c != child);
        }
    }

    fn clear_children(&mut self, node: &u64) {
        if let Some(n) = self.node_mut(*node) {
            n.children.clear();
        }
    }

    /// Advertise the anchorless child-splice path so the receiver
    /// exercises `remove_child` / `insert_at` (keyed `for`
    /// reconciliation) against the mock instead of clear+rebuild.
    fn supports_child_splice(&self) -> bool {
        true
    }

    // ----- content updates ------------------------------------------------

    fn update_text(&mut self, node: &u64, content: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(content.to_string());
        }
    }

    fn update_button_label(&mut self, node: &u64, label: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(label.to_string());
        }
    }

    fn update_image_src(&mut self, node: &u64, src: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.image_src = Some(src.to_string());
        }
    }

    fn update_text_input_value(&mut self, node: &u64, value: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(value.to_string());
        }
    }

    fn update_text_area_value(&mut self, node: &u64, value: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(value.to_string());
        }
    }

    fn update_toggle_value(&mut self, node: &u64, value: bool) {
        if let Some(n) = self.node_mut(*node) {
            n.toggle_value = Some(value);
        }
    }

    fn update_slider_value(&mut self, node: &u64, value: f32) {
        if let Some(n) = self.node_mut(*node) {
            n.slider_value = Some(value);
        }
    }

    // ----- style + animation ----------------------------------------------

    fn apply_style(&mut self, node: &u64, _style: &Rc<StyleRules>) {
        if let Some(n) = self.node_mut(*node) {
            n.styles_applied += 1;
        }
    }

    fn apply_styled_states(
        &mut self,
        node: &u64,
        _base: &Rc<StyleRules>,
        _overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        if let Some(n) = self.node_mut(*node) {
            n.styles_applied += 1;
        }
    }

    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        // The dev-client deserialized the wire payload and dispatched here.
        // Record it so tests can assert the round-trip; render as a plain
        // view node (the mock doesn't model the SDK's native widget).
        let id = self.mint(NodeKind::View);
        if let Some(n) = self.node_mut(id) {
            n.a11y_label = a11y.label.clone();
        }
        self.external_payloads
            .insert(id, (type_name.to_string(), payload.clone()));
        id
    }

    fn apply_safe_area_padding(&mut self, node: &u64, sides: runtime_core::SafeAreaSides) {
        if let Some(n) = self.node_mut(*node) {
            n.safe_area_sides = Some(sides);
            n.safe_area_apply_count += 1;
        }
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &u64, sides: runtime_core::SafeAreaSides) {
        if let Some(n) = self.node_mut(*node) {
            n.safe_area_sides = Some(sides);
            n.safe_area_apply_count += 1;
        }
    }

    fn set_animated_f32(&mut self, node: &u64, prop: AnimProp, value: f32) {
        if let Some(n) = self.node_mut(*node) {
            n.animated.push((format!("{prop:?}"), value));
        }
    }

    fn set_animated_color(&mut self, node: &u64, prop: AnimProp, value: [f32; 4]) {
        if let Some(n) = self.node_mut(*node) {
            // Record the alpha channel as a representative scalar; the
            // assertion surface is "did an animated color write arrive,"
            // not the exact channel values.
            n.animated.push((format!("{prop:?}"), value[3]));
        }
    }

    // ----- native navigators ----------------------------------------------
    //
    // Routes `create_navigator` / `navigator_attach_initial` to a handler
    // registered via [`MockBackend::register_navigator`], so the dev-client
    // takes its native reconstruction path (the one real backends use).
    // With no handler registered, `create_navigator` falls back to a text
    // node — the same graceful fallback the recorder uses — keeping older
    // structural-path tests unaffected.

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::primitives::navigator::NavigatorHost<u64>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let factory = self.nav_factories.get(&type_id).cloned();
        let Some(factory) = factory else {
            return self.create_text(
                &format!("Navigator \"{type_name}\" not registered on the mock"),
                a11y,
            );
        };
        let mut handler = factory();
        let node = handler.init(self, host, presentation);
        self.nav_instances
            .insert(node, Rc::new(RefCell::new(handler)));
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &u64,
        screen: u64,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let handler = self.nav_instances.get(navigator).cloned();
        if let Some(handler) = handler {
            handler
                .borrow_mut()
                .attach_initial(self, screen, scope_id, options);
        }
    }

    fn release_navigator(&mut self, node: &u64) {
        if let Some(handler) = self.nav_instances.remove(node) {
            handler.borrow_mut().release(self);
        }
    }

    // ----- lifecycle ------------------------------------------------------

    fn finish(&mut self, root: u64) {
        self.finish_count += 1;
        if !self.roots.contains(&root) {
            self.roots.push(root);
        }
    }
}

// ---------------------------------------------------------------------------
// In-process wire harness
// ---------------------------------------------------------------------------

/// Mounts a real app, ships its recorded commands through the real
/// `wire::codec`, and replays them into a [`MockBackend`] — all
/// in-process and synchronous. The closest thing to "run the app on a
/// device and look at the screen" that a unit test can do.
pub struct WireHarness {
    // Order matters for Drop: the receiver and recorder can go first;
    // `_owner` tears down the reactive tree last.
    client: WireBackend<MockBackend>,
    recorder: WireRecordingBackend,
    _owner: Owner,
    _outbound_rx: mpsc::Receiver<AppToDev>,
}

impl WireHarness {
    /// Mount `app` and perform the initial render → wire → replay pass.
    /// The returned harness keeps the reactive scope alive; drop it to
    /// tear everything down.
    pub fn mount<F>(app: F) -> Self
    where
        F: FnOnce() -> Element + 'static,
    {
        let recorder = WireRecordingBackend::new();
        let backend_rc = Rc::new(RefCell::new(recorder.clone()));
        let owner = runtime_core::mount(backend_rc, app);

        let (tx, rx) = mpsc::channel();
        let client = WireBackend::new(MockBackend::new(), tx);

        let mut h = Self {
            client,
            recorder,
            _owner: owner,
            _outbound_rx: rx,
        };
        h.sync();
        h
    }

    /// Like [`mount`](Self::mount) but runs `setup(&mut recorder)` before
    /// the render — the seam for registering SDK extensions (navigator
    /// recording handlers, externals) on the recorder, exactly as the
    /// sidecar's `register_extensions` does.
    ///
    /// Also installs the sidecar scheduler and ticks once after mount, so
    /// deferred navigator chrome (e.g. a drawer's sidebar, built via a
    /// `after_ms(0)` past the walker's `create_navigator` borrow) is in
    /// the command stream before the first `sync`. Without the scheduler
    /// `after_ms` runs synchronously and would re-enter that borrow.
    pub fn mount_with<S, F>(setup: S, app: F) -> Self
    where
        S: FnOnce(&mut WireRecordingBackend),
        F: FnOnce() -> Element + 'static,
    {
        dev_server::scheduler::install();

        let mut recorder = WireRecordingBackend::new();
        setup(&mut recorder);
        let backend_rc = Rc::new(RefCell::new(recorder.clone()));
        let owner = runtime_core::mount(backend_rc, app);
        // Fire deferred nav chrome (sidebar) before the first sync.
        recorder.tick_animations(std::time::Duration::from_millis(16));

        let (tx, rx) = mpsc::channel();
        let client = WireBackend::new(MockBackend::new(), tx);

        let mut h = Self {
            client,
            recorder,
            _owner: owner,
            _outbound_rx: rx,
        };
        h.sync();
        h
    }

    /// Tick the recorder's deferred scheduler (deadlines / raf loops),
    /// then drain + replay. Use when an interaction schedules deferred
    /// work (e.g. a drawer `Select` that defers chrome). Returns the
    /// number of commands applied during the follow-up sync.
    pub fn tick_and_sync(&mut self) -> usize {
        self.recorder
            .tick_animations(std::time::Duration::from_millis(16));
        self.sync()
    }

    /// Drain whatever commands the recorder has accumulated since the
    /// last call, round-trip them through `wire::codec`, and replay into
    /// the mock. Call after mutating a signal so the reactive delta
    /// propagates to the client. Returns the number of commands applied.
    pub fn sync(&mut self) -> usize {
        let cmds = self.recorder.drain_commands();
        let n = cmds.len();
        if n == 0 {
            return 0;
        }
        // Encode→decode through the actual wire codec so a serialization
        // regression (a non-roundtrippable Command, a renamed field)
        // fails here, exactly as it would on a real socket.
        let bytes = wire::codec::encode(&DevToApp::Commands(cmds)).expect("wire encode");
        match wire::codec::decode::<DevToApp>(&bytes).expect("wire decode") {
            DevToApp::Commands(c) => self.client.apply_batch(c).expect("replay into MockBackend"),
            other => panic!("expected DevToApp::Commands, got {other:?}"),
        }
        n
    }

    /// Borrow the reconstructed scene for querying.
    pub fn scene(&self) -> Ref<'_, MockBackend> {
        self.client.backend().borrow()
    }
}

// ---------------------------------------------------------------------------
// Real-socket harness (transport fidelity)
// ---------------------------------------------------------------------------

pub use socket::SocketHarness;

mod socket {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use runtime_server_shell_native::RuntimeServerShell;

    /// Mounts the app on a real loopback `dev_server::serve` loop and
    /// connects a real [`RuntimeServerShell`]`<MockBackend>` over a
    /// WebSocket. Use it to pin the transport (Hello / snapshot / worker
    /// thread); for reactive logic prefer [`WireHarness`], since the
    /// app's signals live on the server thread here and can't be poked
    /// from the test thread.
    pub struct SocketHarness {
        shell: RuntimeServerShell<MockBackend>,
    }

    impl SocketHarness {
        /// Spin up the server, render `app` into it (single-process
        /// mode), and connect a mock-backed shell. Blocks until the
        /// server's port is up. The `app` closure runs on the server
        /// thread, so it must be `Send`.
        pub fn mount<F>(app: F) -> Self
        where
            F: FnOnce() -> Element + Send + 'static,
        {
            let port = pick_free_port();
            let addr = format!("127.0.0.1:{port}");
            let url = format!("ws://{addr}");

            let addr_for_thread = addr.clone();
            thread::spawn(move || {
                let recorder = WireRecordingBackend::new();
                let backend_rc = Rc::new(RefCell::new(recorder.clone()));
                // Keep the reactive tree alive for the server's lifetime;
                // the serve loop below never returns.
                let owner = runtime_core::mount(backend_rc, app);
                std::mem::forget(owner);
                let _ = dev_server::serve(addr_for_thread, recorder);
            });

            wait_for_port(&addr, Duration::from_secs(3));
            let shell = RuntimeServerShell::spawn(MockBackend::new(), url);
            Self { shell }
        }

        /// Drive the shell's drain loop until `pred` holds against the
        /// reconstructed scene, or the deadline elapses. Returns whether
        /// `pred` held.
        pub fn pump_until<P>(&self, timeout: Duration, pred: P) -> bool
        where
            P: Fn(&MockBackend) -> bool,
        {
            let backend = self.shell.client.borrow().backend().clone();
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                self.shell.drain();
                if pred(&backend.borrow()) {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
            }
            self.shell.drain();
            let ok = pred(&backend.borrow());
            ok
        }

        /// Snapshot of the scene (clone of the reconstructed backend's
        /// query-relevant state via a borrow). Returns the shared
        /// backend handle for direct querying.
        pub fn with_scene<R>(&self, f: impl FnOnce(&MockBackend) -> R) -> R {
            let backend = self.shell.client.borrow().backend().clone();
            let r = f(&backend.borrow());
            r
        }
    }

    fn pick_free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn wait_for_port(addr: &str, total: Duration) {
        let deadline = Instant::now() + total;
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(addr).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("server at {addr} never came up within {total:?}");
    }
}

// ---------------------------------------------------------------------------
// Headless GPU screenshots (feature = "screenshot")
//
// The headline of the mock-backend dev tool: turn a *mocked* wire
// command stream — the exact bytes a real iOS/Android/web client would
// receive — into a rasterized PNG, with no device and no window. A
// real `WgpuBackend` replays the commands (building a layout + paint
// tree) and `render-wgpu`'s offscreen `Screenshotter` rasterizes it.
// This is what lets Robot / the MCP server screenshot the app even when
// it's only mocked.
// ---------------------------------------------------------------------------

#[cfg(feature = "screenshot")]
pub use screenshot::{register_screenshot_command, screenshot_app, screenshot_commands};
#[cfg(feature = "screenshot")]
pub use headless_screenshot::Screenshotter;

#[cfg(feature = "screenshot")]
mod screenshot {
    use super::*;

    // The scene-commands → PNG bridge + the Robot `"screenshot"` verb
    // live in the `headless-screenshot` leaf crate (so `dev-server` can
    // use them too without a cycle). Re-export the command form here.
    pub use headless_screenshot::screenshot_commands;

    /// Mount an app in-process, record the commands its initial render
    /// produces, and screenshot them through the headless GPU path.
    /// One call: app → wire → GPU → PNG.
    pub fn screenshot_app<F>(width: u32, height: u32, app: F) -> Result<Vec<u8>, String>
    where
        F: FnOnce() -> Element + 'static,
    {
        let recorder = WireRecordingBackend::new();
        let backend_rc = std::rc::Rc::new(std::cell::RefCell::new(recorder.clone()));
        // Hold the owner across the drain so reactive effects that emit
        // initial commands have fired.
        let _owner = runtime_core::mount(backend_rc, app);
        let commands = recorder.drain_commands();
        screenshot_commands(width, height, commands)
    }

    /// Register a `"screenshot"` Robot-bridge verb that captures the
    /// current scene of `recorder` as a PNG. Convenience wrapper over
    /// [`headless_screenshot::register_screenshot_command`] that supplies
    /// the snapshot closure from a [`WireRecordingBackend`].
    ///
    /// Must be called on the thread that polls the bridge (the registry
    /// is thread-local).
    pub fn register_screenshot_command(recorder: WireRecordingBackend, default_size: (u32, u32)) {
        headless_screenshot::register_screenshot_command(default_size, move || recorder.snapshot());
    }
}
