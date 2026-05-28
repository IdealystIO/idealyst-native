//! Roku backend: a command-emitting `Backend` that drives a
//! BrightScript / SceneGraph thin client running on a Roku device.
//!
//! # Status: EXPERIMENTAL — not production-ready
//!
//! Roku has no Rust runtime, so this backend works by **streaming
//! commands from a host process to a BrightScript thin client on
//! the device**. That means every user interaction pays a network
//! round-trip — unacceptable for shipping consumer apps.
//!
//! Currently usable for:
//! - Dev-time previewing a Rust-authored UI on a real Roku.
//! - Static / kiosk-style screens where latency doesn't matter.
//! - As scaffolding for a future build-time codegen path (see the
//!   companion `backend-roku-macros` crate, which is exploring an
//!   `#[method]` attribute that transpiles a Rust subset to
//!   BrightScript so user logic can ship in the .pkg).
//!
//! Do NOT use for shipping production apps. There is no BrightScript
//! thin client written yet either — the wire is defined here, but
//! the consumer side is left to the embedder.
//!
//! # Why command-emitter
//!
//! Roku devices run BrightScript with the SceneGraph UI framework.
//! There is no Rust runtime on the device, no NDK, no JNI — Rust
//! cannot execute on Roku hardware. The only way to drive a Roku
//! UI from Rust is to send instructions over a wire transport (TCP,
//! WebSocket, or local file replay), and let a BrightScript app on
//! the device translate those instructions into SceneGraph
//! mutations.
//!
//! This backend implements `Backend::Node = NodeId` — a pure
//! identifier — and translates every `Backend` trait call into a
//! [`RokuCommand`] appended to an internal queue. The embedder
//! drains the queue, ships the JSON-serialized batch to the device,
//! and the BrightScript client applies it.
//!
//! Event flow (BrightScript → Rust) is the embedder's
//! responsibility: when the client observes an `onClick`,
//! `valueChanged`, etc., it sends back the originating `HandlerId`
//! plus any payload, and the embedder looks the id up in the
//! [`HandlerTable`] returned alongside each command and invokes
//! the held `Rc<dyn Fn(...)>`.
//!
//! # SceneGraph mapping
//!
//! See [`command::RokuCommand`] for the full mapping table. In
//! short: framework `View` → `LayoutGroup`; `Text` → `Label`;
//! `Button` → `Button`; layout flex props translate to
//! `LayoutGroup`'s `layoutDirection` + `itemSpacings`.
//!
//! # Caveats
//!
//! - **No native flex**: SceneGraph's `LayoutGroup` only supports
//!   single-axis stacking. Cross-axis alignment + flex-grow have to
//!   be approximated on the client; the wire format ships the
//!   author's intent (`flex_direction`, `justify_content`,
//!   `align_items`) and the client interprets.
//! - **No SVG**: icon path data ships as strings; the client
//!   rasterizes (or looks up in a sprite atlas) at first use.
//! - **No native navigator**: the `Backend` trait's default
//!   `unimplemented!()` panics for `create_stack_navigator`, etc. —
//!   implementing those means deciding how the BrightScript client
//!   expresses navigation stacks, which is out of scope for this
//!   initial pass. Portals are wired through `create_portal` — the
//!   device-side runtime renders them as top-of-stack Groups; see
//!   the `CreatePortal` wire op.
//!
//! # Accessibility
//!
//! Roku's SceneGraph has no public AT (assistive-technology) API.
//! The platform's accessibility story — Audio Guide, closed-caption
//! routing, etc. — is dictated by the Roku OS itself, not by the
//! app, and there is no documented hook for an app to post live-
//! region announcements, attach semantic labels/roles to a node, or
//! enumerate a parallel accessibility tree the way UIKit / Android /
//! ARIA expose.
//!
//! Consequently this backend accepts an `AccessibilityProps` on
//! every `create_*` (for trait conformance with iOS / Android / web)
//! but currently **drops it on the floor** — the `_a11y` underscore
//! prefix marks the intentional no-op. The trait's no-op defaults
//! for `update_accessibility` / `announce_for_accessibility` /
//! `dump_accessibility_tree` apply unchanged; we do not override
//! them because there is nothing meaningful to do.
//!
//! If a future Roku SDK exposes per-node semantic metadata (e.g. an
//! `accessibilityLabel` field on SceneGraph nodes, or an Audio Guide
//! announcement API), the plumbing point is here:
//!
//! 1. Rename each `_a11y` parameter in this `Backend` impl to
//!    `a11y` (the `backend-roku-a11y` audit will flag the unused
//!    `_a11y` to nudge you to this step).
//! 2. Lower the relevant `AccessibilityProps` fields (label, hint,
//!    role) onto a new wire op — likely an extension to each
//!    `Create*` command or a separate `SetAccessibility { id, ... }`
//!    op — so the BrightScript client can write them into the
//!    SceneGraph node's `text` / `altText` (or whatever Roku names
//!    its semantic field).
//! 3. Override `update_accessibility` to emit the same wire op for
//!    re-renders.
//! 4. Override `announce_for_accessibility` to emit a new
//!    `Announce { msg, priority }` wire op the client routes to
//!    Audio Guide.

