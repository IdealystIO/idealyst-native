//! A headless, queryable platform backend (`runtime_scene::Host` + the
//! 30 `runtime_vocabulary::caps::*Ops` traits) plus over-the-wire test
//! harnesses.
//!
//! The point: exercise the **runtime-server / hot-reload pipeline**
//! without a real device. A bug in the runtime, the `wire` codec, or the
//! `dev-client` receiver normally only shows up as a blank iOS/Android
//! screen — impossible to unit-test. [`MockBackend`] is a stand-in
//! platform backend that reconstructs a queryable scene tree from the
//! commands it's told to apply, so those bugs surface as a wrong/missing
//! node in an assertion instead.
//!
//! Two harnesses tie it to the real pipeline:
//!
//! - [`WireHarness`] (in-process): realizes a scene against the
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

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::{Color, StateBits, StyleRules};
use runtime_scene::Host;
use runtime_vocabulary::caps;
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
    /// The most recent rules `apply_style` landed on this node, so tests
    /// can assert WHAT was styled, not just that something was.
    pub last_style: Option<Rc<StyleRules>>,
    /// Per-frame animated writes (`set_animated_*`), as
    /// `("{prop:?}", value)`. Lets animation-over-wire tests assert that
    /// tween deltas arrive.
    pub animated: Vec<(String, f32)>,
    /// Latest safe-area opt-in applied to this node (`.safe_area(sides)`),
    /// and how many times it's been (re)applied. Lets tests assert the
    /// opt-in crossed the wire AND that a device-insets change re-applies.
    pub safe_area_sides: Option<runtime_shared::SafeAreaSides>,
    pub safe_area_apply_count: u32,
    /// Scroll offset written via `Backend::set_node_scroll` (read back by
    /// `node_scroll`). The mock treats every node as scrollable so
    /// navigator URL-sync scroll snapshot/restore is testable headlessly.
    pub scroll: (f32, f32),
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
            last_style: None,
            animated: Vec::new(),
            safe_area_sides: None,
            safe_area_apply_count: 0,
            scroll: (0.0, 0.0),
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

    /// Id of the first node whose text equals `needle`. Test helper for
    /// locating a screen's content node before walking to its ancestors.
    pub fn find_node_with_text(&self, needle: &str) -> Option<u64> {
        self.nodes
            .values()
            .find(|n| n.text.as_deref() == Some(needle))
            .map(|n| n.id)
    }

    /// Id of the node whose child list contains `id` (the rendered
    /// parent). Linear scan — fine at test scale.
    pub fn parent_of(&self, id: u64) -> Option<u64> {
        self.nodes
            .values()
            .find(|n| n.children.contains(&id))
            .map(|n| n.id)
    }

    /// The first node (if any) that had a safe-area opt-in applied, as
    /// `(sides, apply_count)`. Tests opt in on exactly one node, so this
    /// is unambiguous; `None` means the opt-in never reached the client.
    pub fn safe_area_applied(&self) -> Option<(runtime_shared::SafeAreaSides, u32)> {
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

    /// Whether any node's most recent `apply_style` rules satisfy `pred` —
    /// lets tests assert WHAT was styled (e.g. a navigator root's default
    /// fill sizing) without reaching into the node map.
    pub fn any_node_styled(&self, pred: impl Fn(&StyleRules) -> bool) -> bool {
        self.nodes
            .values()
            .any(|n| n.last_style.as_deref().map(&pred).unwrap_or(false))
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

impl Host for MockBackend {
    type Node = u64;

    fn create_anchor(&mut self) -> u64 {
        self.mint(NodeKind::ReactiveAnchor)
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
    fn supports_splice(&self) -> bool {
        true
    }

}

impl caps::ActivityIndicatorOps for MockBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::ActivityIndicator);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::AnimationOps for MockBackend {
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

}

impl caps::ButtonOps for MockBackend {
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_shared::Action,
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Button);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(label.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn update_button_label(&mut self, node: &u64, label: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(label.to_string());
        }
    }

}

impl caps::ExternalOps for MockBackend {
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

}

impl caps::GraphicsOps for MockBackend {
    fn create_graphics(
        &mut self,
        _on_ready: runtime_shared::primitives::graphics::OnReady,
        _on_resize: runtime_shared::primitives::graphics::OnResize,
        _on_lost: runtime_shared::primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Graphics);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::IconOps for MockBackend {
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Icon);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::ImageOps for MockBackend {
    fn create_image(&mut self, src: &str, _alt: Option<&str>, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Image);
        let n = self.nodes.get_mut(&id).unwrap();
        n.image_src = Some(src.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn update_image_src(&mut self, node: &u64, src: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.image_src = Some(src.to_string());
        }
    }

}

impl caps::LifecycleOps for MockBackend {
    // ----- lifecycle ------------------------------------------------------

    fn finish(&mut self, root: u64) {
        self.finish_count += 1;
        if !self.roots.contains(&root) {
            self.roots.push(root);
        }
    }

}

impl caps::LinkOps for MockBackend {
    fn create_link(
        &mut self,
        _config: runtime_shared::primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Link);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::PortalOps for MockBackend {
    fn create_portal(
        &mut self,
        _target: runtime_shared::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::Portal);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::PressableOps for MockBackend {
    fn create_pressable(&mut self, _on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Pressable);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}

impl caps::SafeAreaOps for MockBackend {
    fn apply_safe_area_padding(&mut self, node: &u64, sides: runtime_shared::SafeAreaSides) {
        if let Some(n) = self.node_mut(*node) {
            n.safe_area_sides = Some(sides);
            n.safe_area_apply_count += 1;
        }
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &u64, sides: runtime_shared::SafeAreaSides) {
        if let Some(n) = self.node_mut(*node) {
            n.safe_area_sides = Some(sides);
            n.safe_area_apply_count += 1;
        }
    }

}

impl caps::ScrollOps for MockBackend {
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

    // ----- scroll ---------------------------------------------------------

    fn node_scroll(&self, node: &u64) -> (f32, f32) {
        self.nodes.get(node).map(|n| n.scroll).unwrap_or((0.0, 0.0))
    }

    fn set_node_scroll(&mut self, node: &u64, x: f32, y: f32) {
        if let Some(n) = self.node_mut(*node) {
            n.scroll = (x, y);
        }
    }

}

impl caps::SliderOps for MockBackend {
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

    fn update_slider_value(&mut self, node: &u64, value: f32) {
        if let Some(n) = self.node_mut(*node) {
            n.slider_value = Some(value);
        }
    }

}

impl caps::StyleOps for MockBackend {
    // ----- style + animation ----------------------------------------------

    fn apply_style(&mut self, node: &u64, style: &Rc<StyleRules>) {
        if let Some(n) = self.node_mut(*node) {
            n.styles_applied += 1;
            n.last_style = Some(style.clone());
        }
    }

    fn apply_styled_states(
        &mut self,
        node: &u64,
        base: &Rc<StyleRules>,
        _overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        if let Some(n) = self.node_mut(*node) {
            n.styles_applied += 1;
            n.last_style = Some(base.clone());
        }
    }

}

impl caps::TextInputOps for MockBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
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
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> u64 {
        let id = self.mint(NodeKind::TextArea);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(initial_value.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    fn update_text_input_value(&mut self, node: &u64, value: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(value.to_string());
        }
    }

    fn update_text_input_secure(&mut self, node: &u64, secure: bool) {
        if let Some(n) = self.node_mut(*node) {
            n.secure = secure;
        }
    }

    fn update_text_area_value(&mut self, node: &u64, value: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(value.to_string());
        }
    }

}

impl caps::TextOps for MockBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::Text);
        let n = self.nodes.get_mut(&id).unwrap();
        n.text = Some(content.to_string());
        n.a11y_label = a11y.label.clone();
        id
    }

    // ----- content updates ------------------------------------------------

    fn update_text(&mut self, node: &u64, content: &str) {
        if let Some(n) = self.node_mut(*node) {
            n.text = Some(content.to_string());
        }
    }

}

