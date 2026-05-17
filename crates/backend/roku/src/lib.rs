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
//! - **No native overlay/navigator**: the `Backend` trait's default
//!   `unimplemented!()` panics for `create_overlay`,
//!   `create_navigator`, etc. — implementing those means deciding
//!   how the BrightScript client expresses navigation stacks, which
//!   is out of scope for this initial pass.

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

use framework_core::{
    primitives::{
        activity_indicator::ActivityIndicatorSize, graphics::{OnLost, OnReady, OnResize},
        icon::IconData,
    },
    Backend, Color, StyleRules,
};

pub use command::{HandlerId, NodeId, RokuCommand, WireColor, WireIconData, WireLength, WireStyle};

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
    F: FnOnce() -> framework_core::Primitive,
{
    let backend = Rc::new(RefCell::new(RokuBackend::new()));
    let tree = builder();
    let _owner = framework_core::render(backend.clone(), tree);
    let cmds = backend.borrow_mut().drain();
    cmds
}

/// Same as [`snapshot`] but serializes to a JSON string, ready to
/// write to disk.
pub fn snapshot_to_json<F>(builder: F) -> Result<String, serde_json::Error>
where
    F: FnOnce() -> framework_core::Primitive,
{
    serde_json::to_string(&snapshot(builder))
}

/// Same as [`snapshot_to_json`] but pretty-printed — easier to
/// eyeball when debugging the build pipeline.
pub fn snapshot_to_pretty_json<F>(builder: F) -> Result<String, serde_json::Error>
where
    F: FnOnce() -> framework_core::Primitive,
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
        self.commands.push(cmd);
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
// Backend trait
// ---------------------------------------------------------------------------

impl Backend for RokuBackend {
    type Node = NodeId;

    fn create_view(&mut self) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateView { id });
        id
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
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
        on_click: Rc<dyn Fn()>,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
    ) -> Self::Node {
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().unit.push((handler, on_click));
        let leading = leading_icon.map(|d| self.lower_icon(d));
        let trailing = trailing_icon.map(|d| self.lower_icon(d));
        self.push(RokuCommand::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
        });
        id
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
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

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
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

    fn create_icon(&mut self, data: &IconData, color: Option<&Color>) -> Self::Node {
        let id = self.mint_node();
        let wire = self.lower_icon(data);
        self.push(RokuCommand::CreateIcon {
            id,
            data: wire,
            color: color.map(|c| WireColor(c.0.clone())),
        });
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        self.push(RokuCommand::UpdateIconColor {
            id: *node,
            color: WireColor(color.0.clone()),
        });
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
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

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateScrollView { id, horizontal });
        id
    }

    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
    ) -> Self::Node {
        let id = self.mint_node();
        let wire_size = match size {
            ActivityIndicatorSize::Small => command::ActivityIndicatorSize::Small,
            ActivityIndicatorSize::Large => command::ActivityIndicatorSize::Large,
        };
        self.push(RokuCommand::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor(c.0.clone())),
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

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        self.push(RokuCommand::SetDisabled {
            id: *node,
            disabled,
        });
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
        let _ = be.create_view();
        let cmds = be.drain();
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], RokuCommand::CreateView { .. }));
    }

    #[test]
    fn insert_records_parent_child() {
        let mut be = RokuBackend::new();
        let mut parent = be.create_view();
        let child = be.create_text("hi");
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
        let mut be = RokuBackend::new();
        let counter = Rc::new(std::cell::Cell::new(0u32));
        let counter2 = counter.clone();
        let on_click: Rc<dyn Fn()> = Rc::new(move || counter2.set(counter2.get() + 1));
        let _ = be.create_button("ok", on_click, None, None);

        let cmds = be.drain();
        let handler_id = match &cmds[0] {
            RokuCommand::CreateButton { on_click, .. } => *on_click,
            _ => panic!("expected CreateButton"),
        };

        be.handlers().dispatch_unit(handler_id);
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn commands_serialize_to_json() {
        let mut be = RokuBackend::new();
        let mut parent = be.create_view();
        let child = be.create_text("hello");
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