#![deny(missing_debug_implementations)]

pub mod command;
mod style;

/// `#[method]` — annotate pure-logic functions for transpilation
/// into BrightScript at compile time. See
/// [`backend_roku_macros`](../backend_roku_macros/index.html) for
/// the supported Rust subset and emitted output shape.
pub use backend_roku_macros::method;

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    primitives::{
        activity_indicator::ActivityIndicatorSize, graphics::{OnLost, OnReady, OnResize},
        icon::IconData,
    },
    Backend, Color, StyleRules,
};

pub use command::{
    HandlerId, NodeId, RokuCommand, SignalId, WireColor, WireElementAlign, WireElementSide,
    WireIconData, WireLength, WirePortalTarget, WireStyle, WireViewportPlacement,
};

// ---------------------------------------------------------------------------
// Build-time snapshot helper
// ---------------------------------------------------------------------------

/// Run a UI builder once against a fresh `RokuBackend` and return
/// the resulting command stream. Intended for build-time snapshotting:
/// a user-owned binary calls this, serializes the result to JSON,
/// writes it to `dist/ui.json`, and `idealyst build roku` picks it
/// up to bake into the .pkg.
///
/// The reactive owner is dropped before the return — anything that
/// depended on signal observation after the initial render is gone.
/// That's the design: a snapshot is a static, point-in-time picture.
/// Live reactivity has to be expressed via `#[method]` BrightScript
/// stubs that the runtime calls in response to events.
pub fn snapshot<F>(builder: F) -> Vec<RokuCommand>
where
    F: FnOnce() -> runtime_core::Element,
{
    let backend = Rc::new(RefCell::new(RokuBackend::new()));
    let tree = builder();
    let _owner = runtime_core::render(backend.clone(), tree);
    let cmds = backend.borrow_mut().drain();
    cmds
}

/// Same as [`snapshot`] but serializes to a JSON string, ready to
/// write to disk.
pub fn snapshot_to_json<F>(builder: F) -> Result<String, serde_json::Error>
where
    F: FnOnce() -> runtime_core::Element,
{
    serde_json::to_string(&snapshot(builder))
}

/// Same as [`snapshot_to_json`] but pretty-printed — easier to
/// eyeball when debugging the build pipeline.
pub fn snapshot_to_pretty_json<F>(builder: F) -> Result<String, serde_json::Error>
where
    F: FnOnce() -> runtime_core::Element,
{
    serde_json::to_string_pretty(&snapshot(builder))
}

// ---------------------------------------------------------------------------
// HandlerTable
// ---------------------------------------------------------------------------

/// Holds the Rust-side closures the BrightScript client cannot
/// execute. The client emits `{ handler: <id>, payload: ... }`
/// messages back through the transport; the embedder looks the
/// handler up here and dispatches.
///
/// Three variants because the wire payload shape differs: a plain
/// click has no payload, a text-change carries a `String`, a slider
/// carries `f32`. Toggles share the bool slot.
#[derive(Default)]
pub struct HandlerTable {
    pub unit: Vec<(HandlerId, Rc<dyn Fn()>)>,
    pub string: Vec<(HandlerId, Rc<dyn Fn(String)>)>,
    pub bool_: Vec<(HandlerId, Rc<dyn Fn(bool)>)>,
    pub float: Vec<(HandlerId, Rc<dyn Fn(f32)>)>,
}

impl std::fmt::Debug for HandlerTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerTable")
            .field("unit", &self.unit.len())
            .field("string", &self.string.len())
            .field("bool_", &self.bool_.len())
            .field("float", &self.float.len())
            .finish()
    }
}

