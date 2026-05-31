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


use runtime_core::{AlignItems, Backend, ColorScheme, FlexDirection, Length, StateBits, StyleRules};
use wire::{
    AppToDev, Command, EventArgs, HandlerId, NodeId, ScopeId, StyleId, WireColorScheme,
    WireDrawerSide, WireDrawerType, WireItemSize, WireMountPolicy, WireTabPlacement,
    WireTabRegistration,
};

pub mod convert;
pub mod graphics;
pub mod navigators;

/// The runtime-server (Application-as-a-Server) **client-side replayer** —
/// wraps any `runtime_core::Backend` and feeds it the wire
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
pub struct WireBackend<B: Backend>
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
    /// Per-navigator state. Populated on `CreateNavigator` /
    /// `CreateTabNavigator` / `CreateDrawerNavigator` and consulted
    /// by the navigator control-plane commands.
    navigators: HashMap<NodeId, Rc<navigators::NavigatorAppState<B::Node>>>,
    /// Edges already realized in the backend's tree. Used by
    /// idempotent `Insert` so a re-applied command stream (after a
    /// reconnect) doesn't reorder or duplicate existing children.
    /// Set of `(parent, child)` pairs.
    inserted_edges: std::collections::HashSet<(NodeId, NodeId)>,
    /// `(navigator, sidebar)` pairs that have already been attached
    /// via [`Command::DrawerAttachSidebar`]. Re-attaching is fatal —
    /// `androidx.drawerlayout.widget.DrawerLayout.onMeasure` throws
    /// `IllegalStateException("Child drawer has absolute gravity LEFT
    /// but this DrawerLayout already has a drawer view along that
    /// edge")` if a second child with the same edge gravity is added.
    /// Sidecar respawns re-emit the initial command stream (Identity
    /// dedup makes the NodeIds match the previously-attached ones),
    /// so we dedup at the command layer.
    drawer_sidebars_attached: std::collections::HashSet<(NodeId, NodeId)>,
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
    /// N reconnects). Same shape as `drawer_sidebars_attached`.
    attached_states: std::collections::HashSet<NodeId>,
}

