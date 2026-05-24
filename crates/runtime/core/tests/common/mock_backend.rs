//! `MockBackend` — a `Backend` impl that records every method
//! invocation into a shared event log.
//!
//! Tests use it as the substrate for assertions:
//!
//! ```ignore
//! let mut bk = MockBackend::new();
//! let owner = runtime_core::render(bk.handle(), view(vec![text("hi").into()]));
//! bk.assert_events(&[
//!     Event::CreateView,
//!     Event::CreateText { content: "hi".into() },
//!     Event::Insert { parent: NodeId(0), child: NodeId(1) },
//! ]);
//! ```
//!
//! Design notes:
//!
//! - `Node` is a transparent `u64` id minted monotonically by the
//!   backend. All record-keeping uses these ids; the per-primitive
//!   `Ops` impls below carry the id behind `Rc<dyn Any>` so handles
//!   (`Ref<ButtonHandle>`, etc.) can be filled the framework way.
//! - The event log is shared (`Rc<RefCell<Vec<Event>>>`) so a
//!   `bk.handle()` clone and any future inspection both see the
//!   same stream. Avoids the "the backend got moved into the
//!   walker, how do I assert on it" papercut.
//! - We deliberately implement *every* `Backend` method, including
//!   the ones with sensible defaults, so tests can assert that the
//!   framework called what we expected. Anything we don't record
//!   here is invisible to tests — a silent gap is worse than a
//!   verbose log.

#![allow(dead_code)] // exported for tests; not every test uses every variant

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    primitives, AssetId, AssetSource, AssetTag, BackendBatch, Backend, BatchOp, ButtonHandle,
    ButtonOps, Color, PressableHandle, PressableOps, StyleRules, SystemFallback, TextHandle,
    TextOps, TypefaceFace, TypefaceId, ViewHandle, ViewOps, ViewportRect,
};

// =============================================================================
// NodeId — the backend's `Node` type
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

impl NodeId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

// =============================================================================
// Event — one structured record per Backend call
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    // --- Create ---
    CreateView,
    CreateText { content: String },
    CreateButton { label: String },
    CreatePressable,
    CreateImage { src: String, alt: Option<String> },
    CreateIcon,
    CreateTextInput { placeholder: Option<String>, has_key_handler: bool },
    CreateTextArea { placeholder: Option<String>, has_key_handler: bool },
    CreateToggle { value: bool },
    CreateScrollView { horizontal: bool },
    CreateSlider { value: f32, min: f32, max: f32, step: Option<f32> },
    CreateActivityIndicator,
    CreateVirtualizer { overscan: f32, horizontal: bool },
    CreateGraphics,
    CreateNavigator { initial_route: String },
    CreateTabNavigator { initial_route: String, tabs: usize },
    CreateDrawerNavigator { initial_route: String },
    CreateLink { url: String, route: String },
    CreatePortal,
    CreateReactiveAnchor,
    CreateExternal { type_name: &'static str },

    // --- Tree mutation ---
    Insert { parent: NodeId, child: NodeId },
    InsertMany { parent: NodeId, children: Vec<NodeId> },
    ClearChildren { node: NodeId },

    // --- Update ---
    UpdateText { node: NodeId, content: String },
    UpdateButtonLabel { node: NodeId, label: String },
    UpdateImageSrc { node: NodeId, src: String },
    UpdateIconColor { node: NodeId, color: Color },
    UpdateIconStroke { node: NodeId, progress: f32 },
    AnimateIconStroke { node: NodeId, from: f32, to: f32, duration_ms: u32 },
    UpdateTextInputValue { node: NodeId, value: String },
    UpdateToggleValue { node: NodeId, value: bool },
    UpdateSliderValue { node: NodeId, value: f32 },

    // --- Style ---
    ApplyStyle { node: NodeId },
    ApplyStyledStates { node: NodeId, overlays: usize },
    RegisterStylesheet { rules: usize },
    UnregisterStylesheet { rules: usize },
    OnNodeUnstyled { node: NodeId },

    // --- Tokens / theme variables ---
    InstallThemeVariables { token_count: usize },
    UpdateTokens { token_count: usize },

    // --- Release ---
    ReleaseVirtualizer { node: NodeId },
    ReleaseGraphics { node: NodeId },
    ReleaseNavigator { node: NodeId },
    ReleaseTabNavigator { node: NodeId },
    ReleaseDrawerNavigator { node: NodeId },
    ReleasePortal { node: NodeId },
    ReleaseExternal { node: NodeId },

    // --- Assets / typefaces ---
    /// `register_asset(id, kind, source)`. `source_bytes_len` is
    /// `Some(n)` when the source is `Embedded` (the only branch
    /// `face!` produces today, after the macro switched to
    /// `include_bytes!`) and `None` for `Bundled` / `Remote`. The
    /// extension comes from the same source variant. Tests use these
    /// to assert real font bytes flowed through to the backend.
    RegisterAsset {
        id: AssetId,
        kind: AssetTag,
        source_bytes_len: Option<usize>,
        source_extension: Option<String>,
    },
    UnregisterAsset { id: AssetId, kind: AssetTag },
    RegisterTypeface { id: TypefaceId, face_count: usize },
    UnregisterTypeface { id: TypefaceId },

    // --- Lifecycle ---
    Finish { root: NodeId },

    // --- Misc ---
    /// `virtualizer_data_changed(node)` — emitted when the framework
    /// signals a virtualized list's data has been edited.
    VirtualizerDataChanged { node: NodeId },

    // --- Batched-Repeat fast path ---
    /// `execute_batch(batch)` — emitted when the walker takes the
    /// batched-Repeat path and `supports_batched_repeat` is enabled
    /// on the mock. Only fires if the test runtime opted in via
    /// [`MockBackendConfig::supports_batched_repeat`].
    ExecuteBatch {
        ops: Vec<BatchOpSummary>,
        node_count: u32,
    },
    /// `execute_batch_with_attach(batch, parent, attach_locals)` —
    /// the walker's combined fast path. `attach_locals` is the
    /// row-top `local_id` list the walker handed off for the
    /// parent-attach step.
    ExecuteBatchWithAttach {
        ops: Vec<BatchOpSummary>,
        node_count: u32,
        parent: NodeId,
        attach_locals: Vec<u32>,
    },
}

