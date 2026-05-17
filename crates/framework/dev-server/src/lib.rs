//! Dev-side runtime for the hot-reload wire protocol.
//!
//! Provides a [`WireRecordingBackend`] that implements
//! [`framework_core::Backend`] with `Node = NodeId`. Each method
//! emits one [`Command`] (or a small cluster) into the recorder's
//! outbound queue, plus registers any closures the walker hands it
//! into a [`HandlerTable`].
//!
//! When the app fires an event, the dev side looks up the
//! `HandlerId` in the handler table and runs the captured closure.
//! That closure mutates signals; signal-driven effects re-fire
//! through the walker; the walker calls more `Backend` methods on
//! this same `WireRecordingBackend`, producing more `Command`s;
//! those flush back to the app.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use framework_core::primitives;
use framework_core::{Backend, Color, ColorScheme, StateBits, StyleRules};
use wire::{
    Command, EventArgs, HandlerId, NodeId, StyleId, WireColor, WireStateBit,
};

mod convert_out;
mod scene_model;
pub mod transport;
pub mod watch;

use scene_model::SceneModel;

pub use transport::serve;
pub use watch::{spawn_rebuild_loop, RebuildCommand, RebuildConfig};

/// The AAS (Application-as-a-Server) **server-side backend** —
/// implements `framework_core::Backend` with `Node = NodeId`. Plug
/// this into `framework_core::render(...)` exactly like you'd plug
/// in `WebBackend` / `IosBackend` / `AndroidBackend`. Instead of
/// driving native widgets it records every walker call as a wire
/// [`wire::Command`] for transport to one or more
/// [`AasClient`](dev_client::AasClient)s.
///
/// `AasBackend` is the heart of the AAS architecture:
///
/// ```text
/// UI tree → AasBackend → Wire (Commands) → AasClient → Platform Backend → Native
/// ```
///
/// The same `Primitive` tree your iOS/web app would render natively
/// is what the server runs against this backend. The wire output is
/// platform-agnostic; an `AasClient` wrapping any platform backend
/// can replay it.
pub use crate::WireRecordingBackend as AasBackend;

/// Stores the live dev-side closures the walker has handed us. Each
/// gets a `HandlerId` minted by the recorder; events arriving back
/// from the app look up the entry and invoke the captured closure.
#[derive(Default)]
pub struct HandlerTable {
    next: u64,
    closures: HashMap<HandlerId, Handler>,
}

enum Handler {
    Unit(Rc<dyn Fn()>),
    Bool(Rc<dyn Fn(bool)>),
    Float(Rc<dyn Fn(f32)>),
    StringFn(Rc<dyn Fn(String)>),
    States(Rc<dyn Fn(StateBits, bool)>),
}

impl HandlerTable {
    fn mint(&mut self) -> HandlerId {
        self.next += 1;
        HandlerId(self.next)
    }

    pub fn register_unit(&mut self, f: Rc<dyn Fn()>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Unit(f));
        id
    }

    pub fn register_bool(&mut self, f: Rc<dyn Fn(bool)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Bool(f));
        id
    }

    pub fn register_float(&mut self, f: Rc<dyn Fn(f32)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::Float(f));
        id
    }

    pub fn register_string(&mut self, f: Rc<dyn Fn(String)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::StringFn(f));
        id
    }

    pub fn register_states(&mut self, f: Rc<dyn Fn(StateBits, bool)>) -> HandlerId {
        let id = self.mint();
        self.closures.insert(id, Handler::States(f));
        id
    }

    pub fn release(&mut self, id: HandlerId) {
        self.closures.remove(&id);
    }
}

/// The dev-side backend. Implements `Backend` with `Node = NodeId`;
/// each method emits one or more `Command`s into the recorder.
///
/// Cloning shares the underlying state (Rc<RefCell<…>>) so callers
/// can hand out clones of the recorder to multiple owners — the
/// usual pattern when the walker holds one ref and the dispatch
/// loop holds another.
pub struct WireRecordingBackend {
    inner: Rc<RefCell<RecorderState>>,
    /// Send+Sync mirror of per-navigator URL stacks. Updated
    /// synchronously by the recorder's dispatcher (main thread)
    /// every time a navigator's stack changes. Read by the file
    /// watch + rebuild thread just before `exec`, to serialize and
    /// pass forward as an env var. Survives the process image
    /// swap, letting the freshly-started server restore the
    /// navigator hierarchy.
    nav_state_mirror: Arc<Mutex<NavStateSnapshot>>,
}

/// Map: navigator `NodeId.0` → stack of route URLs (initial route
/// first, top-of-stack last). Snapshotted across `exec` to recover
/// the navigation hierarchy after a hot-reload restart.
pub type NavStateSnapshot = HashMap<u64, Vec<String>>;

