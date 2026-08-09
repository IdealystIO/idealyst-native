//! App-side replay engine.
//!
//! Wraps a real platform [`Backend`] and applies an incoming stream
//! of [`Command`]s against it. The wire's `NodeId` namespace is held
//! in a `HashMap<NodeId, B::Node>`; styles are pre-registered into a
//! `HashMap<StyleId, Rc<StyleRules>>`. Every wire command maps to
//! one `Backend` trait method call (or a small cluster).
//!
//! Event flow back to the dev side runs through closures the
//! replayer installs at command-apply time. Each closure captures a
//! `Sender<AppToDev>` plus the `HandlerId`; when the platform fires
//! the native event, the closure pushes an `AppToDev::Event` onto
//! the outbound channel.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;


use runtime_shared::{ColorScheme, StateBits, StyleRules};
// The replayer drives the platform through the capability traits. They
// are glob-imported so command dispatch can keep using plain
// method-call syntax (`b.create_view(..)`); `AllCaps` is the single
// bound that guarantees every one of them is present.
use runtime_scene::Host;
use runtime_vocabulary::caps;
#[allow(unused_imports)]
use runtime_vocabulary::caps::{
    A11yOps, ActivityIndicatorOps, AnimationOps, AppEnvOps, AssetOps, BatchOps, ButtonOps,
    DocumentOps, ExternalOps, GraphicsOps, IconOps, ImageOps, InputOps, IntrospectionOps,
    LifecycleOps, LinkOps, NavigatorOps, PortalOps, PresenceOps, PressableOps, SafeAreaOps,
    ScrollOps, SliderOps, StyleOps, TextInputOps, TextOps, ToggleOps, ViewOps, VirtualizerOps,
    WireBindingOps,
};
use wire::{
    AppToDev, Command, EventArgs, HandlerId, NodeId, ScopeId, StyleId, WireColorScheme,
    WireItemSize,
};

pub mod convert;
pub mod graphics;
pub mod navigators;
// Compatibility aliases for the replay client's historical names
// (`NewCoreReplayClient`, `WireBackend::new_newcore`). See the module.
pub mod newcore;

/// The runtime-server (Application-as-a-Server) **client-side replayer** —
/// wraps any `runtime_shared::Backend` and feeds it the wire
/// [`wire::Command`]s shipped by an
/// [`AasBackend`](dev_server::AasBackend). Idempotent
/// apply means re-sending a snapshot only does DOM work for the
/// commands that actually changed something.
///
/// ```text
/// UI tree → AasBackend → Wire → RuntimeServerClient<PlatformBackend> → Native
/// ```
///
/// The same `RuntimeServerClient` plugs into `WebBackend` on the browser,
/// `IosBackend` on iOS, `AndroidBackend` on Android — every
/// platform target the framework supports.
pub use crate::WireBackend as RuntimeServerClient;

// Transport, discovery, and the worker-thread `RuntimeServerShell` for native
// targets live in `runtime-server-shell-native` (under its `runtime-server` feature).
// Hosts on iOS / Android / desktop import them from there. The web
// transport (`web_sys::WebSocket` + rAF outbound pump) lives in
// `backend-web`'s `dev_transport` module under its `runtime-server`
// feature. This crate is platform-pure: protocol + replay engine
// only.

pub use graphics::{
    no_op_graphics_handlers, GraphicsRegistry, GraphicsRendererBundle, OnLostFactory,
    OnReadyFactory, OnResizeFactory,
};

/// Errors the replay engine can surface to the caller. Most are
/// "the dev side referenced something it shouldn't have" — i.e.
/// protocol violations that warrant a noisy panic in debug builds
/// but graceful skipping in production dev mode.
#[derive(Debug)]
pub enum ReplayError {
    UnknownNode(NodeId),
    UnknownStyle(StyleId),
    MissingHandler(HandlerId),
}

thread_local! {
    /// `StyleId`s we've already warned about, so a snapshot that
    /// references the same dropped style on every node doesn't spam the
    /// log once per node.
    static WARNED_STYLES: RefCell<std::collections::HashSet<StyleId>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Log (once per id) that a wire command referenced a `StyleId` with no
/// live `RegisterStyle`. The replay skips that style's application and
/// keeps rendering rather than blanking the frame — see the call sites
/// for why bailing was the wrong default.
fn warn_unknown_style(id: StyleId, ctx: &str) {
    let first = WARNED_STYLES.with(|w| w.borrow_mut().insert(id));
    if first {
        eprintln!(
            "[dev-client] {ctx}: unknown {id:?} (no RegisterStyle in this stream) — \
             skipping this style apply, node keeps its default; replay continues"
        );
    }
}

/// Outbound channel for messages flowing app → dev.
///
/// Wraps `Option<mpsc::Sender<AppToDev>>` behind an `Rc<RefCell<...>>`
/// so the transport can swap the underlying sender on reconnect.
/// Handler closures inside the `WireBackend` capture a clone of this
/// wrapper and call `.send(...)` — when the inner sender is `None`
/// (between reconnects) the event drops silently. When the wrapper
/// is rebound to a fresh sender, the same handler closures resume
/// delivering events to the new transport.
///
/// This is what enables the browser-side `WireBackend` to persist
/// across reconnects without losing event delivery.
#[derive(Clone)]
pub struct OutboundSender {
    inner: Rc<RefCell<Option<Sender<AppToDev>>>>,
}

impl OutboundSender {
    /// Construct an empty sender. Until [`Self::set`] is called,
    /// `send` calls drop silently.
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(None)),
        }
    }

    /// Construct a sender already bound to `tx`. Convenience for the
    /// simple `WireBackend::new(real_backend, tx)` call sites that
    /// don't need swappability.
    pub fn from_sender(tx: Sender<AppToDev>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Some(tx))),
        }
    }

    /// Retarget the wrapper at a new mpsc sender. Called by the
    /// transport on every successful connect.
    pub fn set(&self, tx: Sender<AppToDev>) {
        *self.inner.borrow_mut() = Some(tx);
    }

    /// Clear the wrapper. Used when a connection drops and there's
    /// no replacement yet. Subsequent sends drop until a new sender
    /// is bound.
    pub fn clear(&self) {
        *self.inner.borrow_mut() = None;
    }

    /// Send an event upstream. Returns `Ok(())` if delivered to the
    /// channel, `Err(())` if the wrapper is empty or the channel is
    /// disconnected (the message is dropped either way).
    pub fn send(&self, msg: AppToDev) -> Result<(), ()> {
        if let Some(tx) = self.inner.borrow().as_ref() {
            tx.send(msg).map_err(|_| ())
        } else {
            Err(())
        }
    }
}