/// Clone-able shadow of [`BatchOp`] used by the [`Event::ExecuteBatch`]
/// / [`Event::ExecuteBatchWithAttach`] variants. `BatchOp` itself
/// isn't `Clone` because [`ApplyStyleStatic`](BatchOp::ApplyStyleStatic)
/// carries an `Rc<StyleRules>` we don't want to bake into the public
/// type. We snapshot the salient fields into this enum at record time
/// so test assertions can inspect them without holding the original
/// `Rc`.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchOpSummary {
    CreateView { local_id: u32 },
    CreateText { local_id: u32, content: String },
    ApplyStyleStatic { node: u32, class_name: String },
    Insert { parent: u32, child: u32 },
}

impl BatchOpSummary {
    fn from_op(op: &BatchOp) -> Self {
        match op {
            BatchOp::CreateView { local_id } => Self::CreateView { local_id: *local_id },
            BatchOp::CreateText { local_id, content } => Self::CreateText {
                local_id: *local_id,
                content: content.clone(),
            },
            BatchOp::ApplyStyleStatic {
                node, class_name, ..
            } => Self::ApplyStyleStatic {
                node: *node,
                class_name: class_name.clone(),
            },
            BatchOp::Insert { parent, child } => Self::Insert {
                parent: *parent,
                child: *child,
            },
        }
    }
}

// =============================================================================
// MockBackend
// =============================================================================

/// Tunable behaviours the mock backend can toggle for individual
/// tests. Most tests want the defaults (per-call backend ops, no
/// batched-Repeat opt-in) so existing assertions stay valid; the
/// batched-Repeat coverage tests flip
/// [`supports_batched_repeat`](Self::supports_batched_repeat) on to
/// exercise the walker's fast path.
#[derive(Clone, Copy, Debug)]
pub struct MockBackendConfig {
    /// When `true`, the mock reports `supports_batched_repeat()` =
    /// `true` to the walker, which then takes the batched-Repeat
    /// fast path for compatible row shapes and ends up calling
    /// `execute_batch_with_attach` instead of the per-row
    /// `create_*` + `apply_style` + `insert` sequence.
    pub supports_batched_repeat: bool,
}

impl Default for MockBackendConfig {
    fn default() -> Self {
        Self {
            supports_batched_repeat: false,
        }
    }
}