struct RecorderState {
    next_node: u64,
    next_style: u64,
    handlers: HandlerTable,
    /// Pre-registered styles. Each `Rc<StyleRules>` pointer identity
    /// gets mapped to a `StyleId` on first encounter so the wire
    /// never re-serializes the same rules.
    styles_by_ptr: HashMap<usize, StyleId>,
    out: Vec<Command>,
    color_scheme: ColorScheme,
    /// Maps from a `NodeId` to the most recent state-attach handler.
    /// Lets `dispatch_state` look up the closure to invoke when the
    /// app reports a state-bit transition.
    state_handlers: HashMap<NodeId, HandlerId>,
    /// Per-navigator dev-side state. Used by the dispatcher to call
    /// the framework's screen-mount callbacks and to track the local
    /// scope stack for app-initiated release events.
    navigators: HashMap<NodeId, NavigatorRecState>,
    /// Reverse lookup: scope_id → navigator that owns it. Lets the
    /// recorder route an `AppToDev::ScreenReleased { scope }` to the
    /// right navigator's `release_screen` callback.
    scope_to_navigator: HashMap<u64, NodeId>,
    /// Shared handle to the Send+Sync nav-state mirror, so the
    /// dispatcher can push updates from inside `borrow_mut` without
    /// going through the outer `WireRecordingBackend`.
    nav_state_mirror: Arc<Mutex<NavStateSnapshot>>,
    /// Mirror of the live scene — the source of truth for catch-up
    /// replay. Each recorder method that emits a `Command` also
    /// updates this model so a freshly connecting client can be
    /// brought up to the current state without replaying historical
    /// transients (pushed-then-popped screens, typed-then-deleted
    /// text, scroll positions that moved many times, etc.).
    /// `out` is still used for incremental broadcast to clients
    /// already past the snapshot point.
    scene: SceneModel,
}

/// Per-navigator dev-side state used by the recording backend's
/// dispatcher. The callbacks come from the framework when
/// `create_navigator` is called; the stack is the recorder's own
/// model of the navigator's screen stack (so it can call
/// `release_screen` for the right scope when handling Pop / swipe-back).
pub struct NavigatorRecState {
    /// Framework-supplied callbacks. `Rc`'d because individual fields
    /// are already `Rc<dyn Fn(...)>`; the wrapping `Rc` makes whole-
    /// struct clones cheap from inside the dispatcher closure.
    pub callbacks: Rc<framework_core::primitives::navigator::NavigatorCallbacks<NodeId>>,
    /// Scope ids of the screens currently on the navigator's stack,
    /// top of stack = end of vec. Updated by the dispatcher on push
    /// and by app-event handlers on swipe-back.
    pub stack: Vec<u64>,
    /// URL paths of the screens on the navigator's stack, in lock
    /// step with `stack`. Persisted across `exec` so the navigation
    /// hierarchy can be restored on the freshly-started server.
    pub stack_urls: Vec<String>,
}

impl WireRecordingBackend {
    pub fn new() -> Self {
        let nav_state_mirror = Arc::new(Mutex::new(NavStateSnapshot::new()));
        Self {
            inner: Rc::new(RefCell::new(RecorderState {
                next_node: 0,
                next_style: 0,
                handlers: HandlerTable::default(),
                styles_by_ptr: HashMap::new(),
                out: Vec::new(),
                color_scheme: ColorScheme::Auto,
                state_handlers: HashMap::new(),
                navigators: HashMap::new(),
                scope_to_navigator: HashMap::new(),
                nav_state_mirror: nav_state_mirror.clone(),
                scene: SceneModel::new(),
            })),
            nav_state_mirror,
        }
    }

    /// Public handle to the per-navigator URL stack mirror. Send + Sync
    /// — safe to share with the file-watch / rebuild thread so it can
    /// serialize the current navigation hierarchy before `exec`.
    pub fn nav_state_mirror(&self) -> Arc<Mutex<NavStateSnapshot>> {
        self.nav_state_mirror.clone()
    }

    /// Restore a previously-snapshotted navigator stack. Called by
    /// the dev-server's main on startup, after the framework's
    /// initial render has produced fresh navigators at the same
    /// `NodeId`s. For each saved URL beyond the initial route, we
    /// look up the matching `(name, params)` via `match_path` and
    /// dispatch a `Push` — which goes through the regular dispatcher
    /// and emits real `NavigatorPush` wire commands, so the same
    /// screens come back without any client-side cooperation.
    pub fn restore_nav_state(&self, saved: &NavStateSnapshot) {
        use framework_core::primitives::navigator::NavCommand;
        for (nav_id_raw, urls) in saved {
            let nav_id = NodeId(*nav_id_raw);
            // Snapshot the callbacks under a short borrow.
            let callbacks = {
                let state = self.inner.borrow();
                let Some(nav) = state.navigators.get(&nav_id) else {
                    eprintln!(
                        "[dev-server] restore: no navigator at id {}; the route table may have changed",
                        nav_id_raw
                    );
                    continue;
                };
                nav.callbacks.clone()
            };
            // Skip the initial route — `navigator_attach_initial`
            // already put it in place.
            for url in urls.iter().skip(1) {
                let Some((name, params)) = (callbacks.match_path)(url) else {
                    eprintln!(
                        "[dev-server] restore: no route matches {:?} — stopping replay for navigator {}",
                        url, nav_id_raw
                    );
                    break;
                };
                navigator_dispatcher_handle(
                    &self.inner,
                    nav_id,
                    callbacks.clone(),
                    NavCommand::Push {
                        name,
                        url: url.clone(),
                        params,
                    },
                );
            }
        }
    }

    pub fn set_color_scheme(&self, scheme: ColorScheme) {
        self.inner.borrow_mut().color_scheme = scheme;
    }