impl caps::ToggleOps for MockBackend {
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

    fn update_toggle_value(&mut self, node: &u64, value: bool) {
        if let Some(n) = self.node_mut(*node) {
            n.toggle_value = Some(value);
        }
    }

}

impl caps::ViewOps for MockBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> u64 {
        let id = self.mint(NodeKind::View);
        self.nodes.get_mut(&id).unwrap().a11y_label = a11y.label.clone();
        id
    }

}



// Capability families the mock does not model — every method takes the
// caps default (no-op / `false` / type-correct inert handle), which is
// exactly what the replayer saw before this backend was de-`Backend`-ed.
impl caps::A11yOps for MockBackend {}
impl caps::AppEnvOps for MockBackend {}
impl caps::AssetOps for MockBackend {}
impl caps::BatchOps for MockBackend {}
impl caps::DocumentOps for MockBackend {}
impl caps::InputOps for MockBackend {}
impl caps::IntrospectionOps for MockBackend {}
impl caps::NavigatorOps for MockBackend {}
impl caps::PresenceOps for MockBackend {}
impl caps::VirtualizerOps for MockBackend {}
impl caps::WireBindingOps for MockBackend {}

// ---------------------------------------------------------------------------
// In-process wire harness
// ---------------------------------------------------------------------------

/// Mounts a scene tree (`runtime_scene::Element`) through
/// `dev_server::newcore::SceneSession` — per-session `World`,
/// `runtime_vocabulary::register_builtins`, `realize` — against a
/// `WireRecordingBackend`, ships the recorded commands through the real
/// `wire::codec` (JSON encode→decode, so serialization bugs surface),
/// and replays them into a `WireBackend<MockBackend>`. All in-process
/// and synchronous: the closest thing to "run the app on a device and
/// look at the screen" that a unit test can do.
///
/// The wire protocol is the compatibility contract, so the assertion
/// surface (`scene()`, `sync()`) is stated in terms of the reconstructed
/// CLIENT tree, never the recorder's internals.
pub struct WireHarness {
    // Drop order: client/recorder first, the scene session (world +
    // realized tree) last — cleanups fire against a live world.
    client: WireBackend<MockBackend>,
    recorder: WireRecordingBackend,
    _session: dev_server::newcore::SceneSession,
    _outbound_rx: mpsc::Receiver<AppToDev>,
}