/// Shared event log + monotonic id state. Cloning a `MockBackendCore`
/// is cheap (Rc-clone) and gives you the same log + same id counter.
#[derive(Clone)]
pub struct MockBackendCore {
    next_id: Rc<RefCell<u64>>,
    events: Rc<RefCell<Vec<Event>>>,
    /// Registered key-down handlers keyed by node id. Lets tests call
    /// [`MockBackend::fire_key_event`] to synthesize a keydown without
    /// going through a real platform.
    pub(crate) key_handlers: Rc<RefCell<std::collections::HashMap<NodeId, runtime_core::primitives::key::KeyDownHandler>>>,
}

impl Default for MockBackendCore {
    fn default() -> Self {
        Self {
            next_id: Rc::new(RefCell::new(0)),
            events: Rc::new(RefCell::new(Vec::new())),
            key_handlers: Rc::new(RefCell::new(std::collections::HashMap::new())),
        }
    }
}

impl MockBackendCore {
    fn mint(&self) -> NodeId {
        let mut id = self.next_id.borrow_mut();
        let v = *id;
        *id += 1;
        NodeId(v)
    }

    fn record(&self, e: Event) {
        self.events.borrow_mut().push(e);
    }
}

/// The actual `Backend` type passed to `runtime_core::render`. Wraps
/// a `MockBackendCore` so cloning a backend share the same event log.
pub struct MockBackend {
    core: MockBackendCore,
    config: MockBackendConfig,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            core: MockBackendCore::default(),
            config: MockBackendConfig::default(),
        }
    }

    /// Build a mock backend with a custom config (e.g. to opt into
    /// the batched-Repeat fast path).
    pub fn with_config(config: MockBackendConfig) -> Self {
        Self {
            core: MockBackendCore::default(),
            config,
        }
    }

    /// Return a clone of the shared core for inspection.
    pub fn inspector(&self) -> MockBackendCore {
        self.core.clone()
    }

    /// All events recorded so far.
    pub fn events(&self) -> Vec<Event> {
        self.core.events.borrow().clone()
    }

    /// Clear the event log. Useful between phases of a test.
    pub fn clear_events(&self) {
        self.core.events.borrow_mut().clear();
    }

    /// Number of events recorded.
    pub fn event_count(&self) -> usize {
        self.core.events.borrow().len()
    }

    /// Assert the recorded events exactly match `expected`.
    #[track_caller]
    pub fn assert_events(&self, expected: &[Event]) {
        let actual = self.events();
        if actual != expected {
            panic!(
                "MockBackend events mismatch.\nexpected:\n{:#?}\nactual:\n{:#?}",
                expected, actual,
            );
        }
    }

    /// Assert at least one event in the log matches a predicate.
    #[track_caller]
    pub fn assert_any(&self, pred: impl Fn(&Event) -> bool) {
        if !self.core.events.borrow().iter().any(pred) {
            panic!(
                "MockBackend: no event matched predicate. Recorded events:\n{:#?}",
                self.events()
            );
        }
    }

    /// Synthesize a keydown on the registered handler for `node`.
    /// Returns the handler's [`KeyOutcome`], or `None` if no handler
    /// is registered for that node. Used by walker tests to verify
    /// the on_key_down plumbing without going through a real
    /// platform.
    pub fn fire_key_event(
        &self,
        node: NodeId,
        event: &runtime_core::primitives::key::KeyEvent,
    ) -> Option<runtime_core::primitives::key::KeyOutcome> {
        let handler = self.core.key_handlers.borrow().get(&node).cloned()?;
        Some(handler(event))
    }

    /// Count events matching a predicate.
    pub fn count_matching(&self, pred: impl Fn(&Event) -> bool) -> usize {
        self.core.events.borrow().iter().filter(|e| pred(e)).count()
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Ops impls — one ZST per primitive kind
// =============================================================================

struct MockButtonOps;
impl ButtonOps for MockButtonOps {
    fn click(&self, _node: &dyn Any) {}
    fn rect(&self, _node: &dyn Any) -> ViewportRect {
        ViewportRect::default()
    }
}

struct MockPressableOps;
impl PressableOps for MockPressableOps {
    fn click(&self, _node: &dyn Any) {}
    fn rect(&self, _node: &dyn Any) -> ViewportRect {
        ViewportRect::default()
    }
}

struct MockViewOps;
impl ViewOps for MockViewOps {}

struct MockTextOps;
impl TextOps for MockTextOps {}

// =============================================================================
// Backend impl
// =============================================================================

impl Backend for MockBackend {
    type Node = NodeId;

    // --- Required ---

    fn create_view(
        &mut self,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateView);
        id
    }

    fn create_text(
        &mut self,
        content: &str,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateText { content: content.to_string() });
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateButton { label: label.to_string() });
        id
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        self.core.record(Event::Insert { parent: *parent, child });
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        self.core.record(Event::UpdateText { node: *node, content: content.to_string() });
    }

    fn clear_children(&mut self, node: &Self::Node) {
        self.core.record(Event::ClearChildren { node: *node });
    }

    fn apply_style(&mut self, node: &Self::Node, _style: &Rc<StyleRules>) {
        self.core.record(Event::ApplyStyle { node: *node });
    }

    // --- Optional but explicitly recorded ---

    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreatePressable);
        id
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateReactiveAnchor);
        id
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        self.core.record(Event::InsertMany { parent: *parent, children });
    }

    // --- Batched-Repeat fast path ---
    //
    // `supports_batched_repeat` defaults to false (see
    // [`MockBackendConfig`]) so existing per-call walker tests keep
    // observing the granular CreateView / CreateText / Insert /
    // ApplyStyle event stream they were written against. Tests that
    // want to exercise the batched fast path opt in via
    // `MockBackend::with_config(MockBackendConfig {
    //     supports_batched_repeat: true,
    // })`.

    fn supports_batched_repeat(&self) -> bool {
        self.config.supports_batched_repeat
    }

    /// Hand back a fixed class name so the walker's batched-Repeat
    /// path doesn't bail when it asks the backend to resolve a static
    /// style. The trait default is `None` (which signals "I don't
    /// have a cache for this; please take the per-call path"), so a
    /// backend that wants `supports_batched_repeat = true` to actually
    /// fire must override this. The literal class name doesn't
    /// matter for the mock — it just needs to be `Some`.
    fn mint_style_class(&mut self, _style: &Rc<StyleRules>) -> Option<String> {
        Some("mock-class".to_string())
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        // Mint `node_count` fresh ids — the walker indexes into the
        // returned Vec by `local_id`, so the slot ordering must
        // match the batch's id-allocation order (0..node_count).
        let ops: Vec<BatchOpSummary> = batch.ops.iter().map(BatchOpSummary::from_op).collect();
        let nodes: Vec<NodeId> = (0..batch.node_count).map(|_| self.core.mint()).collect();
        self.core.record(Event::ExecuteBatch {
            ops,
            node_count: batch.node_count,
        });
        nodes
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        // Snapshot the ops + attach plan into a single event so tests
        // can assert on the combined call. The trait default impl
        // would emit `ExecuteBatch` + `InsertMany` separately; this
        // override is what the production web backend does, so
        // recording it specifically lets tests distinguish the two
        // paths.
        let ops: Vec<BatchOpSummary> = batch.ops.iter().map(BatchOpSummary::from_op).collect();
        let nodes: Vec<NodeId> = (0..batch.node_count).map(|_| self.core.mint()).collect();
        self.core.record(Event::ExecuteBatchWithAttach {
            ops,
            node_count: batch.node_count,
            parent: *parent,
            attach_locals: attach_locals.to_vec(),
        });
        nodes
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateImage {
            src: src.to_string(),
            alt: alt.map(|s| s.to_string()),
        });
        id
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        self.core.record(Event::UpdateImageSrc { node: *node, src: src.to_string() });
    }

    fn create_icon(
        &mut self,
        _data: &primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateIcon);
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        self.core.record(Event::UpdateIconColor { node: *node, color: color.clone() });
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        self.core.record(Event::UpdateIconStroke { node: *node, progress });
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        _easing: runtime_core::Easing,
        _infinite: bool,
        _autoreverses: bool,
    ) {
        self.core.record(Event::AnimateIconStroke {
            node: *node,
            from,
            to,
            duration_ms,
        });
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.core.record(Event::UpdateButtonLabel { node: *node, label: label.to_string() });
    }

    fn create_text_input(
        &mut self,
        _initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateTextInput {
            placeholder: placeholder.map(|s| s.to_string()),
            has_key_handler: on_key_down.is_some(),
        });
        if let Some(h) = on_key_down {
            self.core.key_handlers.borrow_mut().insert(id, h);
        }
        id
    }

    fn create_text_area(
        &mut self,
        _initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateTextArea {
            placeholder: placeholder.map(|s| s.to_string()),
            has_key_handler: on_key_down.is_some(),
        });
        if let Some(h) = on_key_down {
            self.core.key_handlers.borrow_mut().insert(id, h);
        }
        id
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        self.core.record(Event::UpdateTextInputValue { node: *node, value: value.to_string() });
    }

    fn create_toggle(
        &mut self,
        value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateToggle { value });
        id
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        self.core.record(Event::UpdateToggleValue { node: *node, value });
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateScrollView { horizontal });
        id
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateSlider {
            value: initial_value,
            min,
            max,
            step,
        });
        id
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        self.core.record(Event::UpdateSliderValue { node: *node, value });
    }

    fn create_activity_indicator(
        &mut self,
        _size: primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateActivityIndicator);
        id
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateVirtualizer { overscan, horizontal });
        id
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        self.core.record(Event::VirtualizerDataChanged { node: *node });
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        self.core.record(Event::ReleaseVirtualizer { node: *node });
    }

    fn create_graphics(
        &mut self,
        _on_ready: primitives::graphics::OnReady,
        _on_resize: primitives::graphics::OnResize,
        _on_lost: primitives::graphics::OnLost,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateGraphics);
        id
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        self.core.record(Event::ReleaseGraphics { node: *node });
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        _base: &Rc<StyleRules>,
        overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
    ) {
        self.core.record(Event::ApplyStyledStates { node: *node, overlays: overlays.len() });
    }

    fn handles_states_natively(&self) -> bool {
        // Default false; backends override to true. We claim false so
        // tests that toggle states see per-state apply_style calls
        // (more useful for assertions than the consolidated
        // apply_styled_states path).
        false
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.core.record(Event::RegisterStylesheet { rules: rules.len() });
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.core.record(Event::UnregisterStylesheet { rules: rules.len() });
    }

    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        let (source_bytes_len, source_extension) = match source {
            AssetSource::Embedded { bytes, extension } => {
                (Some(bytes.len()), Some((*extension).to_string()))
            }
            AssetSource::Bundled { .. } | AssetSource::Remote { .. } => (None, None),
        };
        self.core.record(Event::RegisterAsset {
            id,
            kind,
            source_bytes_len,
            source_extension,
        });
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        self.core.record(Event::UnregisterAsset { id, kind });
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        _family_name: &str,
        faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        self.core.record(Event::RegisterTypeface { id, face_count: faces.len() });
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        self.core.record(Event::UnregisterTypeface { id });
    }

    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        self.core.record(Event::InstallThemeVariables { token_count: tokens.len() });
    }

    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        self.core.record(Event::UpdateTokens { token_count: tokens.len() });
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        self.core.record(Event::OnNodeUnstyled { node: *node });
    }

    fn frame(&self, _node: &Self::Node) -> Option<ViewportRect> {
        Some(ViewportRect::default())
    }

    fn absolute_frame(&self, _node: &Self::Node) -> Option<ViewportRect> {
        Some(ViewportRect::default())
    }

    fn finish(&mut self, root: Self::Node) {
        self.core.record(Event::Finish { root });
    }

    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateLink {
            url: config.url.to_string(),
            route: config.route.to_string(),
        });
        id
    }

    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreatePortal);
        id
    }

    fn release_portal(&mut self, node: &Self::Node) {
        self.core.record(Event::ReleasePortal { node: *node });
    }

    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn Any>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.core.mint();
        self.core.record(Event::CreateExternal { type_name });
        id
    }

    fn release_external(&mut self, node: &Self::Node) {
        self.core.record(Event::ReleaseExternal { node: *node });
    }

    // --- Handle constructors — back the typed handle with the node id ---

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        ButtonHandle::new(Rc::new(*node), &MockButtonOps)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> PressableHandle {
        PressableHandle::new(Rc::new(*node), &MockPressableOps)
    }

    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle::new(Rc::new(*node), &MockViewOps)
    }

    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle::new(Rc::new(*node), &MockTextOps)
    }
}