    /// Drain any pending commands, removing them from the recorder's
    /// log. Used by the legacy single-snapshot path; new code should
    /// prefer the append-only [`Self::commands_since`] /
    /// [`Self::command_count`] API which lets multiple clients each
    /// hold their own cursor into the same shared log.
    pub fn drain_commands(&self) -> Vec<Command> {
        std::mem::take(&mut self.inner.borrow_mut().out)
    }

    /// Total number of commands ever emitted by the walker into this
    /// recorder's log. Pair with [`Self::commands_since`] to ferry
    /// "everything past my cursor" to a connected client.
    pub fn command_count(&self) -> usize {
        self.inner.borrow().out.len()
    }

    /// Return a clone of every command emitted at or after `cursor`.
    /// Used for incremental broadcast to clients already past the
    /// initial snapshot. The recorder's log is append-only; cursors
    /// only ever move forward.
    ///
    /// Note: this does NOT filter out commands superseded by later
    /// state (a Push followed by a Pop both stay in the log). Fresh
    /// clients should instead start from [`Self::snapshot`], which
    /// serializes the *current* scene state. Live clients are
    /// presumed to have already received the historical commands,
    /// so incremental replay from their cursor is correct.
    pub fn commands_since(&self, cursor: usize) -> Vec<Command> {
        let state = self.inner.borrow();
        if cursor >= state.out.len() {
            Vec::new()
        } else {
            state.out[cursor..].to_vec()
        }
    }

    /// Build a fresh wire-command stream that, applied to an empty
    /// client, reproduces the *current* scene. Catchup uses this
    /// instead of `commands_since(0)` so transient history (mounts
    /// later popped, signal mutations later overwritten) doesn't
    /// re-play on every reconnect.
    ///
    /// The client should set its replay cursor to whatever
    /// [`Self::command_count`] returned at the moment this snapshot
    /// was taken — subsequent `commands_since(cursor)` calls then
    /// pick up live mutations that happened after the snapshot.
    pub fn snapshot(&self) -> Vec<Command> {
        self.inner.borrow().scene.snapshot_commands()
    }

    /// Dispatch an event received from the app. Returns true if the
    /// handler was found and invoked. The closure may mutate signals
    /// (triggering further walker work + commands); callers should
    /// `drain_commands()` after this returns to flush.
    pub fn dispatch_event(&self, id: HandlerId, args: EventArgs) -> bool {
        // Snapshot the closure ref out of the borrow first — the
        // closure body may re-enter this backend through the walker.
        let handler_ref = {
            let state = self.inner.borrow();
            state.handlers.closures.get(&id).map(|h| match h {
                Handler::Unit(f) => HandlerSnapshot::Unit(f.clone()),
                Handler::Bool(f) => HandlerSnapshot::Bool(f.clone()),
                Handler::Float(f) => HandlerSnapshot::Float(f.clone()),
                Handler::StringFn(f) => HandlerSnapshot::StringFn(f.clone()),
                Handler::States(f) => HandlerSnapshot::States(f.clone()),
            })
        };
        let Some(handler) = handler_ref else {
            return false;
        };
        match (handler, args) {
            (HandlerSnapshot::Unit(f), EventArgs::Unit) => {
                f();
                true
            }
            (HandlerSnapshot::Bool(f), EventArgs::Bool(v)) => {
                f(v);
                true
            }
            (HandlerSnapshot::Float(f), EventArgs::Float(v)) => {
                f(v);
                true
            }
            (HandlerSnapshot::StringFn(f), EventArgs::String(v)) => {
                f(v);
                true
            }
            _ => false,
        }
    }

    /// Dispatch a state-bit transition reported from the app.
    pub fn dispatch_state(&self, node: NodeId, bit: WireStateBit, on: bool) -> bool {
        let cb = {
            let state = self.inner.borrow();
            let Some(&handler_id) = state.state_handlers.get(&node) else {
                return false;
            };
            state.handlers.closures.get(&handler_id).and_then(|h| {
                if let Handler::States(f) = h {
                    Some(f.clone())
                } else {
                    None
                }
            })
        };
        if let Some(cb) = cb {
            cb(convert_out::wire_state_bit_to_bits(bit), on);
            true
        } else {
            false
        }
    }

    fn mint_node(state: &mut RecorderState) -> NodeId {
        state.next_node += 1;
        NodeId(state.next_node)
    }

    fn intern_style(state: &mut RecorderState, rules: &Rc<StyleRules>) -> StyleId {
        let ptr = Rc::as_ptr(rules) as usize;
        if let Some(&id) = state.styles_by_ptr.get(&ptr) {
            return id;
        }
        state.next_style += 1;
        let id = StyleId(state.next_style);
        state.styles_by_ptr.insert(ptr, id);
        let wire = convert_out::style_rules_to_wire(rules);
        state.out.push(Command::RegisterStyle { id, rules: wire });
        id
    }
}

impl Default for WireRecordingBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WireRecordingBackend {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            nav_state_mirror: self.nav_state_mirror.clone(),
        }
    }
}

enum HandlerSnapshot {
    Unit(Rc<dyn Fn()>),
    Bool(Rc<dyn Fn(bool)>),
    Float(Rc<dyn Fn(f32)>),
    StringFn(Rc<dyn Fn(String)>),
    #[allow(dead_code)]
    States(Rc<dyn Fn(StateBits, bool)>),
}

// ---------------------------------------------------------------------------
// Backend impl
// ---------------------------------------------------------------------------

impl Backend for WireRecordingBackend {
    type Node = NodeId;