impl HandlerTable {
    pub fn dispatch_unit(&self, id: HandlerId) {
        if let Some((_, f)) = self.unit.iter().find(|(h, _)| *h == id) {
            f();
        }
    }
    pub fn dispatch_string(&self, id: HandlerId, value: String) {
        if let Some((_, f)) = self.string.iter().find(|(h, _)| *h == id) {
            f(value);
        }
    }
    pub fn dispatch_bool(&self, id: HandlerId, value: bool) {
        if let Some((_, f)) = self.bool_.iter().find(|(h, _)| *h == id) {
            f(value);
        }
    }
    pub fn dispatch_float(&self, id: HandlerId, value: f32) {
        if let Some((_, f)) = self.float.iter().find(|(h, _)| *h == id) {
            f(value);
        }
    }
}

// ---------------------------------------------------------------------------
// RokuBackend
// ---------------------------------------------------------------------------

/// The Roku-side backend implementation. Stores a queue of pending
/// commands and a handler table for events the client emits back.
///
/// Public surface for embedders:
/// - [`RokuBackend::new`] constructs an empty backend.
/// - [`RokuBackend::drain`] takes all queued commands and clears the queue.
/// - [`RokuBackend::handlers`] borrows the handler table so the
///   transport can dispatch incoming events.
#[derive(Debug)]
pub struct RokuBackend {
    commands: Vec<RokuCommand>,
    handlers: RefCell<HandlerTable>,
    next_node: u64,
    next_handler: u64,
    /// Signal IDs already shipped via a `CreateSignal` command. The
    /// walker calls `note_signal_initial` once per binding-per-signal
    /// pair; deduping here means each signal lands on the wire
    /// exactly once with its snapshot-time initial value.
    created_signals: std::collections::HashSet<u64>,
    /// Stack of in-progress slot-capture buffers. While the stack is
    /// non-empty, every command produced by the walker is pushed
    /// onto the top buffer instead of the main `commands` vec.
    /// Slot bindings (`bind_when!`, `bind_switch!`, `bind_repeat!`)
    /// open one frame per slot, walk the slot's subtree, then call
    /// `end_slot_capture(slot_root)` which pops the frame and stores
    /// it in `captured_slots` keyed by the slot's root node id.
    capture_stack: Vec<Vec<RokuCommand>>,
    /// Slot subtrees captured during the snapshot walk, indexed by
    /// their root node id. Drained when the matching `note_*_binding`
    /// fires and the slot is packaged into its `BindWhen`/
    /// `BindSwitch`/`BindRepeat` command.
    captured_slots: std::collections::HashMap<NodeId, Vec<RokuCommand>>,
}