impl WireHarness {
    /// Mount `app`'s scene and perform the initial realize → wire →
    /// replay pass. The closure runs inside the session world's
    /// `enter`, so free `runtime_world::signal()` calls work.
    pub fn mount<F>(app: F) -> Self
    where
        F: FnOnce() -> runtime_scene::Element + 'static,
    {
        // Same scheduler the sidecar installs — deferred microtasks
        // (navigator chrome et al.) queue instead of re-entering the
        // recorder borrow, and `drain_commands` flushes them.
        dev_server::scheduler::install();

        let recorder = WireRecordingBackend::new();
        let session = dev_server::newcore::SceneSession::mount(&recorder, |_r| {}, app);

        let (tx, rx) = mpsc::channel();
        let client = WireBackend::new(MockBackend::new(), tx);

        let mut h = Self {
            client,
            recorder,
            _session: session,
            _outbound_rx: rx,
        };
        h.sync();
        h
    }

    /// Like [`mount`](Self::mount) but runs `register` against the
    /// session's `runtime_scene::Registry` before realize — the seam an
    /// app/SDK uses to add its own scene handlers, exactly as the
    /// sidecar's `register_scene_extensions_recorder` does.
    pub fn mount_with<S, F>(register: S, app: F) -> Self
    where
        S: FnOnce(&mut dev_server::newcore::SceneRegistry),
        F: FnOnce() -> runtime_scene::Element + 'static,
    {
        dev_server::scheduler::install();

        let recorder = WireRecordingBackend::new();
        let session = dev_server::newcore::SceneSession::mount(&recorder, register, app);
        // Fire deferred chrome (navigator sidebars et al., built past the
        // mount borrow via a microtask) before the first sync.
        recorder.tick_animations(std::time::Duration::from_millis(16));

        let (tx, rx) = mpsc::channel();
        let client = WireBackend::new(MockBackend::new(), tx);

        let mut h = Self {
            client,
            recorder,
            _session: session,
            _outbound_rx: rx,
        };
        h.sync();
        h
    }

    /// Tick the recorder's deferred scheduler (deadlines / raf loops),
    /// then [`sync`](Self::sync). Use when an interaction schedules
    /// deferred work (e.g. a navigator swap that defers chrome), or when
    /// a client-side effect must re-run. Returns the number of commands
    /// applied during the follow-up sync.
    pub fn tick_and_sync(&mut self) -> usize {
        self.recorder
            .tick_animations(std::time::Duration::from_millis(16));
        self.sync()
    }

    /// The session's world — for tests that need `enter` (creating
    /// signals in the app's world up front, driving a flush by hand).
    pub fn world(&self) -> &runtime_world::World {
        self._session.world()
    }

    /// Commit pending world work (`World::flush` — the sidecar's
    /// after-event commit), then drain + codec-round-trip + replay.
    /// Returns the number of commands applied.
    pub fn sync(&mut self) -> usize {
        // World signals have no ambient flush driver in a test process
        // — this is the same explicit commit `sidecar::run_newcore`
        // performs after every dispatched event.
        self._session.flush();
        let cmds = self.recorder.drain_commands();
        let n = cmds.len();
        if n == 0 {
            return 0;
        }
        let bytes = wire::codec::encode(&DevToApp::Commands(cmds)).expect("wire encode");
        match wire::codec::decode::<DevToApp>(&bytes).expect("wire decode") {
            DevToApp::Commands(c) => self.client.apply_batch(c).expect("replay into MockBackend"),
            other => panic!("expected DevToApp::Commands, got {other:?}"),
        }
        n
    }

    /// Canonical catch-up command stream for the CURRENT scene (the
    /// recorder's `SceneModel::snapshot_commands`) — what a
    /// late-joining client would receive. Compared against the frozen
    /// wire snapshot in `tests/goldens/` by the wire-behavior gate.
    pub fn snapshot(&self) -> Vec<wire::Command> {
        self.recorder.snapshot()
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
            F: FnOnce() -> runtime_scene::Element + Send + 'static,
        {
            let port = pick_free_port();
            let addr = format!("127.0.0.1:{port}");
            let url = format!("ws://{addr}");

            let addr_for_thread = addr.clone();
            thread::spawn(move || {
                let recorder = WireRecordingBackend::new();
                // Keep the session (world + realized tree) alive for the
                // server's lifetime; the serve loop below never returns.
                let session =
                    dev_server::newcore::SceneSession::mount(&recorder, |_r| {}, app);
                std::mem::forget(session);
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
        F: FnOnce() -> runtime_scene::Element + 'static,
    {
        let recorder = WireRecordingBackend::new();
        // Hold the session across the drain so realize-time effects that
        // emit initial commands have fired.
        let _session = dev_server::newcore::SceneSession::mount(&recorder, |_r| {}, app);
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
