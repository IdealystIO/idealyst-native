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


use framework_core::{Backend, ColorScheme, StateBits, StyleRules};
use wire::{
    AppToDev, Command, EventArgs, HandlerId, NodeId, ScopeId, StyleId, WireColorScheme,
    WireDrawerSide, WireDrawerType, WireItemSize, WireMountPolicy, WireTabPlacement,
    WireTabRegistration,
};

pub mod convert;
pub mod graphics;
pub mod navigators;

/// The AAS (Application-as-a-Server) **client-side replayer** —
/// wraps any `framework_core::Backend` and feeds it the wire
/// [`wire::Command`]s shipped by an
/// [`AasBackend`](dev_server::AasBackend). Idempotent
/// apply means re-sending a snapshot only does DOM work for the
/// commands that actually changed something.
///
/// ```text
/// UI tree → AasBackend → Wire → AasClient<PlatformBackend> → Native
/// ```
///
/// The same `AasClient` plugs into `WebBackend` on the browser,
/// `IosBackend` on iOS, `AndroidBackend` on Android — every
/// platform target the framework supports.
pub use crate::WireBackend as AasClient;

// Transport, discovery, and the worker-thread `AasShell` for native
// targets live in `aas-shell-native` (under its `aas-shell` feature).
// Hosts on iOS / Android / desktop import them from there. The web
// transport (`web_sys::WebSocket` + rAF outbound pump) lives in
// `backend-web`'s `dev_transport` module under its `aas-shell`
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
    backend: B,
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

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn color_scheme(&self) -> ColorScheme {
        self.backend.color_scheme()
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
            Command::CreateView { id } => {
                if !self.nodes.contains_key(&id) {
                    let node = self.backend.create_view();
                    self.nodes.insert(id, node);
                }
            }
            Command::CreateText { id, content } => {
                if let Some(existing) = self.nodes.get(&id).cloned() {
                    // Same node id, same content → no-op. Same id,
                    // different content → update_text.
                    let prev = self.text_content.get(&id);
                    if prev.map(|s| s.as_str()) != Some(content.as_str()) {
                        self.backend.update_text(&existing, &content);
                        self.text_content.insert(id, content);
                    }
                } else {
                    let node = self.backend.create_text(&content);
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
                        self.backend.update_button_label(&existing, &label);
                        self.button_labels.insert(id, label);
                    }
                    // Drop the synthesized handler — the existing
                    // one stays attached.
                    let _ = (on_click, leading_icon, trailing_icon);
                    return Ok(());
                }
                let cb = self.handler_unit(on_click);
                let leading = leading_icon.map(convert::wire_icon_to_static);
                let trailing = trailing_icon.map(convert::wire_icon_to_static);
                // Wire side has no structured action metadata; wrap
                // the closure as an opaque Action and let the
                // backend's runtime path use `.fire`.
                let action = framework_core::IntoAction::into_action(move || cb());
                let node = self.backend.create_button(
                    &label,
                    &action,
                    leading.as_ref(),
                    trailing.as_ref(),
                );
                self.nodes.insert(id, node);
            }
            Command::CreatePressable { id, on_click } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_unit(on_click);
                let node = self.backend.create_pressable(cb);
                self.nodes.insert(id, node);
            }
            Command::CreateReactiveAnchor { id } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self.backend.create_reactive_anchor();
                self.nodes.insert(id, node);
            }
            Command::CreateImage { id, src, alt } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self.backend.create_image(&src, alt.as_deref());
                self.nodes.insert(id, node);
            }
            Command::CreateIcon { id, data, color } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let icon = convert::wire_icon_to_static(data);
                let color = color.map(convert::wire_color_to_color);
                let node = self.backend.create_icon(&icon, color.as_ref());
                self.nodes.insert(id, node);
            }
            Command::CreateTextInput {
                id,
                initial_value,
                placeholder,
                on_change,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_string(on_change);
                let node = self
                    .backend
                    .create_text_input(&initial_value, placeholder.as_deref(), cb);
                self.nodes.insert(id, node);
            }
            Command::CreateToggle {
                id,
                initial_value,
                on_change,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_bool(on_change);
                let node = self.backend.create_toggle(initial_value, cb);
                self.nodes.insert(id, node);
            }
            Command::CreateSlider {
                id,
                initial_value,
                min,
                max,
                step,
                on_change,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_float(on_change);
                let node = self
                    .backend
                    .create_slider(initial_value, min, max, step, cb);
                self.nodes.insert(id, node);
            }
            Command::CreateScrollView { id, horizontal } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self.backend.create_scroll_view(horizontal);
                self.nodes.insert(id, node);
            }
            Command::CreateWebView { id, url } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self.backend.create_web_view(&url);
                self.nodes.insert(id, node);
            }
            Command::CreateVideo {
                id,
                src,
                autoplay,
                controls,
                loop_playback,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self
                    .backend
                    .create_video(&src, autoplay, controls, loop_playback);
                self.nodes.insert(id, node);
            }
            Command::CreateActivityIndicator { id, size, color } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let size = convert::wire_activity_size(size);
                let color = color.map(convert::wire_color_to_color);
                let node = self.backend.create_activity_indicator(size, color.as_ref());
                self.nodes.insert(id, node);
            }
            Command::CreateLink {
                id,
                route,
                url,
                kind: _,
                on_activate,
            } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                let cb = self.handler_unit(on_activate);
                let route_static: &'static str = Box::leak(route.into_boxed_str());
                let config = framework_core::primitives::link::LinkConfig {
                    route: route_static,
                    url,
                    on_activate: cb,
                };
                let node = self.backend.create_link(config);
                self.nodes.insert(id, node);
            }
            Command::CreateOverlay {
                id,
                anchor,
                backdrop,
                on_dismiss,
                trap_focus,
            } => {
                use framework_core::primitives::overlay::{BackdropMode, ViewportPlacement};
                // The framework recently split overlays into
                // viewport-anchored (`Primitive::Overlay`) and
                // element-anchored (`Primitive::AnchoredOverlay`).
                // The wire still uses one `CreateOverlay` command
                // with a `WireOverlayAnchor` discriminator. For the
                // viewport case we drive `create_overlay`. For the
                // element case we fall back to a centered viewport
                // overlay (proper element-anchoring on the wire
                // would need a `CreateAnchoredOverlay` command with
                // an `AnchorTarget`-equivalent referencing a wire
                // NodeId — TODO).
                let placement = match anchor {
                    wire::WireOverlayAnchor::Viewport(p) => match p {
                        wire::WireViewportPlacement::Center => {
                            ViewportPlacement::Center
                        }
                        wire::WireViewportPlacement::Top => ViewportPlacement::Top,
                        wire::WireViewportPlacement::Bottom => {
                            ViewportPlacement::Bottom
                        }
                        wire::WireViewportPlacement::Left => ViewportPlacement::Left,
                        wire::WireViewportPlacement::Right => ViewportPlacement::Right,
                        // Corner placements + FullScreen don't have
                        // first-class variants today; fall back to
                        // the nearest edge / centered. Acceptable
                        // for the prototype — corner overlays are
                        // rare.
                        wire::WireViewportPlacement::TopLeft
                        | wire::WireViewportPlacement::TopRight
                        | wire::WireViewportPlacement::BottomLeft
                        | wire::WireViewportPlacement::BottomRight => {
                            ViewportPlacement::Center
                        }
                    },
                    wire::WireOverlayAnchor::Element { .. } => {
                        // Without a proper wire-side `AnchorTarget`,
                        // collapse to a centered viewport overlay so
                        // the overlay still mounts visibly.
                        ViewportPlacement::Center
                    }
                };
                let resolved_backdrop = match backdrop {
                    wire::WireBackdropMode::None => BackdropMode::None,
                    wire::WireBackdropMode::Dismiss => BackdropMode::Dismiss,
                    wire::WireBackdropMode::Capture => BackdropMode::Opaque,
                };
                let dismiss_cb: Option<Rc<dyn Fn()>> =
                    on_dismiss.map(|h| self.handler_unit(h));
                if self.nodes.contains_key(&id) { return Ok(()); }
                let node = self.backend.create_overlay(
                    placement,
                    resolved_backdrop,
                    dismiss_cb,
                    trap_focus,
                );
                self.nodes.insert(id, node);
            }
            Command::CreateGraphics { id, renderer } => {
                if self.nodes.contains_key(&id) { return Ok(()); }
                // Look up the renderer in the app-local registry. If
                // absent, the Graphics surface is still created (so the
                // tree layout stays correct) but no GPU code runs.
                let lookup = self.graphics_registry.lookup(&renderer);
                let (on_ready, on_resize, on_lost) = match lookup {
                    Some(triple) => triple,
                    None => no_op_graphics_handlers(),
                };
                let node = self.backend.create_graphics(on_ready, on_resize, on_lost);
                self.nodes.insert(id, node);
            }
            Command::CreateVirtualizer {
                id,
                overscan,
                horizontal,
                initial_size,
                initial_keys,
            } => {
                self.apply_create_virtualizer(id, overscan, horizontal, initial_size, initial_keys);
            }
            Command::CreateNavigator { id, initial_route, initial_path } => {
                self.apply_create_navigator(id, initial_route, initial_path);
            }
            Command::CreateTabNavigator {
                id,
                initial_route,
                initial_path,
                tabs,
                placement,
                mount_policy,
            } => {
                self.apply_create_tab_navigator(
                    id, initial_route, initial_path, tabs, placement, mount_policy,
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
            } => {
                self.apply_create_drawer_navigator(
                    id, initial_route, initial_path, side, drawer_type,
                    drawer_width, swipe_to_open, mount_policy,
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
                self.backend.insert(parent_node, child_node);
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
                self.backend.insert_many(parent_node, children_nodes);
                for c in children_to_insert {
                    self.inserted_edges.insert((parent, c));
                }
            }
            Command::ClearChildren { node } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.clear_children(&n);
                // Forget every edge whose parent was just cleared.
                self.inserted_edges.retain(|(p, _)| *p != node);
            }

            // --- Reactive updates ---
            Command::UpdateText { node, content } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_text(&n, &content);
            }
            Command::UpdateButtonLabel { node, label } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_button_label(&n, &label);
            }
            Command::UpdateImageSrc { node, src } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_image_src(&n, &src);
            }
            Command::UpdateIconColor { node, color } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let c = convert::wire_color_to_color(color);
                self.backend.update_icon_color(&n, &c);
            }
            Command::UpdateIconStroke { node, progress } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_icon_stroke(&n, progress);
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
                self.backend
                    .animate_icon_stroke(&n, from, to, duration_ms, e, infinite, autoreverses);
            }
            Command::UpdateTextInputValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_text_input_value(&n, &value);
            }
            Command::UpdateToggleValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_toggle_value(&n, value);
            }
            Command::UpdateSliderValue { node, value } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_slider_value(&n, value);
            }
            Command::UpdateWebViewUrl { node, url } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_web_view_url(&n, &url);
            }
            Command::UpdateVideoSrc { node, src } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.update_video_src(&n, &src);
            }
            Command::SetDisabled { node, disabled } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                self.backend.set_disabled(&n, disabled);
            }

            // --- Styles ---
            Command::RegisterStyle { id, rules } => {
                let resolved: Rc<StyleRules> = Rc::new(convert::wire_style_to_rules(rules));
                // Notify the backend so it can mint platform-side state
                // (web class caching, etc.). Wrapping in a slice mirrors
                // the Backend signature.
                self.backend.register_stylesheet(std::slice::from_ref(&resolved));
                self.styles.insert(id, resolved);
            }
            Command::UnregisterStyle { id } => {
                if let Some(rules) = self.styles.remove(&id) {
                    self.backend.unregister_stylesheet(std::slice::from_ref(&rules));
                }
            }
            Command::ApplyStyle { node, style } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let s = self.styles.get(&style).ok_or(ReplayError::UnknownStyle(style))?.clone();
                self.backend.apply_style(&n, &s);
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
                self.backend.apply_styled_states(&n, &b, &o);
            }
            Command::AttachStates { node } => {
                let n = self.nodes.get(&node).ok_or(ReplayError::UnknownNode(node))?.clone();
                let outbound = self.outbound.clone();
                let node_id = node;
                self.backend.attach_states(
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
                self.backend.on_node_unstyled(&n);
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
                self.backend.apply_presence(&n, s, t);
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
                let nav = self.lookup_node(navigator)?;
                let screen_node = self.lookup_node(screen)?;
                let opts = convert::wire_screen_options(&options, |id| self.handler_unit(id));
                match state.kind {
                    navigators::NavigatorKind::Stack => {
                        self.backend
                            .navigator_attach_initial(&nav, screen_node, scope.0, opts);
                    }
                    navigators::NavigatorKind::Tab => {
                        self.backend
                            .tab_navigator_attach_initial(&nav, screen_node, scope.0, opts);
                    }
                    navigators::NavigatorKind::Drawer => {
                        self.backend
                            .drawer_navigator_attach_initial(&nav, screen_node, scope.0, opts);
                    }
                }
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
            Command::NavigatorPop { navigator, count } => {
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                // Pop idempotency. The server emits a
                // `Command::NavigatorPop` for client-initiated pops
                // (swipe-back → `handle_screen_released`), and the
                // broadcast goes to every connected client —
                // including the one that originated the pop. That
                // client's native nav controller has already popped
                // locally; re-dispatching here would over-pop. We
                // gate on `mounted_urls.len() > 1` (initial screen
                // can't be popped) and the cursor-already-at-end so
                // we don't double-pop during replay either.
                let mut state_pops = 0;
                {
                    let urls_len = state.mounted_urls.borrow().len();
                    let pos = *state.replay_pos.borrow();
                    // Only pop what we still have above the initial
                    // screen, and never pop during replay (when
                    // `pos < urls_len`).
                    if pos == urls_len {
                        state_pops = count.min((urls_len as u32).saturating_sub(1));
                    }
                }
                *state.suppress_release.borrow_mut() = true;
                for _ in 0..state_pops {
                    state
                        .control
                        .dispatch(framework_core::primitives::navigator::NavCommand::Pop);
                }
                *state.suppress_release.borrow_mut() = false;
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

            // --- Drawer control plane ---
            Command::DrawerAttachSidebar { navigator, sidebar } => {
                // Dedup: sidecar respawns re-emit this command, and
                // Identity dedup gives the same wire ids — calling
                // backend.drawer_navigator_attach_sidebar twice
                // crashes DrawerLayout (two children with the same
                // edge gravity).
                if self
                    .drawer_sidebars_attached
                    .insert((navigator, sidebar))
                {
                    let nav = self.lookup_node(navigator)?;
                    let sb = self.lookup_node(sidebar)?;
                    self.backend.drawer_navigator_attach_sidebar(&nav, sb);
                }
            }
            Command::OpenDrawer { navigator } => {
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                state
                    .control
                    .dispatch(framework_core::primitives::navigator::NavCommand::OpenDrawer);
            }
            Command::CloseDrawer { navigator } => {
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                state
                    .control
                    .dispatch(framework_core::primitives::navigator::NavCommand::CloseDrawer);
            }
            Command::ToggleDrawer { navigator } => {
                let state = self
                    .navigators
                    .get(&navigator)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(navigator))?;
                state
                    .control
                    .dispatch(framework_core::primitives::navigator::NavCommand::ToggleDrawer);
            }

            // --- Navigator chrome styles ---
            Command::ApplyNavigatorHeaderStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_navigator_header_style(&n, &s);
            }
            Command::ApplyNavigatorTitleStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_navigator_title_style(&n, &s);
            }
            Command::ApplyNavigatorButtonStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_navigator_button_style(&n, &s);
            }
            Command::ApplyNavigatorBodyStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_navigator_body_style(&n, &s);
            }
            Command::ApplyDrawerSidebarStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_drawer_sidebar_style(&n, &s);
            }
            Command::ApplyDrawerScrimStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_drawer_scrim_style(&n, &s);
            }
            Command::ApplyTabBarStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_tab_bar_style(&n, &s);
            }
            Command::ApplyTabIconStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_tab_icon_style(&n, &s);
            }
            Command::ApplyTabLabelStyle { navigator, style } => {
                let n = self.lookup_node(navigator)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_tab_label_style(&n, &s);
            }

            // --- Virtualizer control plane ---
            Command::VirtualizerDataChanged { node, item_count: _ } => {
                let n = self.lookup_node(node)?;
                self.backend.virtualizer_data_changed(&n);
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

            // --- Overlay backdrop style ---
            Command::ApplyOverlayBackdropStyle { node, style } => {
                let n = self.lookup_node(node)?;
                let s = self.lookup_style(style)?;
                self.backend.apply_overlay_backdrop_style(&n, &s);
            }

            Command::Finish { root } => {
                let n = self
                    .nodes
                    .get(&root)
                    .cloned()
                    .ok_or(ReplayError::UnknownNode(root))?;
                self.backend.finish(n);
            }
            Command::ReleaseNode { node } => {
                self.nodes.remove(&node);
            }
            Command::InstallThemeVariables { .. } => {
                // Backends that care (web) implement this via
                // install_theme_variables; for the prototype the
                // mapping requires a TokenEntry conversion we haven't
                // implemented yet. Skip cleanly.
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

    fn apply_create_navigator(&mut self, id: NodeId, initial_route: String, initial_path: String) {
        // Idempotency. If a navigator with this id is already
        // mounted (we kept state across a reconnect, or the wire
        // is re-applying its snapshot), don't build a second
        // `UINavigationController` / DOM nav — the old one is
        // still in the view hierarchy, and the new one would be
        // orphaned because `Insert` is also idempotent. Pop/Push
        // commands later in the stream are dispatched against
        // whichever control is in `self.navigators[id]`, so
        // overwriting it with a fresh-but-unmounted nav would make
        // the visible UI un-pop-able.
        //
        // We also use this branch to reset `replay_pos`: a
        // duplicate `CreateNavigator` only arrives when the
        // append-only log is being replayed (server restart or new
        // session). The subsequent `AttachInitial`/`Push` stream
        // should be checked against `mounted_urls` from index 0.
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        let route_static: &'static str = Box::leak(initial_route.clone().into_boxed_str());
        let path_static: &'static str = Box::leak(initial_path.clone().into_boxed_str());

        let control = Rc::new(framework_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));

        // Pre-allocate state with a placeholder node; we'll set the
        // real node after `create_navigator` returns.
        let state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Stack,
            // We can't yet fill `node`; placeholder via uninit is
            // unsafe. Instead we defer state construction until we
            // have the node.
            node: self.backend.create_view(), // temporary placeholder
            control: control.clone(),
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path: initial_path.clone(),
            mounted_urls: mounted_urls.clone(),
            replay_pos: replay_pos.clone(),
        });

        // Build callbacks referencing the state Rc.
        let callbacks = state.build_stub_callbacks(route_static, path_static);

        // Real backend create call. This installs the real backend's
        // dispatcher onto control.
        let nav_node = self.backend.create_navigator(callbacks, control.clone());

        // Reconstruct the state with the real node in place.
        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Stack,
            node: nav_node.clone(),
            control,
            pending_mount: state.pending_mount.clone(),
            suppress_release: state.suppress_release.clone(),
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
    ) {
        // Idempotent — see comment in `apply_create_navigator`.
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        let route_static: &'static str = Box::leak(initial_route.clone().into_boxed_str());
        let path_static: &'static str = Box::leak(initial_path.clone().into_boxed_str());

        let resolved_tabs = tabs
            .into_iter()
            .map(|t| framework_core::primitives::navigator::TabRegistration {
                route: Box::leak(t.route.into_boxed_str()),
                label: t.label,
                icon: t.icon,
                badge: None,
            })
            .collect();
        let resolved_placement = match placement {
            WireTabPlacement::Bottom => {
                framework_core::primitives::navigator::TabPlacement::Bottom
            }
            WireTabPlacement::Top => framework_core::primitives::navigator::TabPlacement::Top,
        };
        let resolved_mount_policy = convert::wire_mount_policy(mount_policy);

        let control = Rc::new(framework_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));
        let state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Tab,
            node: self.backend.create_view(),
            control: control.clone(),
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path: initial_path.clone(),
            mounted_urls: mounted_urls.clone(),
            replay_pos: replay_pos.clone(),
        });

        let callbacks = state.build_stub_tab_callbacks(
            route_static,
            path_static,
            resolved_tabs,
            resolved_placement,
            resolved_mount_policy,
        );
        let nav_node = self.backend.create_tab_navigator(callbacks, control.clone());

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Tab,
            node: nav_node.clone(),
            control,
            pending_mount: state.pending_mount.clone(),
            suppress_release: state.suppress_release.clone(),
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
    ) {
        // Idempotent — see comment in `apply_create_navigator`.
        if let Some(state) = self.navigators.get(&id) {
            *state.replay_pos.borrow_mut() = 0;
            return;
        }

        let route_static: &'static str = Box::leak(initial_route.clone().into_boxed_str());
        let path_static: &'static str = Box::leak(initial_path.clone().into_boxed_str());

        let resolved_side = match side {
            WireDrawerSide::Left => framework_core::primitives::navigator::DrawerSide::Start,
            WireDrawerSide::Right => framework_core::primitives::navigator::DrawerSide::End,
        };
        let resolved_drawer_type = match drawer_type {
            WireDrawerType::Front => framework_core::primitives::navigator::DrawerType::Front,
            // Wire's `Back` (sidebar fixed behind body) doesn't have a
            // first-class framework variant; closest is `Slide` which
            // also moves the body. Acceptable compromise; revisit if
            // the framework adds Back later.
            WireDrawerType::Back => framework_core::primitives::navigator::DrawerType::Slide,
            WireDrawerType::Slide => framework_core::primitives::navigator::DrawerType::Slide,
        };
        let resolved_mount_policy = convert::wire_mount_policy(mount_policy);

        let control = Rc::new(framework_core::primitives::navigator::NavigatorControl::new());
        let mounted_urls = Rc::new(RefCell::new(Vec::new()));
        let replay_pos = Rc::new(RefCell::new(0usize));
        let state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Drawer,
            node: self.backend.create_view(),
            control: control.clone(),
            pending_mount: Rc::new(RefCell::new(None)),
            suppress_release: Rc::new(RefCell::new(false)),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path: initial_path.clone(),
            mounted_urls: mounted_urls.clone(),
            replay_pos: replay_pos.clone(),
        });

        let callbacks = state.build_stub_drawer_callbacks(
            route_static,
            path_static,
            resolved_side,
            resolved_drawer_type,
            drawer_width,
            swipe_to_open,
            resolved_mount_policy,
        );
        let nav_node = self.backend.create_drawer_navigator(callbacks, control.clone());

        let final_state = Rc::new(navigators::NavigatorAppState {
            kind: navigators::NavigatorKind::Drawer,
            node: nav_node.clone(),
            control,
            pending_mount: state.pending_mount.clone(),
            suppress_release: state.suppress_release.clone(),
            outbound: self.outbound.clone(),
            navigator_id: id,
            initial_path,
            mounted_urls,
            replay_pos,
        });

        self.nodes.insert(id, nav_node);
        self.navigators.insert(id, final_state);
    }

    fn apply_create_virtualizer(
        &mut self,
        _id: NodeId,
        _overscan: f32,
        _horizontal: bool,
        _initial_size: WireItemSize,
        _initial_keys: Vec<u64>,
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
        // `_restore` is reserved for platform-specific history glue
        // (e.g. web's `pushState` should be skipped when the server
        // is replaying state we already have). The current generic
        // dispatch path is platform-agnostic; backends that care
        // about URL/history can subscribe to the wire command
        // directly.
        use framework_core::primitives::navigator::NavCommand;

        let state = self
            .navigators
            .get(&navigator)
            .cloned()
            .ok_or(ReplayError::UnknownNode(navigator))?;
        // URL-based replay dedup for Push (the only op
        // `restore_nav_state` re-emits across a rebuild-exec).
        // Replace/Reset always apply: a server-emitted Replace
        // overwrites the top, and Reset rebuilds from scratch —
        // neither comes out of replay. Select (tab activation)
        // legitimately re-targets the same scope on each tap, so
        // it also bypasses dedup.
        if matches!(op, NavOp::Push) {
            let urls = state.mounted_urls.borrow();
            let pos = *state.replay_pos.borrow();
            eprintln!(
                "[aas-client] Push url={:?}: mounted_urls={:?} pos={}",
                url, *urls, pos
            );
            if pos < urls.len() && urls[pos] == url {
                drop(urls);
                *state.replay_pos.borrow_mut() = pos + 1;
                eprintln!("  -> dedup match, skipping");
                return Ok(());
            }
            eprintln!("  -> applying push via control.dispatch");
        }
        let screen_node = self.lookup_node(screen)?;
        let opts = convert::wire_screen_options(&options, |id| self.handler_unit(id));

        let mount_result = framework_core::primitives::navigator::MountResult {
            node: screen_node,
            scope_id: scope.0,
            options: opts,
        };

        *state.pending_mount.borrow_mut() = Some(mount_result);
        *state.suppress_release.borrow_mut() = true;

        let cmd = match op {
            NavOp::Push => NavCommand::Push {
                name: "",
                url: String::new(),
                params: Box::new(()),
            },
            NavOp::Replace => NavCommand::Replace {
                name: "",
                url: String::new(),
                params: Box::new(()),
            },
            NavOp::Reset => NavCommand::Reset {
                name: "",
                url: String::new(),
                params: Box::new(()),
            },
            NavOp::Select => NavCommand::Select {
                name: "",
                url: String::new(),
                params: Box::new(()),
            },
        };
        state.control.dispatch(cmd);

        *state.suppress_release.borrow_mut() = false;
        // Defensive: clear any unconsumed mount (the dispatcher
        // should have taken it; if it didn't, we don't want to leak
        // a stale value into the next push).
        let _ = state.pending_mount.borrow_mut().take();

        // Bookkeep `mounted_urls` for future replay dedup. The
        // backend's release_screen closure (in navigators.rs)
        // pops `mounted_urls` whenever a screen leaves the stack,
        // so for Replace/Reset the previous top(s) are already
        // off; we just push the new url. Push adds without
        // releasing. Select doesn't change the stack at the
        // navigator level, so leave the bookkeeping alone.
        match op {
            NavOp::Push | NavOp::Replace | NavOp::Reset => {
                state.mounted_urls.borrow_mut().push(url);
                *state.replay_pos.borrow_mut() = state.mounted_urls.borrow().len();
            }
            NavOp::Select => {}
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


/// Convert a [`framework_core::ColorScheme`] into the wire form.
pub fn color_scheme_to_wire(scheme: ColorScheme) -> WireColorScheme {
    match scheme {
        ColorScheme::Light => WireColorScheme::Light,
        ColorScheme::Dark => WireColorScheme::Dark,
        ColorScheme::Auto => WireColorScheme::Auto,
    }
}
