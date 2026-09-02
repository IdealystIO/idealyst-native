//! The permanent runtime test substrate: a first-class scene
//! [`Host`] + **all 30** `runtime_vocabulary::caps::*Ops` traits,
//! implemented DIRECTLY over a recording op log.
//!
//! ## Why this crate exists
//!
//! `HostMock` is the canonical way to unit-test vocabulary handlers, SDK
//! components, and anything else that mounts through
//! [`runtime_scene::realize`]: one recorder implements the entire
//! capability surface natively, so a suite asserts against a byte-stable
//! op log instead of hand-rolling a per-suite mock.
//!
//! ## Op-log format (canonical)
//!
//! One string per backend call, `n{id}`-addressed, matching the
//! dominant historical Mini formats so the suites' byte-stable
//! expectations carried over unchanged:
//!
//! - `create n0 view` / `create n1 text "hi"` / `create n2 button "Go"`
//!   / `create n3 anchor` / `create n4 element "pre"` / `create n5
//!   portal` / `create n6 virtualizer overscan=1 layout=…` / …
//! - `insert n0 <- n1`, `insert_at n0 <- n1 @ 0`,
//!   `insert_many n0 <- [n1, n3]`, `remove_child n0 -x n1`,
//!   `clear_children n0`
//! - `apply_style n0 width=Literal(Px(120.0))` (digest configurable per
//!   harness via [`Harness::set_style_line`]), `on_node_unstyled n0`,
//!   `attach_states n0`, `set_disabled n0 false`, `mark_container n0`
//! - `update_text n0 "x"`, `update_button_label n0 "x"`,
//!   `update_toggle_value n0 true`, …
//! - `apply_presence n1 rest 200ms` / `apply_presence n1 op=Some(0.0)
//!   ty=None snap`
//! - `execute_batch nodes=6 ops=[cv#0 ct#1"row 0" ins#0<-#1 style#0=…]`
//!
//! Two recording tiers keep exact-equality assertions stable:
//!
//! - the **core** tier (on by default) records exactly the op families
//!   the historical Minis recorded;
//! - the **verbose** tier ([`Harness::record_all`]) additionally records
//!   every other capability call (input installs, safe area, a11y,
//!   assets, wire notes, lifecycle, …) — the per-caps conformance
//!   battery and new behavioral tests use it.
//!
//! Individual op families can be muted per harness
//! ([`Harness::mute`]) when a suite's exact-log expectations predate an
//! op the mock now records.
//!
//! ## Harness idioms
//!
//! ```ignore
//! let h = host_mock::Harness::new();
//! let realized = h.mount(view().child(text().content("hi")).build());
//! assert_eq!(h.take_log(), vec!["create n0 view", …]);
//! h.flush();                 // world.flush()
//! drop(realized);            // teardown probes fire
//! ```
//!
//! Capability flags are live `Cell`s on [`Shared`] (`h.shared`):
//! `batched_repeat`, `splice`, `renders_lazy_chunks`, `preminted`,
//! `handles_states_natively`, `hydrating`, `screenshot`. Captured
//! platform callbacks (press handlers, slider/toggle/text-input
//! `on_change`, link activations, `attach_states` setters, virtualizer
//! callback bundles, graphics lifecycle closures, portal dismissals,
//! scroll handlers) accumulate in `h.shared.*` vectors so tests fire
//! them like a real platform would.
//!
//! Deterministic async: [`pump`] holds the manually-pumped
//! `AsyncExecutor` (chunk loads) and `Scheduler` (animation frames +
//! timers) that the lazy and presence suites need — thread-local
//! queues behind the process-global installs, so parallel test threads
//! can't cross-fire.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority, Role};
use runtime_shared::animation::AnimProp;
use runtime_shared::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_shared::primitives;
use runtime_shared::primitives::graphics::{OnLost, OnReady, OnResize};
use runtime_shared::primitives::icon::IconData;
use runtime_shared::primitives::link::LinkConfig;
use runtime_shared::primitives::portal::{PortalTarget, ViewportRect};
use runtime_shared::primitives::presence::PresenceState;
use runtime_shared::styled_text::TextRun;
use runtime_shared::{
    Action, BackendBatch, BatchOp, Color, Easing, FileDropHandler, HoverHandler,
    ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides, StateBits,
    StyleRules, TokenEntry, Tokenized, TouchHandler, TouchId, VirtualizerCallbacks, WheelHandler,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::{caps, register_builtins};
use runtime_world::World;

pub mod pump;

/// The mock's node handle: sequentially minted ids, `n0, n1, …`.
pub type Node = u32;

/// Captured graphics lifecycle closures (boxed `FnMut`s — borrow the
/// vec mutably to fire them, mirroring the platform).
pub struct GraphicsCapture {
    pub on_ready: OnReady,
    pub on_resize: OnResize,
    pub on_lost: OnLost,
}

/// Shared recorder state: the op log, the node tree, captured platform
/// callbacks, and the live capability flags. One `Rc<Shared>` is held by
/// the [`HostMock`] and one by the [`Harness`], so tests observe and
/// configure while the mock is mounted behind `Rc<RefCell<…>>`.
pub struct Shared {
    /// The op log (one canonical string per recorded call).
    pub log: RefCell<Vec<String>>,
    /// Muted op-family names (never recorded).
    pub muted: RefCell<HashSet<&'static str>>,
    /// Verbose tier on/off (see crate docs).
    pub verbose: Cell<bool>,

    // --- node tree bookkeeping (for snapshots/assertions) ---
    /// Minted kind/description per node (`"view"`, `"text \"hi\""`, …).
    pub kinds: RefCell<BTreeMap<Node, String>>,
    /// Children per parent, in insertion order.
    pub children: RefCell<BTreeMap<Node, Vec<Node>>>,
    /// Parent per child.
    pub parent: RefCell<BTreeMap<Node, Node>>,

    // --- captured platform callbacks, in creation order ---
    /// `attach_states` setters (flip interaction states like a native
    /// event source).
    pub state_setters: RefCell<Vec<Rc<dyn Fn(StateBits, bool)>>>,
    /// Pressable `on_click`s.
    pub press_handlers: RefCell<Vec<Rc<dyn Fn()>>>,
    /// Button actions (`Action::fire`).
    pub button_presses: RefCell<Vec<Rc<dyn Fn()>>>,
    /// Slider `on_change`s (raw, pre-snap).
    pub slider_changes: RefCell<Vec<Rc<dyn Fn(f32)>>>,
    /// Toggle `on_change`s.
    pub toggle_changes: RefCell<Vec<Rc<dyn Fn(bool)>>>,
    /// Text input AND text area `on_change`s (creation order).
    pub text_input_changes: RefCell<Vec<Rc<dyn Fn(String)>>>,
    /// Text input / text area `on_blur` handlers, `None` when the author
    /// declared none (creation order). Captured so the cancelable
    /// `BlurOutcome` contract — a handler CAN veto the blur — is
    /// assertable from a host test; the platform veto mechanisms
    /// (`textFieldShouldEndEditing:`, AppKit outside-click, web refocus)
    /// all consult exactly this handler.
    pub blur_handlers: RefCell<Vec<Option<primitives::text_input::BlurHandler>>>,
    /// Text input / text area `on_key_down` handlers, `None` when absent
    /// (creation order) — the `KeyOutcome::PreventDefault` propagation
    /// contract.
    pub key_down_handlers: RefCell<Vec<Option<primitives::key::KeyDownHandler>>>,
    /// Link activations (`LinkConfig::on_activate` — fire like clicks).
    pub link_activations: RefCell<Vec<Rc<dyn Fn()>>>,
    /// Scroll-view `on_scroll` handlers (None when absent).
    pub scroll_handlers: RefCell<Vec<Option<Rc<dyn Fn(f32, f32)>>>>,
    /// Virtualizer callback bundles.
    pub virtualizers: RefCell<Vec<VirtualizerCallbacks<Node>>>,
    /// `virtual_grid` callback bundles (two-axis).
    pub virtual_grids: RefCell<Vec<primitives::virtual_grid::GridCallbacks<Node>>>,
    /// Graphics lifecycle closures.
    pub graphics: RefCell<Vec<GraphicsCapture>>,
    /// Portal `on_dismiss` (None when absent).
    pub portal_dismissals: RefCell<Vec<Option<Rc<dyn Fn()>>>>,
    /// File-drop handlers installed via
    /// `InputOps::install_file_drop_handler`, with their target node —
    /// captured (not just logged) so a test can FIRE one and prove the
    /// author callback is reached, which is the whole contract on
    /// backends whose real drop source is unreachable from a host test.
    pub file_drop_handlers: RefCell<Vec<(Node, FileDropHandler)>>,
    /// Touch handlers installed via `InputOps::install_touch_handler`,
    /// with their target node.
    pub touch_handlers: RefCell<Vec<(Node, TouchHandler)>>,
    /// Hover handlers installed via `InputOps::install_hover_handler`.
    pub hover_handlers: RefCell<Vec<(Node, HoverHandler)>>,

    // --- capability flags (live; flip mid-test) ---
    /// `Host::supports_splice` (anchorless reactive regions).
    pub splice: Cell<bool>,
    /// `BatchOps::supports_batched_repeat`.
    pub batched_repeat: Cell<bool>,
    /// `LifecycleOps::renders_lazy_chunks` (false = SSR posture).
    pub renders_lazy_chunks: Cell<bool>,
    /// `LifecycleOps::is_hydrating`.
    pub hydrating: Cell<bool>,
    /// `StyleOps::supports_preminted_styles`.
    pub preminted: Cell<bool>,
    /// `StyleOps::handles_states_natively`.
    pub handles_states_natively: Cell<bool>,
    /// `IntrospectionOps::supports_screenshot`.
    pub screenshot: Cell<bool>,
    /// `AppEnvOps::platform`.
    pub platform: RefCell<Platform>,

    // --- configurable behaviors ---
    /// Full-line formatter for `apply_style` (see
    /// [`Harness::set_style_line`]). Default digests `width` only.
    pub style_line: RefCell<Option<Rc<dyn Fn(Node, &StyleRules) -> String>>>,
    /// `StyleOps::mint_style_class` hook (default: `None` → no class
    /// model).
    pub mint_class: RefCell<Option<Rc<dyn Fn(&StyleRules) -> Option<String>>>>,
    /// Per-node scroll offsets served by `ScrollOps::node_scroll`.
    pub scroll_offsets: RefCell<BTreeMap<Node, (f32, f32)>>,
    /// Frames served by `IntrospectionOps::{frame,absolute_frame,device_frame}`.
    pub frames: RefCell<BTreeMap<Node, ViewportRect>>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            log: RefCell::new(Vec::new()),
            muted: RefCell::new(HashSet::new()),
            verbose: Cell::new(false),
            kinds: RefCell::new(BTreeMap::new()),
            children: RefCell::new(BTreeMap::new()),
            parent: RefCell::new(BTreeMap::new()),
            state_setters: RefCell::new(Vec::new()),
            press_handlers: RefCell::new(Vec::new()),
            button_presses: RefCell::new(Vec::new()),
            slider_changes: RefCell::new(Vec::new()),
            toggle_changes: RefCell::new(Vec::new()),
            text_input_changes: RefCell::new(Vec::new()),
            blur_handlers: RefCell::new(Vec::new()),
            key_down_handlers: RefCell::new(Vec::new()),
            link_activations: RefCell::new(Vec::new()),
            scroll_handlers: RefCell::new(Vec::new()),
            virtualizers: RefCell::new(Vec::new()),
            virtual_grids: RefCell::new(Vec::new()),
            graphics: RefCell::new(Vec::new()),
            portal_dismissals: RefCell::new(Vec::new()),
            file_drop_handlers: RefCell::new(Vec::new()),
            touch_handlers: RefCell::new(Vec::new()),
            hover_handlers: RefCell::new(Vec::new()),
            splice: Cell::new(false),
            batched_repeat: Cell::new(false),
            renders_lazy_chunks: Cell::new(true),
            hydrating: Cell::new(false),
            preminted: Cell::new(false),
            handles_states_natively: Cell::new(false),
            screenshot: Cell::new(false),
            platform: RefCell::new(Platform::Custom("host-mock")),
            style_line: RefCell::new(None),
            mint_class: RefCell::new(None),
            scroll_offsets: RefCell::new(BTreeMap::new()),
            frames: RefCell::new(BTreeMap::new()),
        }
    }
}