impl Default for RokuBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RokuBackend {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            handlers: RefCell::new(HandlerTable::default()),
            // 0 is reserved as a sentinel ("no node"); start at 1.
            next_node: 1,
            next_handler: 1,
            created_signals: std::collections::HashSet::new(),
            capture_stack: Vec::new(),
            captured_slots: std::collections::HashMap::new(),
        }
    }

    /// Take the queued command list, leaving the backend's queue
    /// empty for the next batch.
    pub fn drain(&mut self) -> Vec<RokuCommand> {
        std::mem::take(&mut self.commands)
    }

    /// Borrow the handler table. The transport calls
    /// `dispatch_unit` / `dispatch_string` / etc. on it when it
    /// receives an event message from the client.
    pub fn handlers(&self) -> std::cell::Ref<'_, HandlerTable> {
        self.handlers.borrow()
    }

    /// Mutable handle for tests/inspection — usually you call the
    /// dispatch methods directly.
    pub fn handlers_mut(&self) -> std::cell::RefMut<'_, HandlerTable> {
        self.handlers.borrow_mut()
    }

    fn mint_node(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        id
    }

    fn mint_handler(&mut self) -> HandlerId {
        let id = HandlerId(self.next_handler);
        self.next_handler += 1;
        id
    }

    fn push(&mut self, cmd: RokuCommand) {
        if let Some(top) = self.capture_stack.last_mut() {
            top.push(cmd);
        } else {
            self.commands.push(cmd);
        }
    }

    /// Drain the captured commands for a slot, identified by the
    /// slot's root node id. Returns an empty Vec if no slot was
    /// captured for that id — should not happen in well-formed
    /// snapshots but we tolerate it rather than panic.
    fn take_captured_slot(&mut self, root: NodeId) -> Vec<RokuCommand> {
        self.captured_slots.remove(&root).unwrap_or_default()
    }

    fn lower_icon(&self, data: &IconData) -> WireIconData {
        WireIconData {
            // The framework treats the static `paths` slice pointer
            // as the icon's stable identity — same icon, same address.
            cache_key: data.paths.as_ptr() as usize as u64,
            viewport_width: data.view_box.0 as f32,
            viewport_height: data.view_box.1 as f32,
            paths: data.paths.iter().map(|s| s.to_string()).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Virtualizer slot inspection
// ---------------------------------------------------------------------------

/// Decide whether a captured row template can lower to a native
/// `MarkupList` (Some) or has to fall back to `BindRepeat` (None).
///
/// V1 accepts the shape `Text { method(signals, [i]) }` — one
/// `CreateText` node, optional decoration (`ApplyStyle*`,
/// `UpdateText`), and exactly one `BindText` driving its text. The
/// returned `DynamicField` becomes the row's lone ContentNode
/// field (`title`), watched by the generated item component.
///
/// Returning `None` is a signal to keep BindRepeat semantics —
/// every other row shape (multi-node, mixed kinds, nested
/// bindings) routes through that path until codegen learns more
/// row patterns.
fn inspect_simple_text_row(
    slot: &command::Slot,
    row_index_signal_id: Option<u64>,
) -> Option<Vec<command::DynamicField>> {
    use command::RokuCommand as C;

    let mut create_text_id: Option<NodeId> = None;
    let mut bind_text: Option<(NodeId, Vec<SignalId>, String)> = None;
    let mut saw_other_node = false;

    for cmd in &slot.commands {
        match cmd {
            C::CreateText { id, .. } => {
                if create_text_id.is_some() {
                    return None;
                }
                create_text_id = Some(*id);
            }
            C::BindText { node_id, signal_ids, method } => {
                if bind_text.is_some() {
                    return None;
                }
                bind_text =
                    Some((*node_id, signal_ids.clone(), method.clone()));
            }
            // Tolerated decoration on the lone Text node.
            C::ApplyStyle { .. } | C::ApplyStyleStates { .. } | C::UpdateText { .. } => {}
            // Any structural / reactive sibling kicks us back to
            // BindRepeat. We can grow this matcher to cover more
            // shapes incrementally.
            C::CreateView { .. }
            | C::CreateButton { .. }
            | C::CreateImage { .. }
            | C::CreateIcon { .. }
            | C::CreatePressable { .. }
            | C::CreateScrollView { .. }
            | C::CreateReactiveAnchor { .. }
            | C::CreateTextInput { .. }
            | C::CreateToggle { .. }
            | C::CreateSlider { .. }
            | C::CreateActivityIndicator { .. } => saw_other_node = true,
            C::BindWhen { .. }
            | C::BindSwitch { .. }
            | C::BindRepeat { .. }
            | C::CreateMarkupList { .. }
            | C::Insert { .. } => return None,
            _ => {}
        }
        if saw_other_node {
            return None;
        }
    }

    let (bound_node, signal_ids, method) = bind_text?;
    let root = create_text_id?;
    if bound_node != root {
        return None;
    }
    let _ = row_index_signal_id; // signal_ids already encodes the row-index slot
    Some(vec![command::DynamicField {
        name: "title".to_string(),
        method,
        signal_ids,
        kind: command::DynamicFieldKind::Text,
    }])
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

impl Backend for RokuBackend {
    type Node = NodeId;

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Roku
    }

    fn create_view(
        &mut self,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateView { id });
        id
    }

    fn create_text(
        &mut self,
        content: &str,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateText {
            id,
            content: content.to_string(),
        });
        id
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let handler = self.mint_handler();
        // Roku has no host runtime to evaluate the closure; we ship
        // the structured metadata (method + signal ids + optional
        // output signal) as a `BindButton` wire op below. The
        // closure itself is still registered in the handler table
        // so a host-side runtime-server shell (dev mode) can fire it; in
        // baked-binary builds the device's transpiled #[method]
        // does the work and the closure is dead weight.
        self.handlers
            .borrow_mut()
            .unit
            .push((handler, on_click.fire.clone()));
        let leading = leading_icon.map(|d| self.lower_icon(d));
        let trailing = trailing_icon.map(|d| self.lower_icon(d));
        self.push(RokuCommand::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
        });
        // Carry the structured metadata onto the wire if the Action
        // has any (i.e. came from a `#[method]`-backed handler). An
        // opaque Action (closure with empty method) skips this —
        // generator backends can't ship a nameless handler.
        if !on_click.is_opaque() {
            // Declare each input signal first so the device has a
            // value to read at dispatch time.
            for (sid, val) in on_click.inputs.iter().zip(on_click.initial.iter()) {
                self.note_signal_initial(*sid, val);
            }
            self.push(RokuCommand::BindButton {
                button_id: id,
                input_signal_ids: on_click.inputs.iter().map(|i| SignalId(*i)).collect(),
                method: on_click.method.to_string(),
                output_signal_id: on_click.output.map(SignalId),
            });
        }
        id
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().unit.push((handler, on_click));
        self.push(RokuCommand::CreatePressable {
            id,
            on_click: handler,
        });
        id
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateReactiveAnchor { id });
        id
    }

    // -----------------------------------------------------------
    // Portals — emitted as a `CreatePortal` wire op. The device-side
    // BrightScript runtime materializes a `Group` parented to the
    // root scene at top z-order; `target` carries the positioning
    // intent so the runtime can compute translation / size locally.
    //
    // Anchor targets currently ship without a backing rect signal
    // because the framework's `AnchorTarget` is opaque (it exposes
    // `.rect()` for runtime backends but no signal id). A future
    // pass should expose the anchor's reactive id so the device can
    // subscribe and reposition; for now the runtime falls back to
    // the side/align hints and any explicit absolute style the
    // composition supplies. `Named` targets aren't supported.
    // -----------------------------------------------------------

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        use runtime_core::primitives::portal as p;
        let id = self.mint_node();
        let on_dismiss_handler = on_dismiss.map(|cb| {
            let h = self.mint_handler();
            self.handlers.borrow_mut().unit.push((h, cb));
            h
        });
        let wire_target = match target {
            p::PortalTarget::Viewport(placement) => command::WirePortalTarget::Viewport {
                placement: match placement {
                    p::ViewportPlacement::Center => command::WireViewportPlacement::Center,
                    p::ViewportPlacement::Top => command::WireViewportPlacement::Top,
                    p::ViewportPlacement::Bottom => command::WireViewportPlacement::Bottom,
                    p::ViewportPlacement::Left => command::WireViewportPlacement::Left,
                    p::ViewportPlacement::Right => command::WireViewportPlacement::Right,
                    p::ViewportPlacement::FullScreen => {
                        command::WireViewportPlacement::FullScreen
                    }
                },
            },
            p::PortalTarget::Anchor { side, align, offset, .. } => {
                // No live anchor-rect signal yet — the Roku runtime
                // applies the side/align/offset hints against
                // whatever the composition lays down. Carrying a
                // sentinel id (0) tells the BS client this binding
                // is static; revisit once `AnchorTarget` exposes its
                // backing signal id to generator backends.
                command::WirePortalTarget::Anchor {
                    anchor_rect_signal_id: SignalId(0),
                    side: match side {
                        p::ElementSide::Above => command::WireElementSide::Above,
                        p::ElementSide::Below => command::WireElementSide::Below,
                        p::ElementSide::Start => command::WireElementSide::Start,
                        p::ElementSide::End => command::WireElementSide::End,
                    },
                    align: match align {
                        p::ElementAlign::Start => command::WireElementAlign::Start,
                        p::ElementAlign::Center => command::WireElementAlign::Center,
                        p::ElementAlign::End => command::WireElementAlign::End,
                    },
                    offset,
                }
            }
            p::PortalTarget::Named(slot) => command::WirePortalTarget::Named {
                slot: slot.to_string(),
            },
        };
        self.push(RokuCommand::CreatePortal {
            id,
            target: wire_target,
            on_dismiss: on_dismiss_handler,
            trap_focus,
        });
        id
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateImage {
            id,
            src: src.to_string(),
            alt: alt.map(|s| s.to_string()),
        });
        id
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        self.push(RokuCommand::UpdateImageSrc {
            id: *node,
            src: src.to_string(),
        });
    }

    fn create_icon(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let wire = self.lower_icon(data);
        self.push(RokuCommand::CreateIcon {
            id,
            data: wire,
            color: color.map(|c| WireColor::literal(c.0.clone())),
        });
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        self.push(RokuCommand::UpdateIconColor {
            id: *node,
            color: WireColor::literal(color.0.clone()),
        });
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // `_on_key_down` is unused on Roku — the SceneGraph keyboard
        // surface doesn't expose pre-default key interception in the
        // way Web/UIKit/Android do. Document explicitly so the
        // asymmetry is visible at the API boundary.
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().string.push((handler, on_change));
        self.push(RokuCommand::CreateTextInput {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(|s| s.to_string()),
            on_change: handler,
        });
        id
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        self.push(RokuCommand::UpdateTextInputValue {
            id: *node,
            value: value.to_string(),
        });
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().bool_.push((handler, on_change));
        self.push(RokuCommand::CreateToggle {
            id,
            initial_value,
            on_change: handler,
        });
        id
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        self.push(RokuCommand::UpdateToggleValue { id: *node, value });
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().float.push((handler, on_change));
        self.push(RokuCommand::CreateSlider {
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
        self.push(RokuCommand::UpdateSliderValue { id: *node, value });
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        _on_scroll: Option<std::rc::Rc<dyn Fn(f32, f32)>>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateScrollView { id, horizontal });
        id
    }

    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let wire_size = match size {
            ActivityIndicatorSize::Small => command::ActivityIndicatorSize::Small,
            ActivityIndicatorSize::Large => command::ActivityIndicatorSize::Large,
        };
        self.push(RokuCommand::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor::literal(c.0.clone())),
        });
        id
    }

    // Graphics is GPU-bound; the wgpu surface required by `on_ready`
    // doesn't exist on a Roku device (no native GPU API exposed to
    // BrightScript). Emit a placeholder view and drop the callbacks
    // — the framework's effect graph won't crash, but the surface
    // will never become ready. This is a documented gap; embedders
    // using Graphics primitives shouldn't target Roku.
    fn create_graphics(
        &mut self,
        _on_ready: OnReady,
        _on_resize: OnResize,
        _on_lost: OnLost,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateView { id });
        id
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        self.push(RokuCommand::Insert {
            parent: *parent,
            child,
        });
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        self.push(RokuCommand::UpdateText {
            id: *node,
            content: content.to_string(),
        });
    }

    fn note_text_binding(
        &mut self,
        node: &Self::Node,
        signal_ids: &[u64],
        method: &'static str,
    ) {
        // The walker hands us a `TextSource::Bound` after the
        // `create_text` step; we round-trip the binding into the
        // wire stream so the device-side runtime can subscribe the
        // Label to the signals and apply the transformer on every
        // change. The subsequent Effect will still fire once at
        // snapshot time and emit a redundant `UpdateText` — that's
        // a one-line wire dup with the same string the BindText's
        // initial subscriber-fire would produce anyway, so it's a
        // visual no-op. Worth optimizing later if wire size matters.
        self.push(RokuCommand::BindText {
            node_id: *node,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            method: method.to_string(),
        });
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        let then_slot = command::Slot {
            root_node_id: *then_node,
            commands: self.take_captured_slot(*then_node),
        };
        let otherwise_slot = command::Slot {
            root_node_id: *otherwise_node,
            commands: self.take_captured_slot(*otherwise_node),
        };
        self.push(RokuCommand::BindWhen {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            cond_method: cond_method.to_string(),
            then_slot,
            otherwise_slot,
        });
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        let arms_wire: Vec<command::SwitchArm> = arms
            .iter()
            .map(|(pat, node)| command::SwitchArm {
                pattern: pat.clone(),
                slot: command::Slot {
                    root_node_id: *node,
                    commands: self.take_captured_slot(*node),
                },
            })
            .collect();
        let default_slot = command::Slot {
            root_node_id: *default_node,
            commands: self.take_captured_slot(*default_node),
        };
        self.push(RokuCommand::BindSwitch {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            cond_method: cond_method.to_string(),
            arms: arms_wire,
            default_slot,
        });
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        let row_template = command::Slot {
            root_node_id: *row_template,
            commands: self.take_captured_slot(*row_template),
        };
        self.push(RokuCommand::BindRepeat {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            count_method: count_method.to_string(),
            row_template,
            row_index_signal_id: row_index_signal_id.map(SignalId),
        });
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        let row_template = command::Slot {
            root_node_id: *row_template,
            commands: self.take_captured_slot(*row_template),
        };
        // Inspect the slot. Today we only lower row templates that
        // are structurally one Text node with one BindText (and any
        // ApplyStyle/UpdateText decoration). Anything else falls
        // back to the existing BindRepeat path so the framework
        // stays correct on Roku while we grow MarkupList coverage
        // primitive-by-primitive.
        if let Some(dynamic_fields) = inspect_simple_text_row(
            &row_template,
            row_index_signal_id,
        ) {
            // Component name is keyed on the anchor's id — anchors
            // are unique per virtualizer in the snapshot, so this
            // produces a stable, unique name build-roku can use to
            // emit the .xml/.brs pair.
            let item_component = format!("IdealystListItem_{}", anchor.0);
            self.push(RokuCommand::CreateMarkupList {
                anchor_id: *anchor,
                item_component,
                count_method: count_method.to_string(),
                signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
                row_index_signal_id: row_index_signal_id.map(SignalId),
                dynamic_fields,
                row_template,
                // V1: hard-coded scroll-axis cell size. For
                // vertical lists this is row height; for
                // horizontal carousels we interpret it as the
                // row's height (cell width is then derived from
                // viewport / visibleItems). A future iteration
                // should read this from the row template's style
                // (height for vertical, width for horizontal).
                item_size: 200.0,
                horizontal,
            });
        } else {
            // Generic row template — fall back to the BindRepeat
            // path (the device-side replay machinery handles
            // arbitrary row shapes).
            self.push(RokuCommand::BindRepeat {
                anchor_id: *anchor,
                signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
                count_method: count_method.to_string(),
                row_template,
                row_index_signal_id: row_index_signal_id.map(SignalId),
            });
        }
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        true
    }

    fn begin_slot_capture(&mut self) {
        self.capture_stack.push(Vec::new());
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        // Walker is expected to balance begin/end calls. Popping
        // without a matching begin would mean the walker has a bug
        // — error loudly rather than silently swallow the slot.
        let buf = self
            .capture_stack
            .pop()
            .expect("end_slot_capture without matching begin_slot_capture");
        self.captured_slots.insert(*slot_root, buf);
    }

    fn note_signal_initial(
        &mut self,
        signal_id: u64,
        value: &runtime_core::__serde_json::Value,
    ) {
        // First-time signal observation: declare the signal to the
        // device with its current value. Subsequent observations of
        // the same id are dropped — the value lives in the BS-side
        // arena once it's been seeded; later mutations come from
        // button actions on the device, not from the framework's
        // snapshot. Without dedup, every `bind!` that names the
        // same signal would emit a redundant CreateSignal and reset
        // it back to its initial each time.
        if self.created_signals.insert(signal_id) {
            // Bypass `push` — signals are global. If we routed this
            // through `push` and a nested bind happened to be capturing
            // when its inner signal was first declared, the
            // CreateSignal would land in a slot buffer and get
            // re-emitted on every slot replay, clobbering the signal's
            // current value.
            self.commands.push(RokuCommand::CreateSignal {
                id: SignalId(signal_id),
                initial: value.clone(),
            });
        }
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.push(RokuCommand::UpdateButtonLabel {
            id: *node,
            label: label.to_string(),
        });
    }

    fn clear_children(&mut self, node: &Self::Node) {
        self.push(RokuCommand::ClearChildren { parent: *node });
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let wire = style::lower_style(style);
        self.push(RokuCommand::ApplyStyle {
            id: *node,
            style: Box::new(wire),
        });
    }

    fn handles_states_natively(&self) -> bool {
        // Same posture as the web backend: the framework hands us
        // the base rules plus per-state overlays declaratively, and
        // we ship them through a single wire command. The Roku-side
        // runtime maintains its own focus/press state (driven by
        // D-pad input) and applies the right merged style locally —
        // no Rust round-trip per state change.
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
    ) {
        // Find the overlay (if any) for each well-known state.
        // The framework hands us a list, not a map, so we scan
        // once per state.
        let find = |target: runtime_core::StateBits| -> Option<Box<WireStyle>> {
            overlays
                .iter()
                .find(|(bits, _)| *bits == target)
                .map(|(_, rules)| Box::new(style::lower_style(rules)))
        };

        self.push(RokuCommand::ApplyStyleStates {
            id: *node,
            base: Box::new(style::lower_style(base)),
            hovered: find(runtime_core::StateBits::HOVERED),
            focused: find(runtime_core::StateBits::FOCUSED),
            pressed: find(runtime_core::StateBits::PRESSED),
            disabled: find(runtime_core::StateBits::DISABLED),
        });
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        self.push(RokuCommand::SetDisabled {
            id: *node,
            disabled,
        });
    }

    fn install_tokens(&mut self, _tokens: &[runtime_core::TokenEntry]) {
        // No-op (matches iOS / Android posture).
        //
        // The Roku wire protocol has no runtime variable layer — there is no
        // analog of CSS custom properties on SceneGraph. Styles are lowered
        // through `style::lower_style` at every `apply_style` call, and any
        // `Tokenized<T>` field has already been read via `Tokenized::value()`
        // by then, producing a literal `WireColor` / `WireLength` / number in
        // the emitted `ApplyStyle` command.
        //
        // When the app calls `update_tokens(...)`, the framework's
        // tokens-version signal re-fires every styled effect that subscribed
        // to any of the changed tokens; each of those effects calls
        // `apply_style` again with freshly-resolved literal values. So the
        // wire stream picks up the new values automatically — this method
        // doesn't need to emit anything.
        //
        // Previously this panicked via `unimplemented!()`, breaking any app
        // that touched the token system on Roku (theme switching, custom
        // tokens). The earlier comment referenced a removed
        // `register_theme_variant` hook; the framework moved on to a
        // re-apply-driven model, so the no-op is now the correct behavior.
    }

    fn update_tokens(&mut self, _tokens: &[runtime_core::TokenEntry]) {
        // See `install_tokens` above — same no-op rationale. Updated token
        // values propagate to the wire via re-application of every styled
        // effect that subscribed to a changed token.
    }

    fn finish(&mut self, root: Self::Node) {
        self.push(RokuCommand::Finish { root });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_view_emits_create_view() {
        let mut be = RokuBackend::new();
        let _ = be.create_view(&Default::default());
        let cmds = be.drain();
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], RokuCommand::CreateView { .. }));
    }

    #[test]
    fn insert_records_parent_child() {
        let mut be = RokuBackend::new();
        let mut parent = be.create_view(&Default::default());
        let child = be.create_text("hi", &Default::default());
        be.insert(&mut parent, child);
        let cmds = be.drain();
        // create_view, create_text, insert
        assert_eq!(cmds.len(), 3);
        match &cmds[2] {
            RokuCommand::Insert { parent: p, child: c } => {
                assert_eq!(*p, parent);
                assert_eq!(*c, child);
            }
            other => panic!("expected Insert, got {:?}", other),
        }
    }

    #[test]
    fn button_handler_dispatches() {
        use runtime_core::IntoAction;
        let mut be = RokuBackend::new();
        let counter = Rc::new(std::cell::Cell::new(0u32));
        let counter2 = counter.clone();
        let on_click = (move || counter2.set(counter2.get() + 1)).into_action();
        let _ = be.create_button("ok", &on_click, None, None, &Default::default());

        let cmds = be.drain();
        let handler_id = match &cmds[0] {
            RokuCommand::CreateButton { on_click, .. } => *on_click,
            _ => panic!("expected CreateButton"),
        };

        be.handlers().dispatch_unit(handler_id);
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn regression_roku_install_and_update_tokens_no_panic() {
        // Regression for the `install_tokens` / `update_tokens`
        // `unimplemented!()` panic that blocked any Roku app using the
        // token system (theme switching, custom tokens). Both calls must
        // be no-ops: the Roku wire protocol has no runtime variable
        // layer, and the framework re-fires every styled effect on
        // token updates so apply_style picks up the new literal values.
        use runtime_core::{TokenEntry, TokenValue};

        let mut be = RokuBackend::new();

        let tokens = [
            TokenEntry {
                name: "test-token",
                value: TokenValue::Number(1.0),
            },
            TokenEntry {
                name: "primary-color",
                value: TokenValue::Color(runtime_core::Color(
                    "#ff0000".to_string(),
                )),
            },
        ];

        // Initial install at app boot — must not panic.
        be.install_tokens(&tokens);

        // A subsequent theme switch — must not panic.
        let updated = [TokenEntry {
            name: "test-token",
            value: TokenValue::Number(2.0),
        }];
        be.update_tokens(&updated);

        // Empty input is also legal (degenerate update).
        be.update_tokens(&[]);

        // No-op semantics: neither call should have emitted any
        // commands onto the wire. Token values flow into the wire via
        // re-application of styled effects (apply_style → ApplyStyle
        // commands), not via a dedicated install/update token command.
        let cmds = be.drain();
        assert!(
            cmds.is_empty(),
            "install_tokens / update_tokens must not emit wire commands, \
             got: {:?}",
            cmds
        );
    }

    #[test]
    fn commands_serialize_to_json() {
        let mut be = RokuBackend::new();
        let mut parent = be.create_view(&Default::default());
        let child = be.create_text("hello", &Default::default());
        be.insert(&mut parent, child);
        be.finish(parent);
        let cmds = be.drain();
        let json = serde_json::to_string(&cmds).expect("serialize");
        assert!(json.contains("CreateView"));
        assert!(json.contains("CreateText"));
        assert!(json.contains("Insert"));
        assert!(json.contains("Finish"));
    }
}