impl<B: Backend> WireBackend<B>
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
            drawer_sidebars_attached: std::collections::HashSet::new(),
            text_content: HashMap::new(),
            button_labels: HashMap::new(),
            attached_states: std::collections::HashSet::new(),
        }
    }

    /// Expose the outbound sender so the transport can retarget it on
    /// reconnect.
    pub fn outbound(&self) -> &OutboundSender {
        &self.outbound
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
                let action = runtime_core::IntoAction::into_action(move || cb());
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
                let node = self.backend.borrow_mut().create_reactive_anchor();
                self.nodes.insert(id, node);
            }
            Command::CreateImage { id, src, alt, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_image(&src, alt.as_deref(), &a11y);
                self.nodes.insert(id, node);
            }
            Command::CreateIcon { id, data, color, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let icon = convert::wire_icon_to_static(data);
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
                a11y,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_string(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_text_input(
                    &initial_value,
                    placeholder.as_deref(),
                    cb,
                    None,
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateTextArea {
                id,
                initial_value,
                placeholder,
                on_change,
                a11y,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_string(on_change);
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_text_area(
                    &initial_value,
                    placeholder.as_deref(),
                    cb,
                    None,
                    &a11y,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateExternal { id, type_name, a11y } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                // Payload couldn't cross the wire. Fall back to a plain
                // View placeholder so the tree stays well-formed; the
                // user gets a visible "External primitive: <name>" via
                // the surrounding app code if they want a richer
                // placeholder, this code path leaves room for client-
                // side external-registry lookup as future work.
                let _ = type_name;
                let a11y = self.a11y_props(a11y);
                let node = self.backend.borrow_mut().create_view(&a11y);
                self.nodes.insert(id, node);
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
                if self.nodes.contains_key(&id) { return Ok(()); }
                let size = convert::wire_activity_size(size);
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
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_unit(on_activate);
                let route_static: &'static str = Box::leak(route.into_boxed_str());
                let config = runtime_core::primitives::link::LinkConfig {
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
                use runtime_core::primitives::portal::{
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
                horizontal,
                initial_size,
                initial_keys,
                a11y,
            } => {
                self.apply_create_virtualizer(
                    id, overscan, horizontal, initial_size, initial_keys, a11y,
                );
            }
            Command::CreateNavigator { id, initial_route, initial_path, a11y } => {
                self.apply_create_navigator(id, initial_route, initial_path, a11y);
            }
            Command::CreateTabNavigator {
                id,
                initial_route,
                initial_path,
                tabs,
                placement,
                mount_policy,
                a11y,
            } => {
                self.apply_create_tab_navigator(
                    id, initial_route, initial_path, tabs, placement, mount_policy, a11y,
                );
            }
            Command::CreateDrawerNavigator {
                id,
                initial_route,
                initial_path,
                side,
                drawer_type,
                drawer_width,
                swipe_to_open,
                mount_policy,
                a11y,
            } => {
                self.apply_create_drawer_navigator(
                    id, initial_route, initial_path, side, drawer_type,
                    drawer_width, swipe_to_open, mount_policy, a11y,
                );
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
            Command::UpdateIconColor { node, color } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let c = convert::wire_color_to_color(color);
                self.backend.borrow_mut().update_icon_color(&n, &c);
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
            Command::SetDisabled { node, disabled } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.borrow_mut().set_disabled(&n, disabled);
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
                let s = self.styles.get(&style).ok_or(ReplayError::UnknownStyle(style))?.clone();
                self.backend.borrow_mut().apply_style(&n, &s);
            }
            Command::ApplyStyledStates { node, base, overlays } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let b = self
                    .styles
                    .get(&base)
                    .ok_or(ReplayError::UnknownStyle(base))?
                    .clone();
                let mut o: Vec<(StateBits, Rc<StyleRules>)> = Vec::with_capacity(overlays.len());
                for (bit, sid) in overlays {
                    let bits = convert::wire_state_bit(bit);
                    let rules = self.styles.get(&sid).ok_or(ReplayError::UnknownStyle(sid))?.clone();
                    o.push((bits, rules));
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
                // `inserted_edges` / `drawer_sidebars_attached`.
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
                // tab/drawer navigator. Pre-fix this was conflated with
                // `Reset`, which drained the snapshot model's per-screen
                // state for tab navigators (a Reset means "discard
                // stack and mount new root"; a Select means "switch
                // active tab"). The dev-server now emits
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
            Command::NavigatorMountTab {
                navigator,
                index: _,
                screen,
                scope,
            } => {
                // Tab activation that needed a new mount. Same path
                // as Push from the backend's perspective: stage the
                // pre-built screen and trigger a Select dispatch.
                self.dispatch_push_like(
                    navigator,
                    screen,
                    scope,
                    wire::WireScreenOptions::default(),
                    NavOp::Select,
                    String::new(),
                    false,
                )?;
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

            // --- Drawer control plane ---
            Command::DrawerAttachSidebar { navigator, sidebar } => {
                // Insert the (floating) sidebar subtree into the
                // navigator's persistent sidebar column. Dedup via
                // `inserted_edges`: sidecar respawns re-emit this with
                // the same Identity-deduped ids, and re-inserting would
                // re-order the sidebar into the slot twice.
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                if let Some(slot) = state.sidebar_slot.clone() {
                    if !self.inserted_edges.contains(&(navigator, sidebar)) {
                        let sidebar_node = self.lookup_node(sidebar)?;
                        let mut slot = slot;
                        self.backend.borrow_mut().insert(&mut slot, sidebar_node);
                        self.inserted_edges.insert((navigator, sidebar));
                    }
                }
            }
            Command::OpenDrawer { navigator } => {
                // Drawer open/close/toggle were enum variants on the
                // pre-refactor `NavCommand`; the new model moves these
                // to SDK-specific Custom payloads. Stubbed pending the
                // wire-protocol redesign.
                let _ = navigator;
            }
            Command::CloseDrawer { navigator } => {
                let _ = navigator;
            }
            Command::ToggleDrawer { navigator } => {
                let _ = navigator;
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
            | Command::ApplyNavigatorBodyStyle { .. }
            | Command::ApplyDrawerSidebarStyle { .. }
            | Command::ApplyDrawerScrimStyle { .. }
            | Command::ApplyTabBarStyle { .. }
            | Command::ApplyTabIconStyle { .. }
            | Command::ApplyTabLabelStyle { .. } => {
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
                // `inserted_edges` / `navigators` /
                // `drawer_sidebars_attached` to accumulate forever.
                self.nodes.remove(&node);
                self.text_content.remove(&node);
                self.button_labels.remove(&node);
                self.navigators.remove(&node);
                self.attached_states.remove(&node);
                self.inserted_edges
                    .retain(|(parent, child)| *parent != node && *child != node);
                self.drawer_sidebars_attached
                    .retain(|(nav, sidebar)| *nav != node && *sidebar != node);
            }
            Command::InstallThemeVariables { .. } => {
                // Backends that care (web) implement this via
                // install_theme_variables; for the prototype the
                // mapping requires a TokenEntry conversion we haven't
                // implemented yet. Skip cleanly.
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
    ) -> runtime_core::accessibility::AccessibilityProps {
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

        let control = Rc::new(runtime_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Stack,
            node: nav_node.clone(),
            // Stack reconstruction is Phase 7; mount screens straight
            // into the nav node for now so the active screen renders.
            outlet: nav_node.clone(),
            sidebar_slot: None,
            screen_stack: Rc::new(RefCell::new(Vec::new())),
            control,
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path,
            mounted_urls,
            replay_pos,
        });

        self.nodes.insert(id, nav_node);
        self.navigators.insert(id, final_state);
    }

    fn apply_create_tab_navigator(
        &mut self,
        id: NodeId,
        initial_route: String,
        initial_path: String,
        tabs: Vec<WireTabRegistration>,
        placement: WireTabPlacement,
        mount_policy: WireMountPolicy,
        a11y: wire::WireAccessibilityProps,
    ) {
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        // Stubbed pending wire-protocol redesign.
        let _ = (initial_route, tabs, placement, mount_policy);
        let a11y_props = self.a11y_props(a11y);
        let nav_node = self.backend.borrow_mut().create_view(&a11y_props);

        let control = Rc::new(runtime_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Tab,
            node: nav_node.clone(),
            // Tab reconstruction is Phase 7; mount into the nav node.
            outlet: nav_node.clone(),
            sidebar_slot: None,
            screen_stack: Rc::new(RefCell::new(Vec::new())),
            control,
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path,
            mounted_urls,
            replay_pos,
        });

        self.nodes.insert(id, nav_node);
        self.navigators.insert(id, final_state);
    }

    fn apply_create_drawer_navigator(
        &mut self,
        id: NodeId,
        initial_route: String,
        initial_path: String,
        side: WireDrawerSide,
        drawer_type: WireDrawerType,
        drawer_width: f32,
        swipe_to_open: bool,
        mount_policy: WireMountPolicy,
        a11y: wire::WireAccessibilityProps,
    ) {
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        // `drawer_type` / `swipe_to_open` / `mount_policy` describe the
        // *modal* slide-in behavior of the real per-platform drawer. The
        // wire client reconstructs a **persistent sidebar + outlet**
        // layout instead (the macOS-navigator / wide-web shape — see
        // [[project_macos_navigator_design]]): the sidebar is always
        // visible, no scrim, no animation. That's the right rendering for
        // a thin replay/headless-screenshot client — you see the
        // navigation — and it matches rule #7 (backends converge in
        // observable behavior; the dev client is one such backend).
        let _ = (initial_route, drawer_type, swipe_to_open, mount_policy);
        let a11y_props = self.a11y_props(a11y);

        let (container, sidebar, outlet) =
            self.build_drawer_layout(drawer_width, side, &a11y_props);

        let control = Rc::new(runtime_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Drawer,
            node: container.clone(),
            outlet,
            sidebar_slot: Some(sidebar),
            screen_stack: Rc::new(RefCell::new(Vec::new())),
            control,
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path,
            mounted_urls,
            replay_pos,
        });

        self.nodes.insert(id, container);
        self.navigators.insert(id, final_state);
    }

    /// Build the persistent drawer chrome — `Row[sidebar, outlet]` (or
    /// `Row[outlet, sidebar]` for `DrawerSide::Right`) — and return
    /// `(container, sidebar, outlet)`. Layout mirrors the terminal/macOS
    /// drawer handlers: a fixed-width, non-shrinking sidebar column and a
    /// flex-grow outlet. `flex_shrink: 0` on the sidebar + `flex_basis: 0`
    /// on the outlet keep wide screen content from squashing the sidebar
    /// to nothing (the bug the terminal handler documents).
    fn build_drawer_layout(
        &mut self,
        drawer_width: f32,
        side: WireDrawerSide,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> (B::Node, B::Node, B::Node) {
        let mut backend = self.backend.borrow_mut();

        let mut container = backend.create_view(a11y);
        let mut container_style = StyleRules::default();
        container_style.flex_direction = Some(FlexDirection::Row);
        container_style.align_items = Some(AlignItems::Stretch);
        container_style.width = Some(Length::pct(100.0).into());
        container_style.height = Some(Length::pct(100.0).into());
        backend.apply_style(&container, &Rc::new(container_style));

        let sidebar = backend.create_view(a11y);
        let mut sidebar_style = StyleRules::default();
        sidebar_style.width = Some(Length::Px(drawer_width).into());
        sidebar_style.height = Some(Length::pct(100.0).into());
        sidebar_style.flex_direction = Some(FlexDirection::Column);
        sidebar_style.flex_shrink = Some(0.0f32.into());
        backend.apply_style(&sidebar, &Rc::new(sidebar_style));

        let outlet = backend.create_view(a11y);
        let mut outlet_style = StyleRules::default();
        outlet_style.flex_grow = Some(1.0f32.into());
        outlet_style.flex_basis = Some(Length::Px(0.0).into());
        outlet_style.height = Some(Length::pct(100.0).into());
        outlet_style.flex_direction = Some(FlexDirection::Column);
        backend.apply_style(&outlet, &Rc::new(outlet_style));

        match side {
            WireDrawerSide::Left => {
                backend.insert(&mut container, sidebar.clone());
                backend.insert(&mut container, outlet.clone());
            }
            WireDrawerSide::Right => {
                backend.insert(&mut container, outlet.clone());
                backend.insert(&mut container, sidebar.clone());
            }
        }

        (container, sidebar, outlet)
    }

    fn apply_create_virtualizer(
        &mut self,
        _id: NodeId,
        _overscan: f32,
        _horizontal: bool,
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
                // Drawer/tab single-slot swap: the stack is always one
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


/// Convert a [`runtime_core::ColorScheme`] into the wire form.
pub fn color_scheme_to_wire(scheme: ColorScheme) -> WireColorScheme {
    match scheme {
        ColorScheme::Light => WireColorScheme::Light,
        ColorScheme::Dark => WireColorScheme::Dark,
        ColorScheme::Auto => WireColorScheme::Auto,
    }
}