impl Default for OutboundSender {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Sender<AppToDev>> for OutboundSender {
    fn from(tx: Sender<AppToDev>) -> Self {
        Self::from_sender(tx)
    }
}

/// The app-side replay engine. Generic over a `Backend` so a single
/// implementation covers iOS, Android, web, and any future target.
pub struct WireBackend<B: caps::AllCaps>
where
    B::Node: 'static,
{
    /// Shared backend handle. Pre-refactor this was `backend: B`
    /// (owned by value), which blocked the wgpu sim shell from
    /// sharing its `WgpuBackend` with `render-wgpu::Host` — Host
    /// keeps the backend behind its own `Rc<RefCell<>>` so its
    /// `Renderer::render` call can read scene state across the
    /// frame. With the wrapper here, the sim path constructs the
    /// backend once, wraps it, and hands the shared `Rc` to BOTH
    /// the wire backend (via [`Self::new_with_shared`]) and the
    /// Host. The iOS / Android / macOS shells still use the by-
    /// value [`Self::new`] which wraps internally; they don't
    /// share, so there's no observable behavior change for them.
    backend: Rc<RefCell<B>>,
    nodes: HashMap<NodeId, B::Node>,
    styles: HashMap<StyleId, Rc<StyleRules>>,
    outbound: OutboundSender,
    graphics_registry: GraphicsRegistry,
    /// Per-navigator state. Populated on `CreateNavigator` and consulted
    /// by the navigator control-plane commands.
    navigators: HashMap<NodeId, Rc<navigators::NavigatorAppState<B::Node>>>,
    /// Edges already realized in the backend's tree. Used by
    /// idempotent `Insert` so a re-applied command stream (after a
    /// reconnect) doesn't reorder or duplicate existing children.
    /// Set of `(parent, child)` pairs.
    inserted_edges: std::collections::HashSet<(NodeId, NodeId)>,
    /// Text content currently rendered for each text node — lets
    /// idempotent `CreateText` skip `update_text` calls when the
    /// content hasn't changed.
    text_content: HashMap<NodeId, String>,
    /// Button label currently rendered. Same role as `text_content`
    /// but for button label updates.
    button_labels: HashMap<NodeId, String>,
    /// Per-node idempotency guard for `Command::AttachStates`. Snapshot
    /// replay re-emits `AttachStates` for every styled node on every
    /// reconnect; without this guard, the backend would stack a fresh
    /// listener closure on top of the existing one and every state
    /// transition would fire the wire callback twice (or N times after
    /// N reconnects). Same shape as `inserted_edges`.
    attached_states: std::collections::HashSet<NodeId>,
    /// Nodes that opted into safe-area insets over the wire
    /// (`ApplySafeAreaPadding` / `ApplyScrollViewSafeAreaInset`), with the
    /// resolved client node, the `SafeAreaSides` flag, and whether it's the
    /// scroll-view (contentInset) variant. The dev side ships only the
    /// opt-in — it's headless and has no device insets — so the CLIENT
    /// backend resolves the real platform inset. A single shared effect
    /// (`safe_area_effect`) re-applies all of these whenever the client's
    /// `safe_area_insets()` signal changes (rotation / sheet adaptation),
    /// mirroring the framework's per-node `attach_safe_area` for the
    /// local-render path.
    #[allow(clippy::type_complexity)]
    safe_area_nodes: Rc<RefCell<Vec<(NodeId, B::Node, runtime_shared::SafeAreaSides, bool)>>>,
    /// The shared re-application effect, created lazily on the first
    /// safe-area opt-in. Owns its arena slot (created outside any scope),
    /// so it lives until this backend drops.
    safe_area_effect: Option<runtime_shared::Subscription>,
}

impl<B: caps::AllCaps + 'static> WireBackend<B>
where
    B::Node: 'static,
{
    /// Construct a wire backend bound to an outbound sender.
    /// Accept either a swappable `OutboundSender` directly or — via
    /// the `From<Sender<AppToDev>>` impl below — a raw `mpsc::Sender`
    /// for the common single-connection case.
    pub fn new(backend: B, outbound: impl Into<OutboundSender>) -> Self {
        Self::new_with_shared(Rc::new(RefCell::new(backend)), outbound)
    }

    /// Construct around a pre-shared backend handle. Used by hosts
    /// that hold their own `Rc<RefCell<B>>` alongside the wire
    /// layer — currently the wgpu sim runtime-server path, where
    /// `render-wgpu::Host` reads the backend on every redraw and
    /// the wire layer needs to write through the same `RefCell`.
    pub fn new_with_shared(
        backend: Rc<RefCell<B>>,
        outbound: impl Into<OutboundSender>,
    ) -> Self {
        Self {
            backend,
            nodes: HashMap::new(),
            styles: HashMap::new(),
            outbound: outbound.into(),
            graphics_registry: GraphicsRegistry::new(),
            navigators: HashMap::new(),
            inserted_edges: std::collections::HashSet::new(),
            text_content: HashMap::new(),
            button_labels: HashMap::new(),
            attached_states: std::collections::HashSet::new(),
            safe_area_nodes: Rc::new(RefCell::new(Vec::new())),
            safe_area_effect: None,
        }
    }

    /// Expose the outbound sender so the transport can retarget it on
    /// reconnect.
    pub fn outbound(&self) -> &OutboundSender {
        &self.outbound
    }

    /// Whether the wrapped real backend can capture its rendered
    /// surface. The shell ships this in its `AppToDev::Hello` so the
    /// server knows whether the `screenshot` verb's `client`/`auto`
    /// source can be served by a real-surface capture.
    pub fn supports_screenshot(&self) -> bool {
        self.backend.borrow().supports_screenshot()
    }

    /// Handle a [`DevToApp::CaptureScreenshot`]: capture the real
    /// backend's surface and reply with an [`AppToDev::ScreenshotResult`]
    /// carrying the same `request_id`. Native backends invoke the
    /// capture callback synchronously, so the reply is sent inline; the
    /// callback owns a clone of the outbound sender so an async backend
    /// (future web/DOM) would still reply when its capture completes.
    pub fn capture_screenshot_and_reply(&self, request_id: u64) {
        let outbound = self.outbound.clone();
        self.backend
            .borrow()
            .capture_screenshot(Box::new(move |result| {
                let msg = match result {
                    Ok(shot) => AppToDev::ScreenshotResult {
                        request_id,
                        png: Some(shot.png),
                        width: shot.width,
                        height: shot.height,
                        error: None,
                    },
                    Err(e) => AppToDev::ScreenshotResult {
                        request_id,
                        png: None,
                        width: 0,
                        height: 0,
                        error: Some(e),
                    },
                };
                let _ = outbound.send(msg);
            }));
    }

    /// Handle a [`DevToApp::QueryDeviceFrame`]: look the wire `node` up in
    /// the node map, ask the real backend for its physical screen-pixel
    /// rect (`Backend::device_frame`), and reply with an
    /// [`AppToDev::DeviceFrameResult`] carrying the same `request_id`.
    /// An unknown node id (or a backend that doesn't implement
    /// `device_frame`) replies with `found = false` so the server's
    /// `get_device_frame` verb reports "no frame" rather than hanging.
    pub fn query_device_frame_and_reply(&self, request_id: u64, node: NodeId) {
        let msg = match self.nodes.get(&node) {
            Some(n) => match self.backend.borrow().device_frame(n) {
                Some(r) => AppToDev::DeviceFrameResult {
                    request_id,
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    found: true,
                    error: None,
                },
                None => AppToDev::DeviceFrameResult {
                    request_id,
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    found: false,
                    error: None,
                },
            },
            None => AppToDev::DeviceFrameResult {
                request_id,
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                found: false,
                error: Some(format!("unknown node id {}", node.0)),
            },
        };
        let _ = self.outbound.send(msg);
    }

    /// Install a `GraphicsRegistry`, replacing whatever's there. The
    /// registry owns the app-local `(on_ready, on_resize, on_lost)`
    /// factories that the wire `CreateGraphics { renderer }` command
    /// looks up by name.
    pub fn set_graphics_registry(&mut self, registry: GraphicsRegistry) {
        self.graphics_registry = registry;
    }

    /// Mutable handle to the registry for in-place `register(...)` calls.
    pub fn graphics_registry_mut(&mut self) -> &mut GraphicsRegistry {
        &mut self.graphics_registry
    }

    /// Shared backend handle. Callers that need read-only access
    /// `.borrow()`; callers that need mutation `.borrow_mut()`.
    /// Cloning the returned `Rc` is the supported way to keep a
    /// long-lived reference (e.g. the wgpu renderer's per-frame
    /// reads).
    pub fn backend(&self) -> &Rc<RefCell<B>> {
        &self.backend
    }

    pub fn color_scheme(&self) -> ColorScheme {
        self.backend.borrow().color_scheme()
    }

    /// Apply a batch of commands. Each command is applied to the
    /// real backend; errors short-circuit the batch and surface to
    /// the caller (in production-dev, log + continue; in tests,
    /// fail loudly).
    pub fn apply_batch(&mut self, commands: Vec<Command>) -> Result<(), ReplayError> {
        for cmd in commands {
            self.apply(cmd)?;
        }
        Ok(())
    }

    /// Dispatch a single command.
    ///
    /// Apply is **idempotent for re-applied snapshots**: if a
    /// `Create*` command arrives for a `NodeId` we already have, we
    /// skip creating a new native node and (for content-bearing
    /// primitives like `Text` / `Button`) call the corresponding
    /// `update_*` only if the content actually changed. `Insert`
    /// remembers `(parent, child)` edges so re-applying doesn't
    /// reorder or duplicate. This is what lets the browser keep its
    /// `WireBackend` across reconnects — the new server can resend
    /// the full initial-mount snapshot and only real differences
    /// produce DOM work.
    pub fn apply(&mut self, cmd: Command) -> Result<(), ReplayError> {
        match cmd {
            Command::CreateView { id, a11y } => {
                if !self.nodes.contains_key(&id) {
                    let a11y = self.a11y_props(a11y);
                    let node = self.backend.borrow_mut().create_view(&a11y);
                    self.nodes.insert(id, node);
                }
            }
            Command::CreateText { id, content, a11y } => {
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    // Same node id, same content → no-op. Same id,
                    // different content → update_text.
                    let prev = self.text_content.get(&id);
                    if prev.map(|s| s.as_str()) != Some(content.as_str()) {
                        self.backend.borrow_mut().update_text(&existing, &content);
                        self.text_content.insert(id, content);
                    }
                    let _ = a11y;
                } else {
                    let a11y = self.a11y_props(a11y);
                    let node = self.backend.borrow_mut().create_text(&content, &a11y);
                    self.nodes.insert(id, node);
                    self.text_content.insert(id, content);
                }
            }
            Command::CreateButton {
                id,
                label,
                on_click,
                leading_icon,
                trailing_icon,
                a11y,
            } => {
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    // Button already exists. Update label if it
                    // changed. Re-binding the `on_click` handler
                    // without recreating the DOM node isn't exposed
                    // by the `Backend` trait — but in practice the
                    // wire's `HandlerId` allocation is positional
                    // and stable for unchanged structure, so the
                    // existing handler dispatch still routes to the
                    // right server-side closure. Icons aren't
                    // updated in place yet (TODO).
                    let prev = self.button_labels.get(&id);
                    if prev.map(|s| s.as_str()) != Some(label.as_str()) {
                        self.backend.borrow_mut().update_button_label(&existing, &label);
                        self.button_labels.insert(id, label);
                    }
                    // Drop the synthesized handler — the existing
                    // one stays attached.
                    let _ = (on_click, leading_icon, trailing_icon, a11y);
                    return Ok(());
                }
                let cb = self.handler_unit(on_click);
                let leading = leading_icon.map(convert::wire_icon_to_static);
                let trailing = trailing_icon.map(convert::wire_icon_to_static);
                // Wire side has no structured action metadata; wrap
                // the closure as an opaque Action and let the
                // backend's runtime path use `.fire`.
                let action = runtime_shared::IntoAction::into_action(move || cb());
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_button(
                    &label,
                    &action,
                    leading.as_ref(),
                    trailing.as_ref(),
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreatePressable { id, on_click, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_unit(on_click);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_pressable(cb, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateReactiveAnchor { id } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = Host::create_anchor(&mut *self.backend.borrow_mut());
                self.nodes.insert(id, node);
            }
            Command::CreateImage { id, src, alt, a11y } => {
                // Reconnect reconciliation (see CreateTextInput): re-apply the
                // folded src/alt onto an already-held node instead of dropping.
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    let mut b = self.backend.borrow_mut();
                    b.update_image_src(&existing, &src);
                    b.update_image_alt(&existing, alt.as_deref());
                    return Ok(());
                }
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_image(&src, alt.as_deref(), &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateIcon { id, data, color, a11y } => {
                let icon = convert::wire_icon_to_static(data);
                // Reconnect reconciliation: re-apply the folded geometry.
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    self.backend.borrow_mut().update_icon_data(&existing, &icon);
                    return Ok(());
                }
                let color = color.map(convert::wire_color_to_color);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_icon(&icon, color.as_ref(), &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateTextInput {
                id,
                initial_value,
                placeholder,
                on_change,
                secure,
                a11y,
            } => {
                // Reconnect reconciliation: a persisted client already holds
                // this node, so re-apply the snapshot's folded reactive fields
                // (value/placeholder/secure) rather than dropping them on the
                // early-return. The `update_*` methods no-op when unchanged.
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    let mut b = self.backend.borrow_mut();
                    b.update_text_input_value(&existing, &initial_value);
                    b.update_text_input_placeholder(&existing, placeholder.as_deref());
                    b.update_text_input_secure(&existing, secure);
                    return Ok(());
                }
                let cb = self.handler_string(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_text_input(
                    &initial_value,
                    placeholder.as_deref(),
                    cb,
                    None,
                    // on_blur: the veto closure can't cross the wire, so the
                    // dev-client proxy can't honor cancellation remotely.
                    None,
                    secure,
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateTextArea {
                id,
                initial_value,
                placeholder,
                wrap,
                min_rows,
                max_rows,
                on_change,
                a11y,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_string(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_text_area(
                    &initial_value,
                    placeholder.as_deref(),
                    wrap,
                    min_rows,
                    max_rows,
                    cb,
                    None,
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateExternal { id, type_name, payload, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let a11y_props = self.a11y_props(a11y);
                // Reconstruct the payload via the SDK's registered external
                // serde and dispatch to the REAL handler (the RS client
                // links a fixed set of compiled-in SDK handlers, RN-style).
                // The concrete `TypeId` is read off the deserialized
                // payload, so the backend's `ExternalRegistry` (keyed by
                // that same payload type) finds the handler.
                if let Some(payload_any) =
                    wire::deserialize_external_payload(&type_name, &payload)
                {
                    let type_id = (*payload_any).type_id();
                    // The wire type_name is owned; the backend wants
                    // `&'static str` (debug/error use only). Lazy-intern.
                    let type_name_static: &'static str =
                        Box::leak(type_name.clone().into_boxed_str());
                    let node = self.backend.borrow_mut().create_external(
                        type_id,
                        type_name_static,
                        &payload_any,
                        &a11y_props,
                    );
                    self.nodes.insert(id, node);
                } else {
                    // No serde registered for this external (or empty
                    // sentinel payload, or decode failed). Either the SDK
                    // didn't register a wire serde, or the server/client SDK
                    // sets are desynced. Surface it (RN-style) rather than
                    // silently rendering a blank box.
                    runtime_shared::log(
                        runtime_shared::LogLevel::Warn,
                        &format!(
                            "[wire] external '{}' has no client-side payload serde \
                             — register one via `wire::register_external_serde`, \
                             or the server/client SDK sets are desynced",
                            type_name
                        ),
                    );
                    let mut node = self.backend.borrow_mut().create_view(&a11y_props);
                    let label = format!("Component not available: {}", type_name);
                    let text_a11y = runtime_shared::accessibility::AccessibilityProps::default();
                    let text_node = self.backend.borrow_mut().create_text(&label, &text_a11y);
                    self.backend.borrow_mut().insert(&mut node, text_node);
                    self.nodes.insert(id, node);
                }
            }
            Command::CreateToggle {
                id,
                initial_value,
                on_change,
                a11y,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_bool(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_toggle(initial_value, cb, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateSlider {
                id,
                initial_value,
                min,
                max,
                step,
                on_change,
                a11y,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_float(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_slider(initial_value, min, max, step, cb, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateScrollView { id, horizontal, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let a11y = self.a11y_props(a11y);
                // `on_scroll` is `None`: the wire protocol doesn't yet
                // ferry user `on_scroll` callbacks across server/client
                // boundary (it would need a per-scroll-event message
                // back to the server). The client-side backend's own
                // scroll affordance (Position::Sticky, scrollbars,
                // etc.) still works because those are handled locally.
                let node = self.backend.borrow_mut().create_scroll_view(horizontal, None, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateActivityIndicator { id, size, color, a11y } => {
                let size = convert::wire_activity_size(size);
                // Reconnect reconciliation: re-apply the folded size.
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    self.backend.borrow_mut().update_activity_indicator_size(&existing, size);
                    return Ok(());
                }
                let color = color.map(convert::wire_color_to_color);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_activity_indicator(size, color.as_ref(), &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateLink {
                id,
                route,
                url,
                kind: _,
                on_activate,
                external,
                a11y,
            } => {
                // Reconnect reconciliation: re-apply the folded url.
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    self.backend.borrow_mut().update_link_url(&existing, &url);
                    return Ok(());
                }
                let cb = self.handler_unit(on_activate);
                let route_static: &'static str = Box::leak(route.into_boxed_str());
                let config = runtime_shared::primitives::link::LinkConfig {
                    route: route_static,
                    url,
                    external,
                    on_activate: cb,
                };
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_link(config, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreatePortal {
                id,
                target,
                on_dismiss,
                trap_focus,
                a11y,
            } => {
                use runtime_shared::primitives::portal::{
                    ElementAlign, ElementSide, PortalTarget, ViewportPlacement,
                };
                // runtime-server doesn't have a way to reconstruct a live
                // `AnchorTarget` from a wire `NodeId` — that would
                // need a bidirectional rect-query plumbed through
                // the wire. For Anchor variants we collapse to a
                // centered viewport portal so it still mounts
                // visibly; popovers/tooltips that need real
                // anchoring should be authored against a runtime
                // backend, not over runtime-server.
                let portal_target = match target {
                    wire::WirePortalTarget::Viewport(p) => {
                        let placement = match p {
                            wire::WireViewportPlacement::Center => ViewportPlacement::Center,
                            wire::WireViewportPlacement::Top => ViewportPlacement::Top,
                            wire::WireViewportPlacement::Bottom => ViewportPlacement::Bottom,
                            wire::WireViewportPlacement::Left => ViewportPlacement::Left,
                            wire::WireViewportPlacement::Right => ViewportPlacement::Right,
                            wire::WireViewportPlacement::FullScreen => {
                                ViewportPlacement::FullScreen
                            }
                        };
                        PortalTarget::Viewport(placement)
                    }
                    wire::WirePortalTarget::Anchor { .. } => {
                        let _ = (ElementSide::Below, ElementAlign::Start);
                        PortalTarget::Viewport(ViewportPlacement::Center)
                    }
                    wire::WirePortalTarget::Named(_) => {
                        PortalTarget::Viewport(ViewportPlacement::Center)
                    }
                };
                let dismiss_cb: Option<Rc<dyn Fn()>> =
                    on_dismiss.map(|h| self.handler_unit(h));
                if self.nodes.contains_key(&id) { return Ok(()); }
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_portal(
                    portal_target,
                    dismiss_cb,
                    trap_focus,
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateGraphics { id, renderer, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                // Look up the renderer in the app-local registry. If
                // absent, the Graphics surface is still created (so the
                // tree layout stays correct) but no GPU code runs.
                let lookup = self.graphics_registry.lookup(&renderer);
                let (on_ready, on_resize, on_lost) = match lookup {
                    Some(triple) => triple,
                    None => no_op_graphics_handlers(),
                };
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_graphics(on_ready, on_resize, on_lost, &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateVirtualizer {
                id,
                overscan,
                layout,
                initial_size,
                initial_keys,
                a11y,
            } => {
                self.apply_create_virtualizer(
                    id, overscan, layout, initial_size, initial_keys, a11y,
                );
            }
            Command::CreateNavigator { id, initial_route, initial_path, a11y } => {
                self.apply_create_navigator(id, initial_route, initial_path, a11y);
            }

            // --- Tree mutation ---
            Command::Insert { parent, child } => {
                // Idempotent: if this edge was already realized,
                // skip — re-inserting would re-order in the DOM
                // (move to end of parent's children) which would
                // disturb a tree the user is currently looking at.
                if self.inserted_edges.contains(&(parent, child)) {
                    return Ok(());
                }
                let child_node = self
                    .nodes
                    .get(&child)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(child))?;
                let parent_node = self
                    .nodes
                    .get_mut(&parent)
                    .ok_or(ReplayError::UnknownNode(parent))?;
                self.backend.borrow_mut().insert(parent_node, child_node);
                self.inserted_edges.insert((parent, child));
            }
            Command::InsertMany { parent, children } => {
                // Filter out edges already realized.
                let mut children_to_insert = Vec::with_capacity(children.len());
                for c in children {
                    if !self.inserted_edges.contains(&(parent, c)) {
                        children_to_insert.push(c);
                    }
                }
                if children_to_insert.is_empty() {
                    return Ok(());
                }
                let mut children_nodes = Vec::with_capacity(children_to_insert.len());
                for child_id in &children_to_insert {
                    let child = self
                        .nodes
                        .get(child_id)
                        .cloned()
                        .ok_or(ReplayError::UnknownNode(*child_id))?;
                    children_nodes.push(child);
                }
                let parent_node = self
                    .nodes
                    .get_mut(&parent)
                    .ok_or(ReplayError::UnknownNode(parent))?;
                self.backend.borrow_mut().insert_many(parent_node, children_nodes);
                for c in children_to_insert {
                    self.inserted_edges.insert((parent, c));
                }
            }
            Command::ClearChildren { node } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().clear_children(&n);
                // Forget every edge whose parent was just cleared.
                self.inserted_edges.retain(|(p, _)| *p != node);
            }

            // --- Reactive updates ---
            Command::UpdateText { node, content } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_text(&n, &content);
            }
            Command::UpdateButtonLabel { node, label } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_button_label(&n, &label);
            }
            Command::UpdateImageSrc { node, src } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_image_src(&n, &src);
            }
            Command::UpdateLinkUrl { node, url } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_link_url(&n, &url);
            }
            Command::UpdateImageAlt { node, alt } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_image_alt(&n, alt.as_deref());
            }
            Command::UpdateIconColor { node, color } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let c = convert::wire_color_to_color(color);
                self.backend.borrow_mut().update_icon_color(&n, &c);
            }
            Command::UpdateIconData { node, data } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let d = convert::wire_icon_to_static(data);
                self.backend.borrow_mut().update_icon_data(&n, &d);
            }
            Command::UpdateIconStroke { node, progress } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_icon_stroke(&n, progress);
            }
            Command::AnimateIconStroke {
                node,
                from,
                to,
                duration_ms,
                easing,
                infinite,
                autoreverses,
            } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let e = convert::wire_easing(easing);
                self.backend.borrow_mut()
                    .animate_icon_stroke(&n, from, to, duration_ms, e, infinite, autoreverses);
            }
            Command::UpdateTextInputValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_text_input_value(&n, &value);
            }
            Command::UpdateTextInputSecure { node, secure } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_text_input_secure(&n, secure);
            }
            Command::UpdateTextInputPlaceholder { node, placeholder } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_text_input_placeholder(&n, placeholder.as_deref());
            }
            Command::UpdateTextAreaValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_text_area_value(&n, &value);
            }
            Command::UpdateToggleValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_toggle_value(&n, value);
            }
            Command::UpdateSliderValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().update_slider_value(&n, value);
            }
            Command::UpdateActivityIndicatorSize { node, size } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let s = convert::wire_activity_size(size);
                self.backend.borrow_mut().update_activity_indicator_size(&n, s);
            }
            Command::SetDisabled { node, disabled } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().set_disabled(&n, disabled);
            }
            Command::ApplySafeAreaPadding { node, sides } => {
                self.register_safe_area(node, sides, false)?;
            }
            Command::ApplyScrollViewSafeAreaInset { node, sides } => {
                self.register_safe_area(node, sides, true)?;
            }

            // --- Animation ticks (per-frame, high-frequency) ---
            //
            // The dev side resolves the animation curve and ships a
            // value per tick; the client just dispatches to the
            // wrapped backend's per-platform `set_animated_*` impl.
            // Unknown nodes get logged + dropped rather than aborting
            // the batch — animation deltas are idempotent (next tick
            // supersedes), so a one-frame skip on a transient race
            // (e.g. a node was just released but the in-flight tick
            // hadn't been canceled yet on the sidecar) is invisible.
            Command::SetAnimatedF32 { node, prop, value } => {
                if let Some(n) = self.nodes.get(&node).cloned() {
                    if let Some(p) = convert::wire_anim_prop(prop) {
                        self.backend.borrow_mut().set_animated_f32(&n, p, value);
                    }
                }
            }
            Command::SetAnimatedColor { node, prop, value } => {
                if let Some(n) = self.nodes.get(&node).cloned() {
                    if let Some(p) = convert::wire_anim_prop(prop) {
                        self.backend.borrow_mut().set_animated_color(&n, p, value);
                    }
                }
            }

            // --- Styles ---
            Command::RegisterStyle { id, rules } => {
                let resolved: Rc<StyleRules> = Rc::new(convert::wire_style_to_rules(rules));
                // Notify the backend so it can mint platform-side state
                // (web class caching, etc.). Wrapping in a slice mirrors
                // the Backend signature.
                self.backend.borrow_mut().register_stylesheet(std::slice::from_ref(&resolved));
                self.styles.insert(id, resolved);
            }
            Command::UnregisterStyle { id } => {
                if let Some(rules) = self.styles.remove(&id) {
                    self.backend.borrow_mut().unregister_stylesheet(std::slice::from_ref(&rules));
                }
            }
            Command::ApplyStyle { node, style } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                // Degrade gracefully on an unknown style rather than
                // aborting the whole batch. A snapshot can legitimately
                // reference a `StyleId` whose `RegisterStyle` was dropped
                // (a stylesheet unregistered while a node still carried
                // its `ApplyStyle` — the scene-mirror's
                // unregister-while-referenced gap). Bailing here renders a
                // BLANK frame for the entire scene (the reported all-white
                // screenshot); skipping just this one apply leaves the
                // node with its create-time default style and lets the
                // rest of the tree render. Warn so the gap is still
                // visible in logs.
                match self.styles.get(&style).cloned() {
                    Some(s) => self.backend.borrow_mut().apply_style(&n, &s),
                    None => warn_unknown_style(style, "ApplyStyle"),
                }
            }
            Command::ApplyStyledStates { node, base, overlays } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                // Same graceful-degrade rationale as `ApplyStyle`: a
                // missing base or overlay style skips this node's styled-
                // state apply instead of blanking the frame.
                let Some(b) = self.styles.get(&base).cloned() else {
                    warn_unknown_style(base, "ApplyStyledStates(base)");
                    return Ok(());
                };
                let mut o: Vec<(StateBits, Rc<StyleRules>)> = Vec::with_capacity(overlays.len());
                for (bit, sid) in overlays {
                    let bits = convert::wire_state_bit(bit);
                    match self.styles.get(&sid).cloned() {
                        Some(rules) => o.push((bits, rules)),
                        None => warn_unknown_style(sid, "ApplyStyledStates(overlay)"),
                    }
                }
                self.backend.borrow_mut().apply_styled_states(&n, &b, &o);
            }
            Command::AttachStates { node } => {
                // Idempotency: snapshot replay re-emits `AttachStates`
                // for every styled node on every reconnect; without
                // this guard the backend would stack a fresh listener
                // closure on top of the existing one and every state
                // transition would fire the wire callback twice (or N
                // times after N reconnects). Same shape as
                // `inserted_edges`.
                if !self.attached_states.insert(node) {
                    return Ok(());
                }
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let outbound = self.outbound.clone();
                let node_id = node;
                self.backend.borrow_mut().attach_states(
                    &n,
                    Rc::new(move |bits: StateBits, on: bool| {
                        // Decompose into single-bit transitions for
                        // wire simplicity. (Most state activations are
                        // single-bit anyway.)
                        for axis in bits.active_axes() {
                            let bit = convert::axis_name_to_wire_state(axis);
                            if let Some(bit) = bit {
                                let _ = outbound.send(AppToDev::StateChanged {
                                    node: node_id,
                                    bit,
                                    on,
                                });
                            }
                        }
                    }),
                );
            }
            Command::OnNodeUnstyled { node } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().on_node_unstyled(&n);
            }

            // --- Presence ---
            Command::ApplyPresence {
                node,
                state,
                transition,
            } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let s = convert::wire_presence_state(state);
                let t = transition.map(|(d, e)| (d, convert::wire_easing(e)));
                self.backend.borrow_mut().apply_presence(&n, s, t);
            }

            // --- Navigator control plane ---
            Command::NavigatorAttachInitial {
                navigator,
                screen,
                scope,
                options,
            } => {
                // URL-based replay dedup. After a server rebuild-exec,
                // the append-only log is replayed with fresh scope
                // ids; the iOS/web client still has the previous
                // native stack alive. Compare against `mounted_urls`
                // at the current `replay_pos`: if it matches, skip
                // the actual attach and just advance the cursor.
                // Scope-id-based dedup wouldn't work because scope
                // ids are session-local (server reallocates on each
                // process restart).
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                let url = state.initial_path.clone();
                {
                    let urls = state.mounted_urls.borrow();
                    let pos = *state.replay_pos.borrow();
                    if pos < urls.len() && urls[pos] == url {
                        drop(urls);
                        *state.replay_pos.borrow_mut() = pos + 1;
                        return Ok(());
                    }
                }
                let screen_node = self.lookup_node(screen)?;
                if state.native {
                    // Native path: hand the pre-built screen to the
                    // registered handler, which mounts it into its own
                    // native body outlet (e.g.
                    // `web_navigator_helpers::attach_initial`). The
                    // handler ignores the structural `state.outlet`.
                    self.backend.borrow_mut().navigator_attach_initial(
                        &state.node,
                        screen_node,
                        scope.0,
                        Box::new(()),
                    );
                    state.screen_stack.borrow_mut().push(screen);
                    state.mounted_urls.borrow_mut().push(url);
                    *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
                    return Ok(());
                }
                let _opts = convert::wire_screen_options(&options, |id| self.handler_unit(id));
                let _ = scope;
                // Mount the initial screen subtree into the navigator's
                // outlet. The recorder built the screen as a floating
                // primitive subtree (no parent edge); this is what makes
                // it visible.
                let mut outlet = state.outlet.clone();
                self.backend.borrow_mut().insert(&mut outlet, screen_node);
                state.screen_stack.borrow_mut().push(screen);
                state.mounted_urls.borrow_mut().push(url);
                *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
            }
            Command::NavigatorPush {
                navigator,
                screen,
                scope,
                options,
                url,
                restore,
            } => {
                self.dispatch_push_like(
                    navigator, screen, scope, options, NavOp::Push, url, restore,
                )?;
            }
            Command::NavigatorReplace {
                navigator,
                screen,
                scope,
                options,
                url,
                restore,
            } => {
                self.dispatch_push_like(
                    navigator, screen, scope, options, NavOp::Replace, url, restore,
                )?;
            }
            Command::NavigatorReset {
                navigator,
                screen,
                scope,
                options,
                url,
                restore,
            } => {
                self.dispatch_push_like(
                    navigator, screen, scope, options, NavOp::Reset, url, restore,
                )?;
            }
            Command::NavigatorSelect {
                navigator,
                screen,
                scope,
                options,
                url,
            } => {
                // Select is dispatched as `NavCommand::Select` to the
                // select-style (swap) navigator. Pre-fix this was
                // conflated with `Reset`, which drained the snapshot
                // model's per-screen state (a Reset means "discard
                // stack and mount new root"; a Select means "switch
                // active screen"). The dev-server now emits
                // `NavigatorSelect` for the select-flavored push-like.
                self.dispatch_push_like(
                    navigator, screen, scope, options, NavOp::Select, url, false,
                )?;
            }
            Command::NavigatorPop { navigator, count } => {
                // Pop `count` frames off the tracked screen stack and
                // re-show the new top in the outlet. Popped screen nodes
                // stay in `self.nodes` (just detached) — the dev side
                // releases their scopes via the recorder.
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                let top = {
                    let mut st = state.screen_stack.borrow_mut();
                    for _ in 0..count {
                        if st.len() <= 1 {
                            break;
                        }
                        st.pop();
                    }
                    st.last().copied()
                };
                if let Some(top) = top {
                    let top_node = self.lookup_node(top)?;
                    let mut outlet = state.outlet.clone();
                    let mut backend = self.backend.borrow_mut();
                    backend.clear_children(&outlet);
                    backend.insert(&mut outlet, top_node);
                }
            }

            // --- Layout attach (web-only effectively) ---
            Command::AttachNavigatorLayout {
                navigator,
                root,
                outlet,
            } => {
                // Dev wire layout attach stubbed pending protocol
                // redesign; legacy Backend trait method removed.
                let _ = (navigator, root, outlet);
            }

            // --- Navigator chrome styles ---
            // Dev wire navigator chrome dispatch is stubbed pending
            // protocol redesign for the SDK-based navigator model. The
            // old Backend trait methods (apply_navigator_*_style etc.)
            // were removed when the per-kind nav surface left core; the
            // wire ops below remain in the protocol but no longer have
            // a generic backend target. Wire navigators no-op until
            // the protocol is reworked to drive through the SDK's
            // `NavigatorHandler::apply_slot_style`.
            Command::ApplyNavigatorHeaderStyle { .. }
            | Command::ApplyNavigatorTitleStyle { .. }
            | Command::ApplyNavigatorButtonStyle { .. }
            | Command::ApplyNavigatorBodyStyle { .. } => {
                // no-op: dev wire navigator chrome TBD post-SDK migration
            }

            // --- Virtualizer control plane ---
            Command::VirtualizerDataChanged { node, item_count: _ } => {
                let n = self.lookup_node(node)?;
                self.backend.borrow_mut().virtualizer_data_changed(&n);
            }
            Command::VirtualizerAttachItem { .. } => {
                // Lazy-mount path for virtualizer items. The wire
                // command carries the pre-built subtree but the
                // current Backend trait doesn't expose an
                // "attach pre-built item" method — the framework's
                // VirtualizerCallbacks::mount_item is what drives
                // attachment in normal operation. Plumbing this
                // through requires the same pending-mount-slot
                // pattern as navigators, applied to virtualizer's
                // callback bundle. Deferred to a follow-up.
            }

            Command::Finish { root } => {
                let n = self
                    .nodes
                    .get(&root)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(root))?;
                self.backend.borrow_mut().finish(n);
            }
            Command::ReleaseNode { node } => {
                // Mirror `SceneModel::apply(Command::ReleaseNode)` — clear
                // every per-node map so a hot-reload that releases and
                // re-creates the same logical primitive doesn't leak the
                // old node's bookkeeping. Pre-fix, only `self.nodes` was
                // cleared, leaving `text_content` / `button_labels` /
                // `inserted_edges` / `navigators` to accumulate forever.
                self.nodes.remove(&node);
                self.text_content.remove(&node);
                self.button_labels.remove(&node);
                self.navigators.remove(&node);
                self.attached_states.remove(&node);
                self.inserted_edges
                    .retain(|(parent, child)| *parent != node && *child != node);
                self.safe_area_nodes
                    .borrow_mut()
                    .retain(|(id, ..)| *id != node);
            }
            Command::InstallThemeVariables { tokens } => {
                // Populate THIS (wire-replay) thread's thread-local token
                // registry so device-side token resolution works. Native
                // SDK handlers run on the client over the wire — e.g. a
                // handler's chrome resolves `color-background` via
                // `Tokenized::resolve()`. The token registry is
                // thread-local and the app's `install_theme` ran on the
                // HOST, not here; without installing the forwarded tokens
                // on this thread, `resolve()` panics (style.rs), which
                // aborts the apply batch and leaves the tree incomplete
                // (blank screen). The host forwards its installed tokens
                // via this command (see dev-server `install_tokens`).
                // Token names are a small, install-once set, so leaking
                // them to `&'static str` is fine.
                let entries: Vec<runtime_shared::TokenEntry> = tokens
                    .into_iter()
                    .filter_map(|t| {
                        let value = match t.value {
                            wire::WireTokenValue::Color(c) => {
                                runtime_shared::TokenValue::Color(convert::wire_color_to_color(c))
                            }
                            wire::WireTokenValue::Number(n) => {
                                runtime_shared::TokenValue::Number(n)
                            }
                            wire::WireTokenValue::Length(l) => {
                                runtime_shared::TokenValue::Length(convert::wire_length(l))
                            }
                            // Core `TokenValue` has no String variant and
                            // the recorder never emits one — skip cleanly.
                            wire::WireTokenValue::String(_) => return None,
                        };
                        let name: &'static str = Box::leak(t.name.into_boxed_str());
                        Some(runtime_shared::TokenEntry { name, value })
                    })
                    .collect();
                // Thread-local registry (fixes the resolve-on-unthemed-thread
                // panic), then let the backend apply backend-specific theme
                // variables too (web sets CSS custom properties; native
                // backends typically no-op).
                runtime_shared::install_tokens(&entries);
                self.backend.borrow_mut().install_tokens(&entries);
            }
            Command::RegisterAsset { id, kind, source } => {
                let core_id = convert::wire_asset_id(id);
                let core_kind = convert::wire_asset_tag(kind);
                let core_source = convert::wire_asset_source(source);
                self.backend.borrow_mut().register_asset(core_id, core_kind, &core_source);
            }
            Command::UnregisterAsset { id, kind } => {
                self.backend.borrow_mut().unregister_asset(
                    convert::wire_asset_id(id),
                    convert::wire_asset_tag(kind),
                );
            }
            Command::RegisterTypeface {
                id,
                family_name,
                faces,
                fallback,
            } => {
                let core_id = convert::wire_typeface_id(id);
                let core_faces: Vec<_> =
                    faces.into_iter().map(convert::wire_typeface_face).collect();
                let core_fallback = convert::wire_system_fallback(fallback);
                self.backend.borrow_mut().register_typeface(
                    core_id,
                    &family_name,
                    &core_faces,
                    core_fallback,
                );
            }
            Command::UnregisterTypeface { id } => {
                self.backend.borrow_mut().unregister_typeface(convert::wire_typeface_id(id));
            }

            // --- Accessibility ---
            Command::UpdateAccessibility {
                id,
                a11y,
                inferred_role,
            } => {
                let n = self.lookup_node(id)?;
                let props = self.a11y_props(a11y);
                let role = inferred_role.and_then(convert::wire_role_to_role);
                self.backend.borrow_mut().update_accessibility(&n, &props, role);
            }
            Command::AnnounceForAccessibility { msg, priority } => {
                let priority = convert::wire_live_region_to_priority(priority);
                self.backend.borrow_mut().announce_for_accessibility(&msg, priority);
            }

            // Host surface / document chrome. Colors arrive pre-resolved
            // (the recorder resolves tokens dev-side), so we hand the real
            // backend a `Tokenized::Literal` — the client's backend then
            // does its native thing (web sets the CSS var-free literal).
            Command::SetAppBackground { color } => {
                self.backend
                    .borrow_mut()
                    .set_app_background(&runtime_shared::Tokenized::Literal(runtime_shared::Color(
                        color.0,
                    )));
            }
            Command::SetScrollbarTheme { thumb, track } => {
                self.backend.borrow_mut().set_scrollbar_theme(
                    &runtime_shared::Tokenized::Literal(runtime_shared::Color(thumb.0)),
                    &runtime_shared::Tokenized::Literal(runtime_shared::Color(track.0)),
                );
            }
            Command::SetPageMetadata { meta } => {
                self.backend
                    .borrow_mut()
                    .set_page_metadata(&runtime_shared::PageMetadata {
                        title: meta.title,
                        description: meta.description,
                        og_image: meta.og_image,
                        canonical_url: meta.canonical_url,
                    });
            }
            Command::RegisterRawCss { css } => {
                self.backend.borrow_mut().register_raw_css(&css);
            }
        }
        Ok(())
    }

    /// Build a unit closure that, when called, sends an `Event` back
    /// to the dev side. Used for `on_click` style handlers.
    fn handler_unit(&self, id: HandlerId) -> Rc<dyn Fn()> {
        let outbound = self.outbound.clone();
        Rc::new(move || {
            let _ = outbound.send(AppToDev::Event {
                handler: id,
                args: EventArgs::Unit,
            });
        })
    }

    /// Reconstruct an in-memory `AccessibilityProps` from its wire
    /// form. Action handlers go through the same trampoline factory
    /// as `on_click` — each `WireAccessibilityAction.handler` becomes
    /// a closure that posts `AppToDev::Event { handler, args: Unit }`
    /// over the reverse channel, so AT-triggered rotor / TalkBack
    /// actions on the app side dispatch the dev-side closure that was
    /// registered when the primitive was built.
    fn a11y_props(
        &self,
        a11y: wire::WireAccessibilityProps,
    ) -> runtime_shared::accessibility::AccessibilityProps {
        let outbound = self.outbound.clone();
        convert::wire_a11y_to_props(a11y, move |id| {
            let outbound = outbound.clone();
            Rc::new(move || {
                let _ = outbound.send(AppToDev::Event {
                    handler: id,
                    args: EventArgs::Unit,
                });
            })
        })
    }

    fn handler_bool(&self, id: HandlerId) -> Rc<dyn Fn(bool)> {
        let outbound = self.outbound.clone();
        Rc::new(move |v| {
            let _ = outbound.send(AppToDev::Event {
                handler: id,
                args: EventArgs::Bool(v),
            });
        })
    }

    fn handler_float(&self, id: HandlerId) -> Rc<dyn Fn(f32)> {
        let outbound = self.outbound.clone();
        Rc::new(move |v| {
            let _ = outbound.send(AppToDev::Event {
                handler: id,
                args: EventArgs::Float(v),
            });
        })
    }

    fn handler_string(&self, id: HandlerId) -> Rc<dyn Fn(String)> {
        let outbound = self.outbound.clone();
        Rc::new(move |v| {
            let _ = outbound.send(AppToDev::Event {
                handler: id,
                args: EventArgs::String(v),
            });
        })
    }

    fn lookup_node(&self, id: NodeId) -> Result<B::Node, ReplayError> {
        self.nodes
            .get(&id)
            .cloned()
            .ok_or(ReplayError::UnknownNode(id))
    }

    fn lookup_style(&self, id: StyleId) -> Result<Rc<StyleRules>, ReplayError> {
        self.styles
            .get(&id)
            .cloned()
            .ok_or(ReplayError::UnknownStyle(id))
    }

    fn apply_create_navigator(
        &mut self,
        id: NodeId,
        initial_route: String,
        initial_path: String,
        a11y: wire::WireAccessibilityProps,
    ) {
        // Idempotency. Pre-existing navigator: reset replay cursor
        // and keep the current mount.
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        // Dev wire stack-navigator creation stubbed pending SDK-
        // dispatch wire-protocol redesign. We still create a
        // placeholder backend view so subsequent commands have a node
        // to target, but no per-kind native nav container is built.
        let _ = (initial_route,);
        let a11y_props = self.a11y_props(a11y);
        let nav_node = self.backend.borrow_mut().create_view(&a11y_props);

        let control = Rc::new(runtime_shared::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Stack,
            node: nav_node.clone(),
            // Stack reconstruction is Phase 7; mount screens straight
            // into the nav node for now so the active screen renders.
            outlet: nav_node.clone(),
            screen_stack: Rc::new(RefCell::new(Vec::new())),
            control,
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path,
            mounted_urls,
            replay_pos,
            native: false,
        });

        self.nodes.insert(id, nav_node);
        self.navigators.insert(id, final_state);
    }

    fn apply_create_virtualizer(
        &mut self,
        _id: NodeId,
        _overscan: f32,
        _layout: wire::WireVirtualLayout,
        _initial_size: WireItemSize,
        _initial_keys: Vec<u64>,
        _a11y: wire::WireAccessibilityProps,
    ) {
        // Virtualizer replay requires the same pending-mount-slot
        // pattern as navigators, applied to VirtualizerCallbacks's
        // mount_item / release_item / item_count / item_key /
        // item_size closures. The wire vocabulary and the
        // dev-side recorder both cover virtualizers; this replay
        // path is the remaining piece. Deferred to a follow-up so
        // navigators (the more commonly-needed primitive) ship
        // first.
    }

    /// Register a wire safe-area opt-in: resolve the node, apply the inset
    /// once via the client backend (which reads its OWN device insets — the
    /// dev side is headless and ships only `sides`), and ensure the shared
    /// re-application effect exists so rotation / sheet adaptation re-apply.
    /// Idempotent: snapshot replay re-emits this on every reconnect, so a
    /// prior entry for the node is replaced rather than stacked.
    fn register_safe_area(
        &mut self,
        node: NodeId,
        sides: u8,
        is_scroll: bool,
    ) -> Result<(), ReplayError> {
        let n = self
            .nodes
            .get(&node)
            .ok_or(ReplayError::UnknownNode(node))?
            .clone();
        let sides = runtime_shared::SafeAreaSides(sides);
        {
            let mut list = self.safe_area_nodes.borrow_mut();
            list.retain(|(id, ..)| *id != node);
            list.push((node, n.clone(), sides, is_scroll));
        }
        {
            let mut b = self.backend.borrow_mut();
            if is_scroll {
                b.apply_scroll_view_safe_area_inset(&n, sides);
            } else {
                b.apply_safe_area_padding(&n, sides);
            }
        }
        self.ensure_safe_area_effect();
        Ok(())
    }

    /// Create — once — the shared effect that re-applies every registered
    /// safe-area opt-in whenever the CLIENT's `safe_area_insets()` signal
    /// changes (rotation, sheet adaptation, dynamic island). This is the
    /// device-side analogue of the framework's per-node `attach_safe_area`
    /// effect, which over the wire only ran on the headless (ZERO-inset)
    /// dev side. A caller-owned `watch` stored on the backend (created with
    /// no active scope), so its `Subscription` lives until this backend drops.
    fn ensure_safe_area_effect(&mut self) {
        if self.safe_area_effect.is_some() {
            return;
        }
        let backend = self.backend.clone();
        let nodes = self.safe_area_nodes.clone();
        self.safe_area_effect = Some(runtime_shared::watch(move || {
            // Subscribe to the device insets; the backend reads the
            // concrete platform value itself inside the apply calls.
            let _ = runtime_shared::safe_area_insets().get();
            let mut b = backend.borrow_mut();
            for (_, node, sides, is_scroll) in nodes.borrow().iter() {
                if *is_scroll {
                    b.apply_scroll_view_safe_area_inset(node, *sides);
                } else {
                    b.apply_safe_area_padding(node, *sides);
                }
            }
        }));
    }

    fn dispatch_push_like(
        &mut self,
        navigator: NodeId,
        screen: NodeId,
        scope: ScopeId,
        options: wire::WireScreenOptions,
        op: NavOp,
        url: String,
        _restore: bool,
    ) -> Result<(), ReplayError> {
        // Dev wire push/replace/reset/select dispatch is stubbed
        // pending the SDK-driven navigator wire-protocol redesign.
        // The legacy callback layer this method used to drive (via
        // `NavigatorControl::dispatch` plus a pending mount) has
        // been removed from runtime-core. We still maintain the
        // mounted_urls/replay_pos bookkeeping so dedup logic
        // continues to behave deterministically across reconnects.
        let state = self
            .navigators
            .get(&navigator)
            .cloned()
            .ok_or(ReplayError::UnknownNode(navigator))?;

        if state.native {
            // Native handler owns the body outlet AND the navigate +
            // auto-close logic (the registered SDK handler installed a
            // dispatcher on `state.control` at create time). So drive that
            // dispatcher — the SAME path local (non-wire) mode uses —
            // rather than inserting into the structural `state.outlet`,
            // which native handlers ignore (mirrors the `state.native`
            // branch in `NavigatorAttachInitial`). Stage the wire-built
            // screen node; the handler's `mount_screen` (wired to
            // `pending_mount`) hands it back; any handler-side reaction
            // to the Select (chrome updates etc.) runs on its own — no
            // kind-specific wire command needed for navigation.
            use runtime_shared::primitives::navigator::{split_query, MountResult, NavCommand};
            let screen_node = self.lookup_node(screen)?;
            // The server rebuilds + ships a fresh node per select and owns
            // screen lifecycle, so the client must not reuse a cached view.
            // The interned URL is a stable route key for active-route
            // bookkeeping.
            // Interned from the PATH, not the full URL: the query is screen
            // state, so `/inbox?filter=unread` and `/inbox?filter=all` are
            // one route. Interning the query too would mint a fresh route
            // name per filter value and desync active-route bookkeeping.
            let (url_path, url_query) = split_query(&url);
            let name: &'static str = Box::leak(url_path.to_string().into_boxed_str());
            *state.pending_mount.borrow_mut() = Some(MountResult {
                node: screen_node,
                scope_id: scope.0,
                // Screen options don't cross the wire for native nav (the
                // initial-mount path passes unit options too); the handler
                // falls back to defaults. Wiring per-screen options through
                // is a separate gap, tracked with the navigator-over-wire
                // work.
                options: Box::new(()),
            });
            let cmd = match op {
                NavOp::Push => NavCommand::Push {
                    name,
                    url: url.clone(),
                    params: Box::new(()),
                        query: url_query.clone(),
                },
                NavOp::Replace => NavCommand::Replace {
                    name,
                    url: url.clone(),
                    params: Box::new(()),
                        query: url_query.clone(),
                },
                NavOp::Reset => NavCommand::Reset {
                    name,
                    url: url.clone(),
                    params: Box::new(()),
                        query: url_query.clone(),
                },
                NavOp::Select => NavCommand::Select {
                    name,
                    url: url.clone(),
                    params: Box::new(()),
                        query: url_query.clone(),
                },
            };
            state.control.dispatch(cmd);
            // Keep screen_stack / mounted_urls / replay_pos coherent so
            // NavigatorPop and Push-dedup behave across reconnects.
            match op {
                NavOp::Push => {
                    state.screen_stack.borrow_mut().push(screen);
                    state.mounted_urls.borrow_mut().push(url);
                    *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
                }
                NavOp::Replace => {
                    let mut st = state.screen_stack.borrow_mut();
                    st.pop();
                    st.push(screen);
                }
                NavOp::Reset => {
                    let mut st = state.screen_stack.borrow_mut();
                    st.clear();
                    st.push(screen);
                    drop(st);
                    state.mounted_urls.borrow_mut().push(url);
                    *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
                }
                NavOp::Select => {
                    let mut st = state.screen_stack.borrow_mut();
                    st.clear();
                    st.push(screen);
                }
            }
            return Ok(());
        }

        if matches!(op, NavOp::Push) {
            let urls = state.mounted_urls.borrow();
            let pos = *state.replay_pos.borrow();
            if pos < urls.len() && urls[pos] == url {
                drop(urls);
                *state.replay_pos.borrow_mut() = pos + 1;
                return Ok(());
            }
        }
        let _ = (scope, &options);
        let screen_node = self.lookup_node(screen)?;

        // Every push-like op makes `screen` the single visible child of
        // the outlet (the client renders the top-of-stack screen). The
        // difference is what each does to the tracked screen stack, which
        // is what lets `NavigatorPop` re-show the prior screen.
        {
            let mut outlet = state.outlet.clone();
            let mut backend = self.backend.borrow_mut();
            backend.clear_children(&outlet);
            backend.insert(&mut outlet, screen_node);
        }
        match op {
            NavOp::Push => {
                state.screen_stack.borrow_mut().push(screen);
                state.mounted_urls.borrow_mut().push(url);
                *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
            }
            NavOp::Replace => {
                // Swap the top frame.
                let mut st = state.screen_stack.borrow_mut();
                st.pop();
                st.push(screen);
            }
            NavOp::Reset => {
                let mut st = state.screen_stack.borrow_mut();
                st.clear();
                st.push(screen);
                state.mounted_urls.borrow_mut().push(url);
                *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
            }
            NavOp::Select => {
                // Single-slot swap: the stack is always one
                // entry (the selected screen).
                let mut st = state.screen_stack.borrow_mut();
                st.clear();
                st.push(screen);
            }
        }
        Ok(())
    }
}

/// Internal: which dispatcher-driven navigation op a push-like wire
/// command should produce. All four share the same staging dance
/// (set pending_mount, dispatch, clear).
#[derive(Copy, Clone)]
enum NavOp {
    Push,
    Replace,
    Reset,
    Select,
}


/// Convert a [`runtime_shared::ColorScheme`] into the wire form.
pub fn color_scheme_to_wire(scheme: ColorScheme) -> WireColorScheme {
    match scheme {
        ColorScheme::Light => WireColorScheme::Light,
        ColorScheme::Dark => WireColorScheme::Dark,
        ColorScheme::Auto => WireColorScheme::Auto,
    }
}
