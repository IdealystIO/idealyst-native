//! Roku backend: a command-emitting renderer that drives a
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
//! This backend implements `Host::Node = NodeId` — a pure
//! identifier — and translates every capability call into a
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
//! # Rendering surface
//!
//! [`newcore::start`] mounts a scene-element tree on a
//! `runtime_world::World` through the vocabulary's builtin handlers;
//! `RokuBackend`'s `runtime_scene::Host` + 30 capability impls live in
//! [`newcore`]. Every mechanism body moved there verbatim from this
//! crate's old `impl runtime_core::Backend` when the mega-trait was
//! deleted, so the emitted command stream is unchanged — pinned against
//! the old core's frozen streams by `tests/newcore_parity.rs` +
//! `tests/goldens/`. The flush model is embedder-driven: wrapped author
//! callbacks land in the [`HandlerTable`], and after dispatching device
//! events the embedder calls `newcore::settle()` before draining the
//! queue.
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
//! - **No native navigator**: navigators mount as ordinary vocabulary
//!   built-ins (swap/stack) over the View/Lifecycle capabilities;
//!   expressing a native navigation stack on the BrightScript client is
//!   out of scope for this pass. Portals are wired through
//!   `create_portal` — the
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
//! every `create_*` (for capability conformance with iOS / Android /
//! web) but currently **drops it on the floor** — the `_a11y` underscore
//! prefix marks the intentional no-op. The caps no-op defaults
//! for `update_accessibility` / `announce_for_accessibility` /
//! `dump_accessibility_tree` apply unchanged; we do not override
//! them because there is nothing meaningful to do.
//!
//! If a future Roku SDK exposes per-node semantic metadata (e.g. an
//! `accessibilityLabel` field on SceneGraph nodes, or an Audio Guide
//! announcement API), the plumbing point is here:
//!
//! 1. Rename each `_a11y` parameter in the capability impls (in
//!    [`newcore`]) to `a11y` (the `backend-roku-a11y` audit will flag
//!    the unused
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
pub mod dispatch_hook;
/// `runtime_scene::Host` + the 30 capability traits on [`RokuBackend`],
/// plus the boot entry and the `settle()` embedder boundary.
pub mod newcore;
mod style;

/// `#[method]` — annotate pure-logic functions for transpilation
/// into BrightScript at compile time. See
/// [`backend_roku_macros`](../backend_roku_macros/index.html) for
/// the supported Rust subset and emitted output shape.
pub use backend_roku_macros::method;

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::primitives::icon::IconData;

pub use command::{
    HandlerId, NodeId, RokuCommand, SignalId, WireColor, WireElementAlign, WireElementSide,
    WireIconData, WireLength, WirePortalTarget, WireStyle, WireViewportPlacement,
};

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
            // Fold `filled` into the key so the filled and outlined sprite
            // for one path set get distinct atlas entries.
            cache_key: (data.paths.as_ptr() as usize as u64) ^ (data.filled as u64),
            viewport_width: data.view_box.0 as f32,
            viewport_height: data.view_box.1 as f32,
            paths: data.paths.iter().map(|s| s.to_string()).collect(),
            filled: data.filled,
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // The mechanism lives on the capability traits (the `Backend`
    // mega-trait is gone), so these tests reach it through them; `Host`
    // supplies the structural ops.
    use runtime_scene::Host;
    use runtime_vocabulary::caps::{ButtonOps, LifecycleOps, StyleOps, TextOps, ViewOps};

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
        use runtime_shared::IntoAction;
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
        use runtime_shared::{TokenEntry, TokenValue};

        let mut be = RokuBackend::new();

        let tokens = [
            TokenEntry {
                name: "test-token",
                value: TokenValue::Number(1.0),
            },
            TokenEntry {
                name: "primary-color",
                value: TokenValue::Color(runtime_shared::Color(
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