    fn color_scheme(&self) -> ColorScheme {
        self.inner.borrow().color_scheme
    }

    fn create_view(&mut self) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateView { id });
        id
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateText {
            id,
            content: content.to_string(),
        });
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: Rc<dyn Fn()>,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_unit(on_click);
        let leading = leading_icon.map(convert_out::icon_data_to_wire);
        let trailing = trailing_icon.map(convert_out::icon_data_to_wire);
        state.out.push(Command::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
        });
        id
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_unit(on_click);
        state.out.push(Command::CreatePressable {
            id,
            on_click: handler,
        });
        id
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateReactiveAnchor { id });
        id
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::Insert {
            parent: *parent,
            child,
        });
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::InsertMany {
            parent: *parent,
            children,
        });
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::ClearChildren { node: *node });
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateText {
            node: *node,
            content: content.to_string(),
        });
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateImage {
            id,
            src: src.to_string(),
            alt: alt.map(str::to_string),
        });
        id
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateImageSrc {
            node: *node,
            src: src.to_string(),
        });
    }

    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_data = convert_out::icon_data_to_wire(data);
        state.out.push(Command::CreateIcon {
            id,
            data: wire_data,
            color: color.map(|c| WireColor(c.0.clone())),
        });
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateIconColor {
            node: *node,
            color: WireColor(color.0.clone()),
        });
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateIconStroke {
            node: *node,
            progress,
        });
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: framework_core::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::AnimateIconStroke {
            node: *node,
            from,
            to,
            duration_ms,
            easing: convert_out::easing_to_wire(easing),
            infinite,
            autoreverses,
        });
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateButtonLabel {
            node: *node,
            label: label.to_string(),
        });
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_string(on_change);
        state.out.push(Command::CreateTextInput {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(str::to_string),
            on_change: handler,
        });
        id
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateTextInputValue {
            node: *node,
            value: value.to_string(),
        });
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_bool(on_change);
        state.out.push(Command::CreateToggle {
            id,
            initial_value,
            on_change: handler,
        });
        id
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateToggleValue {
            node: *node,
            value,
        });
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateScrollView { id, horizontal });
        id
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_float(on_change);
        state.out.push(Command::CreateSlider {
            id,
            initial_value,
            min,
            max,
            step,
            on_change: handler,
        });
        id
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateSliderValue {
            node: *node,
            value,
        });
    }

    fn create_web_view(&mut self, url: &str) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateWebView {
            id,
            url: url.to_string(),
        });
        id
    }

    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateWebViewUrl {
            node: *node,
            url: url.to_string(),
        });
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateVideo {
            id,
            src: src.to_string(),
            autoplay,
            controls,
            loop_playback,
        });
        id
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::UpdateVideoSrc {
            node: *node,
            src: src.to_string(),
        });
    }

    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let wire_size = match size {
            primitives::activity_indicator::ActivityIndicatorSize::Small => {
                wire::WireActivityIndicatorSize::Small
            }
            primitives::activity_indicator::ActivityIndicatorSize::Large => {
                wire::WireActivityIndicatorSize::Large
            }
        };
        state.out.push(Command::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor(c.0.clone())),
        });
        id
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyStyle {
            node: *node,
            style: sid,
        });
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        let mut state = self.inner.borrow_mut();
        let base_id = Self::intern_style(&mut state, base);
        let mut wire_overlays = Vec::with_capacity(overlays.len());
        for (bits, rules) in overlays {
            let sid = Self::intern_style(&mut state, rules);
            for bit in convert_out::expand_state_bits(*bits) {
                wire_overlays.push((bit, sid));
            }
        }
        state.out.push(Command::ApplyStyledStates {
            node: *node,
            base: base_id,
            overlays: wire_overlays,
        });
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        let mut state = self.inner.borrow_mut();
        for r in rules {
            Self::intern_style(&mut state, r);
        }
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        let mut state = self.inner.borrow_mut();
        for r in rules {
            let ptr = Rc::as_ptr(r) as usize;
            if let Some(sid) = state.styles_by_ptr.remove(&ptr) {
                state.out.push(Command::UnregisterStyle { id: sid });
            }
        }
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        let mut state = self.inner.borrow_mut();
        let handler_id = state.handlers.register_states(setter);
        state.state_handlers.insert(*node, handler_id);
        state.out.push(Command::AttachStates { node: *node });
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::SetDisabled {
            node: *node,
            disabled,
        });
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::OnNodeUnstyled { node: *node });
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        s: primitives::presence::PresenceState,
        transition: Option<(u32, framework_core::Easing)>,
    ) {
        let mut state = self.inner.borrow_mut();
        let wire_state = wire::WirePresenceState {
            opacity: s.opacity,
            tx: s.translate_x,
            ty: s.translate_y,
            scale: s.scale,
        };
        let wire_transition = transition.map(|(d, e)| (d, convert_out::easing_to_wire(e)));
        state.out.push(Command::ApplyPresence {
            node: *node,
            state: wire_state,
            transition: wire_transition,
        });
    }

    fn finish(&mut self, root: Self::Node) {
        let mut state = self.inner.borrow_mut();
        state.out.push(Command::Finish { root });
    }

    fn create_overlay(
        &mut self,
        placement: primitives::overlay::ViewportPlacement,
        backdrop: primitives::overlay::BackdropMode,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = on_dismiss.map(|cb| state.handlers.register_unit(cb));
        // The framework recently split overlays into viewport
        // (`Primitive::Overlay`) and element-anchored
        // (`Primitive::AnchoredOverlay`). This Backend method now
        // handles the viewport case only; element-anchored ones go
        // through `create_anchored_overlay` below.
        let wire_anchor = wire::WireOverlayAnchor::Viewport(match placement {
            primitives::overlay::ViewportPlacement::Center => {
                wire::WireViewportPlacement::Center
            }
            primitives::overlay::ViewportPlacement::Top => {
                wire::WireViewportPlacement::Top
            }
            primitives::overlay::ViewportPlacement::Bottom => {
                wire::WireViewportPlacement::Bottom
            }
            primitives::overlay::ViewportPlacement::Left => {
                wire::WireViewportPlacement::Left
            }
            primitives::overlay::ViewportPlacement::Right => {
                wire::WireViewportPlacement::Right
            }
            primitives::overlay::ViewportPlacement::FullScreen => {
                // FullScreen isn't first-class on the wire; closest
                // fit is Center (the style sheet drives the actual
                // size).
                wire::WireViewportPlacement::Center
            }
        });
        let wire_backdrop = match backdrop {
            primitives::overlay::BackdropMode::None => wire::WireBackdropMode::None,
            primitives::overlay::BackdropMode::Dismiss => wire::WireBackdropMode::Dismiss,
            primitives::overlay::BackdropMode::Opaque => wire::WireBackdropMode::Capture,
        };
        state.out.push(Command::CreateOverlay {
            id,
            anchor: wire_anchor,
            backdrop: wire_backdrop,
            on_dismiss: handler,
            trap_focus,
        });
        id
    }

    fn create_graphics(
        &mut self,
        _on_ready: primitives::graphics::OnReady,
        _on_resize: primitives::graphics::OnResize,
        _on_lost: primitives::graphics::OnLost,
    ) -> Self::Node {
        // Graphics emits an "unnamed" renderer reference. Apps that
        // want GPU code to actually run under hot reload need to
        // either (a) extend the Graphics primitive to carry a
        // renderer name in user code, or (b) wire a default renderer
        // registration on the app side. Without one, the surface
        // mounts (so layout is correct) but no GPU code runs.
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateGraphics {
            id,
            renderer: "<unnamed>".to_string(),
        });
        id
    }

    fn create_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.initial_route;
        let initial_path = callbacks.initial_path;
        let callbacks_rc = Rc::new(callbacks);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.out.push(Command::CreateNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        // Install dispatcher. The closure captures a Weak ref to the
        // shared state so it doesn't keep the recorder alive past
        // teardown.
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        let mirror = self.nav_state_mirror.clone();
        let mut urls_snapshot: Option<Vec<String>> = None;
        {
            let mut state = self.inner.borrow_mut();
            if let Some(nav) = state.navigators.get_mut(navigator) {
                nav.stack.push(scope_id);
                // Seed `stack_urls` with the navigator's declared
                // initial path so `restore_nav_state` knows what URL
                // the bottom-of-stack screen corresponds to.
                let initial_path = nav.callbacks.initial_path.to_string();
                nav.stack_urls.push(initial_path);
                urls_snapshot = Some(nav.stack_urls.clone());
            }
            state.scope_to_navigator.insert(scope_id, *navigator);
            let wire_options = screen_options_to_wire(&mut state, &options);
            state.out.push(Command::NavigatorAttachInitial {
                navigator: *navigator,
                screen,
                scope: wire::ScopeId(scope_id),
                options: wire_options,
            });
        }
        if let Some(urls) = urls_snapshot {
            if let Ok(mut m) = mirror.lock() {
                m.insert(navigator.0, urls);
            }
        }
    }

    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.navigator.initial_route;
        let initial_path = callbacks.navigator.initial_path;
        let tabs_wire: Vec<wire::WireTabRegistration> = callbacks
            .tabs
            .iter()
            .map(|t| wire::WireTabRegistration {
                route: t.route.to_string(),
                label: t.label.clone(),
                icon: t.icon.clone(),
            })
            .collect();
        let placement = match callbacks.placement {
            framework_core::primitives::navigator::TabPlacement::Bottom
            | framework_core::primitives::navigator::TabPlacement::Auto => {
                wire::WireTabPlacement::Bottom
            }
            framework_core::primitives::navigator::TabPlacement::Top
            | framework_core::primitives::navigator::TabPlacement::Sidebar => {
                wire::WireTabPlacement::Top
            }
        };
        let mount_policy = match callbacks.mount_policy {
            framework_core::primitives::navigator::MountPolicy::EagerPersistent => {
                wire::WireMountPolicy::EagerPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyPersistent => {
                wire::WireMountPolicy::LazyPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyDisposing => {
                wire::WireMountPolicy::LazyDisposing
            }
        };
        let inner_callbacks_rc = Rc::new(callbacks.navigator);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.out.push(Command::CreateTabNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
                tabs: tabs_wire,
                placement,
                mount_policy,
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: inner_callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = inner_callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // Same wire shape as the stack-navigator initial attach; the
        // app side dispatches based on what kind of navigator the
        // navigator NodeId belongs to.
        self.navigator_attach_initial(navigator, screen, scope_id, options);
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let nav_id;
        let initial_route = callbacks.navigator.initial_route;
        let initial_path = callbacks.navigator.initial_path;
        let items_wire: Vec<wire::WireDrawerItemRegistration> = callbacks
            .items
            .iter()
            .map(|i| wire::WireDrawerItemRegistration {
                route: i.route.to_string(),
                label: i.label.clone(),
                icon: i.icon.clone(),
            })
            .collect();
        let side = match callbacks.side {
            framework_core::primitives::navigator::DrawerSide::Start => {
                wire::WireDrawerSide::Left
            }
            framework_core::primitives::navigator::DrawerSide::End => {
                wire::WireDrawerSide::Right
            }
        };
        let drawer_type = match callbacks.drawer_type {
            framework_core::primitives::navigator::DrawerType::Front => {
                wire::WireDrawerType::Front
            }
            framework_core::primitives::navigator::DrawerType::Slide => {
                wire::WireDrawerType::Slide
            }
        };
        let mount_policy = match callbacks.mount_policy {
            framework_core::primitives::navigator::MountPolicy::EagerPersistent => {
                wire::WireMountPolicy::EagerPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyPersistent => {
                wire::WireMountPolicy::LazyPersistent
            }
            framework_core::primitives::navigator::MountPolicy::LazyDisposing => {
                wire::WireMountPolicy::LazyDisposing
            }
        };
        let drawer_width = callbacks.drawer_width;
        let pinned_above = callbacks.pinned_above;
        let swipe_to_open = callbacks.swipe_to_open;
        let inner_callbacks_rc = Rc::new(callbacks.navigator);
        {
            let mut state = self.inner.borrow_mut();
            nav_id = Self::mint_node(&mut state);
            state.out.push(Command::CreateDrawerNavigator {
                id: nav_id,
                initial_route: initial_route.to_string(),
                initial_path: initial_path.to_string(),
                items: items_wire,
                side,
                drawer_type,
                drawer_width,
                pinned_above,
                swipe_to_open,
                mount_policy,
            });
            state.navigators.insert(
                nav_id,
                NavigatorRecState {
                    callbacks: inner_callbacks_rc.clone(),
                    stack: Vec::new(), stack_urls: Vec::new(),
                },
            );
        }
        let weak_inner = Rc::downgrade(&self.inner);
        let cbs = inner_callbacks_rc.clone();
        control.install(Box::new(move |cmd| {
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            navigator_dispatcher_handle(&inner, nav_id, cbs.clone(), cmd);
        }));
        nav_id
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        self.navigator_attach_initial(navigator, screen, scope_id, options);
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        self.inner.borrow_mut().out.push(Command::DrawerAttachSidebar {
            navigator: *navigator,
            sidebar,
        });
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        // Eagerly snapshot the current data set: count + keys +
        // initial sizes. The wire ships these so the app's
        // virtualizer can stand up its scroll math and visible-window
        // tracking. Items themselves mount lazily via
        // `VirtualizerMountItem` reverse-channel events.
        let count = (callbacks.item_count)();
        let keys: Vec<u64> = (0..count).map(|i| (callbacks.item_key)(i)).collect();
        let sizes: Vec<f32> = (0..count).map(|i| (callbacks.item_size)(i)).collect();
        let measured = callbacks.measure_sizes;

        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        state.out.push(Command::CreateVirtualizer {
            id,
            overscan,
            horizontal,
            initial_size: wire::WireItemSize { measured, sizes },
            initial_keys: keys,
        });
        // Note: callbacks aren't stored on the recorder side yet.
        // Lazy mount-on-demand is the natural next step:
        //   - Stash `callbacks` in a `HashMap<NodeId, _>` similar to
        //     navigators.
        //   - Add `handle_virtualizer_mount_item(node, index)` on the
        //     recorder, mirroring `handle_screen_released`.
        //   - Inside it: `let (n, scope) = (callbacks.mount_item)(idx)`,
        //     emit `VirtualizerAttachItem { node, index, child: n,
        //     scope: ScopeId(scope) }`.
        // Deferred so the comprehensive nav/link/overlay/graphics
        // work ships first.
        let _ = callbacks; // suppress unused
        id
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Re-snapshot count for now — keys/sizes refresh in a follow-up
        // alongside mount-on-demand wiring above.
        self.inner.borrow_mut().out.push(Command::VirtualizerDataChanged {
            node: *node,
            item_count: 0,
        });
    }

    fn apply_navigator_header_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyNavigatorHeaderStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_navigator_title_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyNavigatorTitleStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_navigator_button_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyNavigatorButtonStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_drawer_sidebar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyDrawerSidebarStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_drawer_scrim_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyDrawerScrimStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_bar_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyTabBarStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_icon_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyTabIconStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_tab_label_style(&mut self, navigator: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyTabLabelStyle {
            navigator: *navigator,
            style: sid,
        });
    }

    fn apply_overlay_backdrop_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let mut state = self.inner.borrow_mut();
        let sid = Self::intern_style(&mut state, style);
        state.out.push(Command::ApplyOverlayBackdropStyle {
            node: *node,
            style: sid,
        });
    }

    fn create_link(&mut self, config: primitives::link::LinkConfig) -> Self::Node {
        let mut state = self.inner.borrow_mut();
        let id = Self::mint_node(&mut state);
        let handler = state.handlers.register_unit(config.on_activate);
        // NavKind isn't carried in LinkConfig (the closure already
        // encodes which command to dispatch); the wire stores a
        // placeholder so a future renderer can target the right
        // accessibility role if it cares. Default Push is fine.
        state.out.push(Command::CreateLink {
            id,
            route: config.route.to_string(),
            url: config.url,
            kind: wire::WireNavKind::Push,
            on_activate: handler,
        });
        id
    }

    // Handle constructors fall through to the trait's no-op defaults.
    // Imperative `handle.click()` from dev-side code is a v1 feature
    // (needs a synchronous round trip we don't implement yet).
}