impl Shared {
    /// Record a core-tier op (unless muted).
    fn rec(&self, name: &'static str, line: String) {
        if self.muted.borrow().contains(name) {
            return;
        }
        self.log.borrow_mut().push(line);
    }

    /// Record a verbose-tier op (only when `verbose` is on; still
    /// mutable).
    fn rec_v(&self, name: &'static str, line: String) {
        if self.verbose.get() {
            self.rec(name, line);
        }
    }
}

fn width_of(rules: &StyleRules) -> String {
    rules
        .width
        .as_ref()
        .map(|w| format!("{w:?}"))
        .unwrap_or_else(|| "none".into())
}

fn presence_digest(state: &PresenceState) -> String {
    if *state == PresenceState::rest() {
        "rest".to_string()
    } else {
        format!("op={:?} ty={:?}", state.opacity, state.translate_y)
    }
}

// ===========================================================================
// Recording view-handle ops (animated-prop writes)
// ===========================================================================

/// `runtime_shared::ViewOps` (the handle-ops trait) whose animated-prop
/// writers record into a thread-local log — `ViewHandle` ops must be
/// `&'static`, so the log can't live in the harness. Drain with
/// [`take_animated_log`].
struct RecordingViewOps;

thread_local! {
    static ANIMATED_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Drain the thread-local animated-prop write log
/// (`"set_animated_f32 n0 TranslateX 16"`, …).
pub fn take_animated_log() -> Vec<String> {
    ANIMATED_LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

impl runtime_shared::ViewOps for RecordingViewOps {
    fn set_animated_f32(&self, node: &dyn Any, prop: AnimProp, value: f32) {
        let n = node.downcast_ref::<Node>().copied().unwrap_or(Node::MAX);
        ANIMATED_LOG.with(|l| {
            l.borrow_mut()
                .push(format!("set_animated_f32 n{n} {prop:?} {value}"))
        });
    }
}

static RECORDING_VIEW_OPS: RecordingViewOps = RecordingViewOps;

// ===========================================================================
// HostMock
// ===========================================================================

/// The recording scene host. Construct through [`Harness`] (or
/// [`HostMock::new`] for bespoke setups).
pub struct HostMock {
    next: Node,
    /// Shared recorder handle (same `Rc` the harness holds).
    pub s: Rc<Shared>,
}

impl HostMock {
    pub fn new(shared: Rc<Shared>) -> Self {
        HostMock { next: 0, s: shared }
    }

    /// Mint the next node id, recording `create n{id} {desc}` and the
    /// kind entry.
    fn mint(&mut self, desc: String) -> Node {
        let n = self.next;
        self.next += 1;
        self.s.kinds.borrow_mut().insert(n, desc.clone());
        self.s.rec("create", format!("create n{n} {desc}"));
        n
    }

    /// Mint WITHOUT logging (batch materialization mints through the
    /// batch digest instead).
    fn mint_silent(&mut self, desc: &str) -> Node {
        let n = self.next;
        self.next += 1;
        self.s.kinds.borrow_mut().insert(n, desc.to_string());
        n
    }

    fn link(&self, parent: Node, child: Node, index: Option<usize>) {
        let mut children = self.s.children.borrow_mut();
        let list = children.entry(parent).or_default();
        match index {
            Some(i) if i <= list.len() => list.insert(i, child),
            _ => list.push(child),
        }
        self.s.parent.borrow_mut().insert(child, parent);
    }
}

// ---------------------------------------------------------------------------
// Host — the structural seam
// ---------------------------------------------------------------------------

impl Host for HostMock {
    type Node = Node;

    fn insert(&mut self, parent: &mut Node, child: Node) {
        self.s.rec("insert", format!("insert n{parent} <- n{child}"));
        self.link(*parent, child, None);
    }

    fn insert_many(&mut self, parent: &mut Node, children: Vec<Node>) {
        let kids: Vec<String> = children.iter().map(|c| format!("n{c}")).collect();
        self.s.rec(
            "insert_many",
            format!("insert_many n{parent} <- [{}]", kids.join(", ")),
        );
        for child in children {
            self.link(*parent, child, None);
        }
    }

    fn insert_at(&mut self, parent: &mut Node, child: Node, index: usize) {
        self.s.rec(
            "insert_at",
            format!("insert_at n{parent} <- n{child} @ {index}"),
        );
        // A move of an already-mounted child detaches first (DOM
        // `insertBefore` semantics).
        if let Some(old_parent) = self.s.parent.borrow().get(&child).copied() {
            if let Some(list) = self.s.children.borrow_mut().get_mut(&old_parent) {
                list.retain(|c| c != &child);
            }
        }
        self.link(*parent, child, Some(index));
    }

    fn remove_child(&mut self, parent: &Node, child: &Node) {
        self.s.rec(
            "remove_child",
            format!("remove_child n{parent} -x n{child}"),
        );
        if let Some(list) = self.s.children.borrow_mut().get_mut(parent) {
            list.retain(|c| c != child);
        }
        self.s.parent.borrow_mut().remove(child);
    }

    fn clear_children(&mut self, node: &Node) {
        self.s.rec("clear_children", format!("clear_children n{node}"));
        if let Some(list) = self.s.children.borrow_mut().get_mut(node) {
            let mut parent = self.s.parent.borrow_mut();
            for c in list.drain(..) {
                parent.remove(&c);
            }
        }
    }

    /// Recorded so tests can tell a real DISPOSAL from the
    /// `clear_children` that merely detaches a retained screen — the
    /// distinction `Host::release_subtree` exists to carry.
    fn release_subtree(&mut self, node: &Node) {
        self.s.rec("release_subtree", format!("release_subtree n{node}"));
    }

    fn create_anchor(&mut self) -> Node {
        self.mint("anchor".into())
    }

    fn supports_splice(&self) -> bool {
        self.s.splice.get()
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for HostMock {
    fn platform(&self) -> Platform {
        *self.s.platform.borrow()
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        self.s
            .rec_v("set_page_metadata", format!("set_page_metadata {:?}", meta.title));
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        self.s
            .rec_v("set_app_background", format!("set_app_background {color:?}"));
    }

    fn set_scrollbar_theme(&mut self, _thumb: &Tokenized<Color>, _track: &Tokenized<Color>) {
        self.s
            .rec_v("set_scrollbar_theme", "set_scrollbar_theme".to_string());
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        self.s.rec_v(
            "set_app_key_handler",
            format!("set_app_key_handler {}", if handler.is_some() { "some" } else { "none" }),
        );
    }
}

impl caps::LifecycleOps for HostMock {
    fn finish(&mut self, root: Node) {
        self.s.rec_v("finish", format!("finish n{root}"));
    }

    fn run_layout(&mut self) {
        self.s.rec_v("run_layout", "run_layout".to_string());
    }

    fn is_hydrating(&self) -> bool {
        self.s.hydrating.get()
    }

    fn renders_lazy_chunks(&self) -> bool {
        self.s.renders_lazy_chunks.get()
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for HostMock {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Node {
        self.mint("view".into())
    }

    fn make_view_handle(&self, node: &Node) -> runtime_shared::ViewHandle {
        // Real (recording) animated-prop ops, not the trait's no-op
        // default — animated-bind regression tests assert writes reach
        // `ViewOps::set_animated_f32` (drain via `take_animated_log`).
        runtime_shared::ViewHandle::new(Rc::new(*node), &RECORDING_VIEW_OPS)
    }
}

impl caps::InputOps for HostMock {
    fn install_touch_handler(&mut self, node: &Node, handler: TouchHandler) {
        self.s
            .rec_v("install_touch_handler", format!("install_touch_handler n{node}"));
        self.s.touch_handlers.borrow_mut().push((*node, handler));
    }

    fn claim_touch(&mut self, node: &Node, _touch_id: TouchId) {
        self.s.rec_v("claim_touch", format!("claim_touch n{node}"));
    }

    fn install_wheel_handler(&mut self, node: &Node, _handler: WheelHandler) {
        self.s
            .rec_v("install_wheel_handler", format!("install_wheel_handler n{node}"));
    }

    fn install_hover_handler(&mut self, node: &Node, handler: HoverHandler) {
        self.s
            .rec_v("install_hover_handler", format!("install_hover_handler n{node}"));
        self.s.hover_handlers.borrow_mut().push((*node, handler));
    }

    fn mark_preserves_focus(&mut self, node: &Node) {
        self.s
            .rec_v("mark_preserves_focus", format!("mark_preserves_focus n{node}"));
    }

    fn install_file_drop_handler(&mut self, node: &Node, handler: FileDropHandler) {
        self.s.rec_v(
            "install_file_drop_handler",
            format!("install_file_drop_handler n{node}"),
        );
        self.s.file_drop_handlers.borrow_mut().push((*node, handler));
    }
}

impl caps::PressableOps for HostMock {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Node {
        self.s.press_handlers.borrow_mut().push(on_click);
        self.mint("pressable".into())
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for HostMock {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Node {
        self.mint(format!("text {content:?}"))
    }

    fn create_styled_text(&mut self, runs: &[TextRun], _a11y: &AccessibilityProps) -> Node {
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        self.mint(format!("styled_text {texts:?}"))
    }

    fn update_styled_text(&mut self, node: &Node, runs: &[TextRun]) {
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        self.s
            .rec("update_styled_text", format!("update_styled_text n{node} {texts:?}"));
    }

    fn update_text(&mut self, node: &Node, content: &str) {
        self.s
            .rec("update_text", format!("update_text n{node} {content:?}"));
    }

    // create_text_with_id stays the trait default (`None`): dyn text
    // exercises the legacy create+update fallback, like the old Minis.
}

impl caps::ButtonOps for HostMock {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&IconData>,
        _trailing_icon: Option<&IconData>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.button_presses.borrow_mut().push(on_click.fire.clone());
        self.mint(format!("button {label:?}"))
    }

    fn update_button_label(&mut self, node: &Node, label: &str) {
        self.s.rec(
            "update_button_label",
            format!("update_button_label n{node} {label:?}"),
        );
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for HostMock {
    fn create_image(&mut self, src: &str, _alt: Option<&str>, _a11y: &AccessibilityProps) -> Node {
        self.mint(format!("image {src:?}"))
    }

    fn update_image_src(&mut self, node: &Node, src: &str) {
        self.s
            .rec("update_image_src", format!("update_image_src n{node} {src:?}"));
    }

    fn update_image_alt(&mut self, node: &Node, alt: Option<&str>) {
        self.s
            .rec_v("update_image_alt", format!("update_image_alt n{node} {alt:?}"));
    }

    fn install_image_load_handler(&mut self, node: &Node, _handler: ImageLoadHandler) {
        self.s.rec_v(
            "install_image_load_handler",
            format!("install_image_load_handler n{node}"),
        );
    }

    fn install_image_error_handler(&mut self, node: &Node, _handler: ImageErrorHandler) {
        self.s.rec_v(
            "install_image_error_handler",
            format!("install_image_error_handler n{node}"),
        );
    }
}

impl caps::IconOps for HostMock {
    fn create_icon(
        &mut self,
        _data: &IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.mint("icon".into())
    }

    fn update_icon_color(&mut self, node: &Node, color: &Color) {
        self.s
            .rec_v("update_icon_color", format!("update_icon_color n{node} {}", color.0));
    }

    fn update_icon_data(&mut self, node: &Node, data: &IconData) {
        // The glyph geometry is the observable — a live `data` source has
        // to reach the backend with the NEW paths, not just fire.
        self.s.rec_v(
            "update_icon_data",
            format!("update_icon_data n{node} {:?}", data.paths),
        );
    }

    fn update_icon_stroke(&mut self, node: &Node, progress: f32) {
        self.s
            .rec_v("update_icon_stroke", format!("update_icon_stroke n{node} {progress}"));
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        _easing: Easing,
        _infinite: bool,
        _autoreverses: bool,
    ) {
        self.s.rec_v(
            "animate_icon_stroke",
            format!("animate_icon_stroke n{node} {from}->{to} {duration_ms}ms"),
        );
    }
}

impl caps::LinkOps for HostMock {
    fn create_link(&mut self, config: LinkConfig, _a11y: &AccessibilityProps) -> Node {
        self.s
            .link_activations
            .borrow_mut()
            .push(config.on_activate.clone());
        self.mint(format!("link route={:?} url={:?}", config.route, config.url))
    }

    fn update_link_url(&mut self, node: &Node, url: &str) {
        self.s
            .rec_v("update_link_url", format!("update_link_url n{node} {url:?}"));
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for HostMock {
    fn create_text_input(
        &mut self,
        _initial_value: &str,
        _placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        _secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.text_input_changes.borrow_mut().push(on_change);
        self.s.key_down_handlers.borrow_mut().push(on_key_down);
        self.s.blur_handlers.borrow_mut().push(on_blur);
        self.mint("text_input".into())
    }

    fn update_text_input_value(&mut self, node: &Node, value: &str) {
        self.s.rec(
            "update_text_input_value",
            format!("update_text_input_value n{node} {value:?}"),
        );
    }

    fn update_text_input_secure(&mut self, node: &Node, secure: bool) {
        self.s.rec_v(
            "update_text_input_secure",
            format!("update_text_input_secure n{node} {secure}"),
        );
    }

    fn set_text_input_focus_handler(&mut self, node: &Node, _handler: Rc<dyn Fn(bool)>) {
        self.s.rec_v(
            "set_text_input_focus_handler",
            format!("set_text_input_focus_handler n{node}"),
        );
    }

    fn update_text_input_placeholder(&mut self, node: &Node, placeholder: Option<&str>) {
        self.s.rec_v(
            "update_text_input_placeholder",
            format!("update_text_input_placeholder n{node} {placeholder:?}"),
        );
    }

    fn create_text_area(
        &mut self,
        _initial_value: &str,
        _placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.text_input_changes.borrow_mut().push(on_change);
        self.s.key_down_handlers.borrow_mut().push(on_key_down);
        // A text area has no `on_blur` slot; keep the two capture lists
        // index-aligned with `text_input_changes` so `blur_handler(i)`
        // means the same creation as `text_input_change(i)`.
        self.s.blur_handlers.borrow_mut().push(None);
        // Wrap / row bounds are create-time config with no update op, so
        // the create line is the only place they are observable.
        self.mint(format!("text_area wrap={wrap} min_rows={min_rows:?} max_rows={max_rows:?}"))
    }

    fn update_text_area_value(&mut self, node: &Node, value: &str) {
        self.s.rec_v(
            "update_text_area_value",
            format!("update_text_area_value n{node} {value:?}"),
        );
    }
}

impl caps::ToggleOps for HostMock {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.toggle_changes.borrow_mut().push(on_change);
        self.mint(format!("toggle {initial_value}"))
    }

    fn update_toggle_value(&mut self, node: &Node, value: bool) {
        self.s.rec(
            "update_toggle_value",
            format!("update_toggle_value n{node} {value}"),
        );
    }
}

impl caps::SliderOps for HostMock {
    fn create_slider(
        &mut self,
        _initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.slider_changes.borrow_mut().push(on_change);
        self.mint("slider".into())
    }

    fn update_slider_value(&mut self, node: &Node, value: f32) {
        self.s.rec(
            "update_slider_value",
            format!("update_slider_value n{node} {value}"),
        );
    }
}

impl caps::ActivityIndicatorOps for HostMock {
    fn create_activity_indicator(
        &mut self,
        _size: primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.mint("activity_indicator".into())
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        self.s.rec(
            "update_activity_indicator_size",
            format!("update_activity_indicator_size n{node} {size:?}"),
        );
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for HostMock {
    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.scroll_handlers.borrow_mut().push(on_scroll);
        self.mint("scroll_view".into())
    }

    fn node_scroll(&self, node: &Node) -> (f32, f32) {
        self.s
            .scroll_offsets
            .borrow()
            .get(node)
            .copied()
            .unwrap_or((0.0, 0.0))
    }

    fn set_node_scroll(&mut self, node: &Node, x: f32, y: f32) {
        self.s
            .rec("set_node_scroll", format!("set_node_scroll n{node} {x} {y}"));
        self.s.scroll_offsets.borrow_mut().insert(*node, (x, y));
    }
}

impl caps::SafeAreaOps for HostMock {
    fn apply_safe_area_padding(&mut self, node: &Node, _sides: SafeAreaSides) {
        self.s.rec_v(
            "apply_safe_area_padding",
            format!("apply_safe_area_padding n{node}"),
        );
    }

    // apply_scroll_view_safe_area_inset keeps the trait default (falls
    // back to apply_safe_area_padding) — the frozen fallback chain the
    // conformance battery pins.
}

impl caps::GridOps for HostMock {
    fn create_virtual_grid(
        &mut self,
        callbacks: primitives::virtual_grid::GridCallbacks<Node>,
        overscan: f32,
        _a11y: &AccessibilityProps,
    ) -> Node {
        // Do NOT call callbacks here: the mock is mutably borrowed
        // during create — same constraint as every real backend.
        let cols = (callbacks.col_count)();
        let rows = (callbacks.row_count)();
        self.s.virtual_grids.borrow_mut().push(callbacks);
        self.mint(format!(
            "virtual_grid cols={cols} rows={rows} overscan={overscan}"
        ))
    }

    fn virtual_grid_data_changed(&mut self, node: &Node) {
        self.s.rec(
            "virtual_grid_data_changed",
            format!("virtual_grid_data_changed n{node}"),
        );
    }

    fn release_virtual_grid(&mut self, node: &Node) {
        self.s.rec(
            "release_virtual_grid",
            format!("release_virtual_grid n{node}"),
        );
    }
}

impl caps::VirtualizerOps for HostMock {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Node {
        // Do NOT call callbacks here: the mock is mutably borrowed
        // during create — same constraint as every real backend.
        self.s.virtualizers.borrow_mut().push(callbacks);
        self.mint(format!("virtualizer overscan={overscan} layout={layout:?}"))
    }

    fn virtualizer_data_changed(&mut self, node: &Node) {
        self.s.rec(
            "virtualizer_data_changed",
            format!("virtualizer_data_changed n{node}"),
        );
    }

    fn release_virtualizer(&mut self, node: &Node) {
        self.s
            .rec("release_virtualizer", format!("release_virtualizer n{node}"));
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for HostMock {
    fn create_graphics(
        &mut self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.graphics.borrow_mut().push(GraphicsCapture {
            on_ready,
            on_resize,
            on_lost,
        });
        self.mint("graphics".into())
    }

    fn release_graphics(&mut self, node: &Node) {
        self.s
            .rec("release_graphics", format!("release_graphics n{node}"));
    }
}

impl caps::PortalOps for HostMock {
    fn create_portal(
        &mut self,
        _target: PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.s.portal_dismissals.borrow_mut().push(on_dismiss);
        self.mint("portal".into())
    }

    fn release_portal(&mut self, node: &Node) {
        self.s.rec("release_portal", format!("release_portal n{node}"));
    }

    fn set_portal_hidden(&mut self, node: &Node, hidden: bool) {
        self.s.rec(
            "set_portal_hidden",
            format!("set_portal_hidden n{node} {hidden}"),
        );
    }
}

impl caps::PresenceOps for HostMock {
    fn create_presence_placeholder(&mut self, _a11y: &AccessibilityProps) -> Node {
        self.mint("presence_placeholder".into())
    }

    fn apply_presence(
        &mut self,
        node: &Node,
        state: PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        let t = match transition {
            Some((ms, _)) => format!("{ms}ms"),
            None => "snap".to_string(),
        };
        self.s.rec(
            "apply_presence",
            format!("apply_presence n{node} {} {t}", presence_digest(&state)),
        );
    }
}

impl caps::NavigatorOps for HostMock {
    fn release_navigator(&mut self, node: &Node) {
        self.s
            .rec("release_navigator", format!("release_navigator n{node}"));
    }

    fn apply_navigator_slot_style(&mut self, node: &Node, slot: &'static str, _style: &Rc<StyleRules>) {
        self.s.rec_v(
            "apply_navigator_slot_style",
            format!("apply_navigator_slot_style n{node} {slot}"),
        );
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Node,
        screen: Node,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        self.s.rec_v(
            "navigator_attach_initial",
            format!("navigator_attach_initial n{navigator} <- n{screen}"),
        );
        self.link(*navigator, screen, None);
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for HostMock {
    fn create_external(
        &mut self,
        _type_id: TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn Any>,
        _a11y: &AccessibilityProps,
    ) -> Node {
        self.mint(format!("external {type_name}"))
    }

    fn release_external(&mut self, node: &Node) {
        self.s
            .rec("release_external", format!("release_external n{node}"));
    }
}

impl caps::DocumentOps for HostMock {
    fn create_element(&mut self, tag: &str) -> Node {
        self.mint(format!("element {tag:?}"))
    }

    fn attach_html_id(&self, node: &Node, id: &str) {
        self.s
            .rec_v("attach_html_id", format!("attach_html_id n{node} {id}"));
    }

    fn attach_html_class(&self, node: &Node, class: &str) {
        self.s
            .rec("attach_html_class", format!("attach_html_class n{node} {class}"));
    }

    fn attach_html_style(&self, node: &Node, prop: &str, value: &str) {
        self.s.rec_v(
            "attach_html_style",
            format!("attach_html_style n{node} {prop}={value}"),
        );
    }

    fn register_raw_css(&mut self, css: &str) {
        self.s
            .rec_v("register_raw_css", format!("register_raw_css {} bytes", css.len()));
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for HostMock {
    fn apply_style(&mut self, node: &Node, style: &Rc<StyleRules>) {
        let line = match &*self.s.style_line.borrow() {
            Some(f) => f(*node, style),
            None => format!("apply_style n{node} width={}", width_of(style)),
        };
        self.s.rec("apply_style", line);
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        let hook = self.s.mint_class.borrow().clone();
        hook.and_then(|f| f(style))
    }

    fn mark_container(&mut self, node: &Node) {
        self.s.rec("mark_container", format!("mark_container n{node}"));
    }

    fn handles_states_natively(&self) -> bool {
        self.s.handles_states_natively.get()
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.s.rec(
            "register_stylesheet",
            format!("register_stylesheet rules={}", rules.len()),
        );
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.s.rec(
            "unregister_stylesheet",
            format!("unregister_stylesheet rules={}", rules.len()),
        );
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.s.rec("install_tokens", format!("install_tokens {names:?}"));
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.s.rec("update_tokens", format!("update_tokens {names:?}"));
    }

    fn on_node_unstyled(&mut self, node: &Node) {
        self.s
            .rec("on_node_unstyled", format!("on_node_unstyled n{node}"));
    }

    fn attach_states(&mut self, node: &Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        self.s.state_setters.borrow_mut().push(setter);
        self.s.rec("attach_states", format!("attach_states n{node}"));
    }

    fn set_disabled(&mut self, node: &Node, disabled: bool) {
        self.s
            .rec("set_disabled", format!("set_disabled n{node} {disabled}"));
    }

    fn supports_preminted_styles(&self) -> bool {
        self.s.preminted.get()
    }

    fn apply_default_text_font(&mut self, font: Option<&runtime_shared::FontFamily>) {
        self.s.rec_v(
            "apply_default_text_font",
            format!("apply_default_text_font {}", if font.is_some() { "some" } else { "none" }),
        );
    }
}

impl caps::AssetOps for HostMock {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, _source: &AssetSource) {
        self.s
            .rec_v("register_asset", format!("register_asset {id:?} {kind:?}"));
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        self.s
            .rec_v("unregister_asset", format!("unregister_asset {id:?} {kind:?}"));
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        _faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        self.s.rec_v(
            "register_typeface",
            format!("register_typeface {id:?} {family_name}"),
        );
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        self.s
            .rec_v("unregister_typeface", format!("unregister_typeface {id:?}"));
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for HostMock {
    fn update_accessibility(
        &mut self,
        node: &Node,
        _a11y: &AccessibilityProps,
        _inferred_role: Option<Role>,
    ) {
        self.s
            .rec_v("update_accessibility", format!("update_accessibility n{node}"));
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        self.s.rec_v(
            "announce_for_accessibility",
            format!("announce_for_accessibility {msg:?} {priority:?}"),
        );
    }
}

impl caps::AnimationOps for HostMock {
    fn set_animated_f32(&mut self, node: &Node, prop: AnimProp, value: f32) {
        self.s.rec_v(
            "set_animated_f32",
            format!("set_animated_f32 n{node} {prop:?} {value}"),
        );
    }

    fn set_animated_color(&mut self, node: &Node, prop: AnimProp, value: [f32; 4]) {
        self.s.rec_v(
            "set_animated_color",
            format!("set_animated_color n{node} {prop:?} {value:?}"),
        );
    }
}

impl caps::IntrospectionOps for HostMock {
    fn frame(&self, node: &Node) -> Option<ViewportRect> {
        self.s.frames.borrow().get(node).copied()
    }

    fn absolute_frame(&self, node: &Node) -> Option<ViewportRect> {
        self.s.frames.borrow().get(node).copied()
    }

    fn device_frame(&self, node: &Node) -> Option<ViewportRect> {
        self.s.frames.borrow().get(node).copied()
    }

    fn supports_screenshot(&self) -> bool {
        self.s.screenshot.get()
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for HostMock {
    fn supports_batched_repeat(&self) -> bool {
        self.s.batched_repeat.get()
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Node> {
        let mut digest: Vec<String> = Vec::new();
        for op in &batch.ops {
            match op {
                BatchOp::CreateView { local_id } => digest.push(format!("cv#{local_id}")),
                BatchOp::CreateText { local_id, content } => {
                    digest.push(format!("ct#{local_id}{content:?}"))
                }
                BatchOp::ApplyStyleStatic { node, class_name, .. } => {
                    digest.push(format!("style#{node}={class_name}"))
                }
                BatchOp::Insert { parent, child } => digest.push(format!("ins#{parent}<-#{child}")),
            }
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(batch.node_count as usize);
        for _ in 0..batch.node_count {
            nodes.push(self.mint_silent("batched"));
        }
        self.s.rec(
            "execute_batch",
            format!(
                "execute_batch nodes={} ops=[{}]",
                batch.node_count,
                digest.join(" ")
            ),
        );
        nodes
    }

    // execute_batch_with_attach keeps the trait default (execute_batch
    // then Host::insert_many) — the frozen attach sequence.
}

impl caps::WireBindingOps for HostMock {
    fn note_text_binding(&mut self, node: &Node, _signal_ids: &[u64], method: &'static str) {
        self.s
            .rec_v("note_text_binding", format!("note_text_binding n{node} {method}"));
    }

    fn note_signal_initial(&mut self, signal_id: u64, _value: &runtime_shared::__serde_json::Value) {
        self.s
            .rec_v("note_signal_initial", format!("note_signal_initial {signal_id}"));
    }

    fn begin_slot_capture(&mut self) {
        self.s
            .rec_v("begin_slot_capture", "begin_slot_capture".to_string());
    }

    fn end_slot_capture(&mut self, slot_root: &Node) {
        self.s
            .rec_v("end_slot_capture", format!("end_slot_capture n{slot_root}"));
    }
}

// ===========================================================================
// Harness
// ===========================================================================

/// The standard test harness: a fresh [`World`], the recording
/// [`HostMock`] behind `Rc<RefCell<…>>`, and a [`Registry`] with the
/// vocabulary built-ins registered. Mirrors the shape every historical
/// Mini harness had, so suites keep their idioms
/// (`h.world.enter(|| realize(&h.backend, &h.registry, el))`,
/// `h.take_log()`, `h.world.flush()`).
pub struct Harness {
    pub world: World,
    pub backend: Rc<RefCell<HostMock>>,
    pub registry: Rc<Registry<HostMock>>,
    /// Shared recorder: op log, captures, flags (see [`Shared`]).
    pub shared: Rc<Shared>,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// Harness with the vocabulary built-ins only.
    pub fn new() -> Harness {
        Self::with_registry(|_| {})
    }

    /// Harness with the built-ins PLUS extra handler registrations (the
    /// SDK boot-registration seam — pass e.g. `codeblock::register`).
    pub fn with_registry(extra: impl FnOnce(&mut Registry<HostMock>)) -> Harness {
        let shared: Rc<Shared> = Rc::new(Shared::default());
        let backend = Rc::new(RefCell::new(HostMock::new(shared.clone())));
        let mut registry = Registry::new();
        register_builtins(&mut registry);
        extra(&mut registry);
        Harness {
            world: World::new(),
            backend,
            registry: Rc::new(registry),
            shared,
        }
    }

    /// Realize `element` inside the harness world.
    pub fn mount(&self, element: Element) -> Realized<Node> {
        self.world
            .enter(|| realize(&self.backend, &self.registry, element))
    }

    /// Commit staged writes (world flush).
    pub fn flush(&self) {
        self.world.flush();
    }

    /// Drain the op log.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.log.borrow_mut())
    }

    /// Copy the op log without draining.
    pub fn ops(&self) -> Vec<String> {
        self.shared.log.borrow().clone()
    }

    /// Clear the op log.
    pub fn clear_ops(&self) {
        self.shared.log.borrow_mut().clear();
    }

    /// Record EVERY capability call, not just the core tier.
    pub fn record_all(&self) {
        self.shared.verbose.set(true);
    }

    /// Mute op families by name (never recorded), e.g.
    /// `h.mute(&["mark_container"])`.
    pub fn mute(&self, names: &[&'static str]) {
        self.shared.muted.borrow_mut().extend(names.iter().copied());
    }

    /// Install a full-line `apply_style` formatter (`|node, rules| …`).
    pub fn set_style_line(&self, f: impl Fn(Node, &StyleRules) -> String + 'static) {
        *self.shared.style_line.borrow_mut() = Some(Rc::new(f));
    }

    /// Install a `mint_style_class` hook (batched-Repeat class model).
    pub fn set_mint_class(&self, f: impl Fn(&StyleRules) -> Option<String> + 'static) {
        *self.shared.mint_class.borrow_mut() = Some(Rc::new(f));
    }

    // --- capture accessors (creation order) ---

    pub fn state_setter(&self, i: usize) -> Rc<dyn Fn(StateBits, bool)> {
        self.shared.state_setters.borrow()[i].clone()
    }

    pub fn press_handler(&self, i: usize) -> Rc<dyn Fn()> {
        self.shared.press_handlers.borrow()[i].clone()
    }

    pub fn link_activation(&self, i: usize) -> Rc<dyn Fn()> {
        self.shared.link_activations.borrow()[i].clone()
    }

    pub fn slider_change(&self, i: usize) -> Rc<dyn Fn(f32)> {
        self.shared.slider_changes.borrow()[i].clone()
    }

    pub fn toggle_change(&self, i: usize) -> Rc<dyn Fn(bool)> {
        self.shared.toggle_changes.borrow()[i].clone()
    }

    pub fn text_input_change(&self, i: usize) -> Rc<dyn Fn(String)> {
        self.shared.text_input_changes.borrow()[i].clone()
    }

    /// The `on_blur` handler the i-th `create_text_input` received, or
    /// `None` when the author declared none (index-aligned with
    /// [`Harness::text_input_change`]; `create_text_area` pushes `None`).
    /// Calling it returns the author's
    /// [`BlurOutcome`](runtime_shared::primitives::text_input::BlurOutcome)
    /// — the cancelable-blur contract every platform veto mechanism
    /// consults.
    pub fn blur_handler(&self, i: usize) -> Option<primitives::text_input::BlurHandler> {
        self.shared.blur_handlers.borrow().get(i).cloned().flatten()
    }

    /// The `on_scroll` handler the i-th `create_scroll_view` received
    /// (`None` when the author declared none) — firing it is how a test
    /// reproduces a platform scroll report.
    pub fn scroll_handler(&self, i: usize) -> Option<Rc<dyn Fn(f32, f32)>> {
        self.shared.scroll_handlers.borrow().get(i).cloned().flatten()
    }

    /// How many scroll views were created (so "absence" is assertable
    /// separately from "index out of range").
    pub fn scroll_view_count(&self) -> usize {
        self.shared.scroll_handlers.borrow().len()
    }

    /// The file-drop handler installed on the i-th
    /// `install_file_drop_handler` call, with its target node.
    pub fn file_drop_handler(&self, i: usize) -> Option<(Node, FileDropHandler)> {
        self.shared
            .file_drop_handlers
            .borrow()
            .get(i)
            .map(|(n, h)| (*n, h.clone()))
    }

    /// The `on_key_down` handler the i-th text input / text area received
    /// (`None` when absent) — the `KeyOutcome::PreventDefault`
    /// propagation contract.
    pub fn key_down_handler(&self, i: usize) -> Option<primitives::key::KeyDownHandler> {
        self.shared
            .key_down_handlers
            .borrow()
            .get(i)
            .cloned()
            .flatten()
    }

    /// Shallow clone (all fields `Rc` + one bool) of a captured
    /// `virtual_grid` callback bundle — clone-by-field, same reason
    /// as [`Harness::virtualizer`] (the bundle isn't `Clone` because
    /// its `N` needn't be).
    pub fn virtual_grid(&self, i: usize) -> primitives::virtual_grid::GridCallbacks<Node> {
        let cbs = self.shared.virtual_grids.borrow();
        let v = &cbs[i];
        primitives::virtual_grid::GridCallbacks {
            col_count: v.col_count.clone(),
            row_count: v.row_count.clone(),
            col_width: v.col_width.clone(),
            row_height: v.row_height.clone(),
            cell_key: v.cell_key.clone(),
            mount_cell: v.mount_cell.clone(),
            release_cell: v.release_cell.clone(),
            on_scroll: v.on_scroll.clone(),
        }
    }

    /// virtualizer callback bundle.
    pub fn virtualizer(&self, i: usize) -> VirtualizerCallbacks<Node> {
        let cbs = self.shared.virtualizers.borrow();
        let v = &cbs[i];
        VirtualizerCallbacks {
            item_count: v.item_count.clone(),
            item_key: v.item_key.clone(),
            item_size: v.item_size.clone(),
            measure_sizes: v.measure_sizes,
            mount_item: v.mount_item.clone(),
            release_item: v.release_item.clone(),
            set_measured_size: v.set_measured_size.clone(),
            on_scroll: v.on_scroll.clone(),
        }
    }

    // --- node-tree snapshot helpers ---

    /// The minted kind/description of `node` (`"view"`, `"text \"hi\""`).
    pub fn kind_of(&self, node: Node) -> Option<String> {
        self.shared.kinds.borrow().get(&node).cloned()
    }

    /// Current children of `node`, in order.
    pub fn children_of(&self, node: Node) -> Vec<Node> {
        self.shared
            .children
            .borrow()
            .get(&node)
            .cloned()
            .unwrap_or_default()
    }

    /// Indented tree snapshot rooted at `node` (`"n0 view\n  n1 text …"`).
    pub fn tree(&self, node: Node) -> String {
        fn walk(h: &Harness, node: Node, depth: usize, out: &mut String) {
            let kind = h.kind_of(node).unwrap_or_else(|| "?".into());
            out.push_str(&format!("{}n{node} {kind}\n", "  ".repeat(depth)));
            for child in h.children_of(node) {
                walk(h, child, depth + 1, out);
            }
        }
        let mut out = String::new();
        walk(self, node, 0, &mut out);
        out.trim_end().to_string()
    }
}

/// Shorthand for [`Harness::new`], mirroring the historical per-suite
/// `harness()` free function.
pub fn harness() -> Harness {
    Harness::new()
}

// Compile-time proof: HostMock satisfies the whole capability surface
// natively (the claim the per-caps conformance battery run-proves).
const _: () = {
    const fn assert_all_caps<T: runtime_vocabulary::AllCaps>() {}
    assert_all_caps::<HostMock>()
};