// ---------------------------------------------------------------------------
// Navigator dispatcher — called from the closure installed on the
// NavigatorControl in `create_navigator`. Free function rather than
// method so we don't get tangled up in lifetimes between the closure
// and `self`.
// ---------------------------------------------------------------------------

fn navigator_dispatcher_handle(
    inner: &Rc<RefCell<RecorderState>>,
    nav_id: NodeId,
    cbs: Rc<framework_core::primitives::navigator::NavigatorCallbacks<NodeId>>,
    cmd: framework_core::primitives::navigator::NavCommand,
) {
    use framework_core::primitives::navigator::NavCommand;
    // Helper closure: build the screen subtree by invoking
    // `mount_screen` (which calls back into the recording backend),
    // then translate into wire form. Borrows are released BEFORE
    // mount_screen runs so the walker can re-enter `&mut self`.
    let push_like = |kind: PushLikeKind,
                     name: &'static str,
                     url: String,
                     params: Box<dyn std::any::Any>,
                     restore: bool| {
        let mount = (cbs.mount_screen)(name, params);
        let scope = wire::ScopeId(mount.scope_id);
        let (released_scopes, new_depth, urls_snapshot, mirror) = {
            let mut state = inner.borrow_mut();
            let wire_options = screen_options_to_wire(&mut state, &mount.options);
            let nav_state = state.navigators.get_mut(&nav_id).unwrap();
            let mut released = Vec::new();
            match kind {
                PushLikeKind::Push | PushLikeKind::Select => {
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
                PushLikeKind::Replace => {
                    if let Some(popped) = nav_state.stack.pop() {
                        released.push(popped);
                    }
                    let _ = nav_state.stack_urls.pop();
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
                PushLikeKind::Reset => {
                    released.extend(nav_state.stack.drain(..));
                    nav_state.stack_urls.clear();
                    nav_state.stack.push(mount.scope_id);
                    nav_state.stack_urls.push(url.clone());
                }
            }
            let depth = nav_state.stack.len();
            let urls_snapshot = nav_state.stack_urls.clone();
            state.scope_to_navigator.insert(mount.scope_id, nav_id);
            // Replace/Reset implicitly release the previous top(s);
            // tombstone their push entries so fresh clients don't
            // re-mount them.
            for r in &released {
                state.scope_to_navigator.remove(r);
                if let Some(push_idx) = state.screen_push_log_index.remove(r) {
                    state.tombstones.insert(push_idx);
                }
            }
            let push_log_index = state.out.len();
            state.out.push(match kind {
                PushLikeKind::Push => Command::NavigatorPush {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                PushLikeKind::Replace => Command::NavigatorReplace {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                PushLikeKind::Reset => Command::NavigatorReset {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
                PushLikeKind::Select => Command::NavigatorPush {
                    navigator: nav_id,
                    screen: mount.node,
                    scope,
                    options: wire_options,
                    url: url.clone(),
                    restore,
                },
            });
            // Remember where the push for this scope landed in the
            // log — if the screen is popped later, we tombstone this
            // entry so catch-up replay skips it.
            state
                .screen_push_log_index
                .insert(mount.scope_id, push_log_index);
            let mirror = state.nav_state_mirror.clone();
            (released, depth, urls_snapshot, mirror)
        };
        // Sync the Send+Sync mirror after the borrow is released —
        // the rebuild thread can read this snapshot just before
        // `exec` to persist the navigation hierarchy.
        if let Ok(mut m) = mirror.lock() {
            m.insert(nav_id.0, urls_snapshot);
        }
        for scope_id in released_scopes {
            (cbs.release_screen)(scope_id);
        }
        (cbs.depth_changed)(new_depth);
    };

    match cmd {
        NavCommand::Push { name, url, params } => {
            push_like(PushLikeKind::Push, name, url, params, false)
        }
        NavCommand::Replace { name, url, params } => {
            push_like(PushLikeKind::Replace, name, url, params, false)
        }
        NavCommand::Reset { name, url, params } => {
            push_like(PushLikeKind::Reset, name, url, params, false)
        }
        NavCommand::Select { name, url, params } => {
            push_like(PushLikeKind::Select, name, url, params, false)
        }
        NavCommand::Pop => {
            let (popped_scope, new_depth) = {
                let mut state = inner.borrow_mut();
                let nav_state = state.navigators.get_mut(&nav_id).unwrap();
                let popped = nav_state.stack.pop();
                let _ = nav_state.stack_urls.pop();
                let depth = nav_state.stack.len();
                let urls_snapshot = nav_state.stack_urls.clone();
                let mirror = state.nav_state_mirror.clone();
                // Tombstone the originating push so fresh clients
                // catching up don't replay the mount.
                if let Some(scope_id) = popped {
                    if let Some(push_idx) = state.screen_push_log_index.remove(&scope_id) {
                        state.tombstones.insert(push_idx);
                    }
                }
                state.out.push(Command::NavigatorPop {
                    navigator: nav_id,
                    count: 1,
                });
                // Update mirror outside inner borrow path.
                if let Ok(mut m) = mirror.lock() {
                    m.insert(nav_id.0, urls_snapshot);
                }
                (popped, depth)
            };
            if let Some(scope) = popped_scope {
                inner.borrow_mut().scope_to_navigator.remove(&scope);
                (cbs.release_screen)(scope);
            }
            (cbs.depth_changed)(new_depth);
        }
        NavCommand::OpenDrawer => {
            inner
                .borrow_mut()
                .out
                .push(Command::OpenDrawer { navigator: nav_id });
        }
        NavCommand::CloseDrawer => {
            inner.borrow_mut().out.push(Command::CloseDrawer {
                navigator: nav_id,
            });
        }
        NavCommand::ToggleDrawer => {
            inner.borrow_mut().out.push(Command::ToggleDrawer {
                navigator: nav_id,
            });
        }
    }
}

#[derive(Copy, Clone)]
enum PushLikeKind {
    Push,
    Replace,
    Reset,
    Select,
}

fn screen_options_to_wire(
    state: &mut RecorderState,
    options: &framework_core::primitives::navigator::ScreenOptions,
) -> wire::WireScreenOptions {
    wire::WireScreenOptions {
        title: options.title.clone(),
        header_shown: options.header_shown,
        header_left: options
            .header_left
            .as_ref()
            .map(|btn| header_button_to_wire(state, btn)),
        header_right: options
            .header_right
            .as_ref()
            .map(|btn| header_button_to_wire(state, btn)),
    }
}

fn header_button_to_wire(
    state: &mut RecorderState,
    btn: &framework_core::primitives::navigator::HeaderButton,
) -> wire::WireHeaderButton {
    let handler = state.handlers.register_unit(btn.on_press.clone());
    wire::WireHeaderButton {
        icon: btn.icon.clone(),
        on_press: handler,
        tint: btn.tint.as_ref().map(|c| wire::WireColor(c.0.clone())),
    }
}

// ---------------------------------------------------------------------------
// App→Dev event dispatch helpers exposed on WireRecordingBackend.
// ---------------------------------------------------------------------------

impl WireRecordingBackend {
    /// Handle an `AppToDev::ScreenReleased { scope }` event arriving
    /// from the app side. Looks up which navigator owns the scope,
    /// pops it off the stack model, and invokes the framework's
    /// `release_screen` callback to drop the scope on dev.
    ///
    /// This is the path for *client-initiated* pops — iOS swipe-back
    /// or any platform back gesture that pops a screen without going
    /// through `NavCommand::Pop`. We mirror what the server-side Pop
    /// dispatch does: trim `stack_urls`, sync the snapshot mirror so
    /// the next rebuild-exec snapshot is accurate, and emit a
    /// `Command::NavigatorPop` into the append-only log so other
    /// connected clients (and any fresh client that reconnects)
    /// learn about the pop.
    pub fn handle_screen_released(&self, scope: u64) -> bool {
        eprintln!("[recorder] handle_screen_released(scope={})", scope);
        let (cbs, new_depth, urls_snapshot, nav_id, mirror) = {
            let mut state = self.inner.borrow_mut();
            let Some(&nav_id) = state.scope_to_navigator.get(&scope) else {
                eprintln!("  -> unknown scope, ignored");
                return false;
            };
            let Some(nav) = state.navigators.get_mut(&nav_id) else {
                return false;
            };
            eprintln!("  -> stack_urls before: {:?}", nav.stack_urls);
            nav.stack.retain(|&s| s != scope);
            // Stack navigators only release from the top, so the
            // popped url is the tail of stack_urls. If swap-style
            // releases ever land, this needs to be by-position.
            let _ = nav.stack_urls.pop();
            let depth = nav.stack.len();
            let urls = nav.stack_urls.clone();
            let cbs = nav.callbacks.clone();
            state.scope_to_navigator.remove(&scope);
            // Tombstone the originating push so fresh catch-up
            // replays don't briefly remount this screen.
            if let Some(push_idx) = state.screen_push_log_index.remove(&scope) {
                state.tombstones.insert(push_idx);
            }
            state.out.push(Command::NavigatorPop {
                navigator: nav_id,
                count: 1,
            });
            let mirror = state.nav_state_mirror.clone();
            (cbs, depth, urls, nav_id, mirror)
        };
        if let Ok(mut m) = mirror.lock() {
            m.insert(nav_id.0, urls_snapshot);
        }
        (cbs.release_screen)(scope);
        (cbs.depth_changed)(new_depth);
        true
    }

    /// Handle `AppToDev::DrawerStateChanged` — informational only;
    /// useful for analytics or for fwd to the framework's drawer
    /// open-state signal in a follow-up.
    pub fn handle_drawer_state_changed(&self, _navigator: NodeId, _is_open: bool) {
        // Reserved for drawer signal sync in a follow-up.
    }

    /// Handle `AppToDev::TabSelected` — used when the platform fires
    /// a tab activation gesture. The framework's tab navigator
    /// dispatcher needs to be told to switch.
    pub fn handle_tab_selected(&self, _navigator: NodeId, _index: u32) {
        // Reserved for tab signal sync in a follow-up.
    }
}
