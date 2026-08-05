//! Text-family payloads: `text`, `button`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::icon::IconData;
use runtime_shared::styled_text::TextRun;
use runtime_shared::{Action, ButtonHandle, TextHandle};
use runtime_world::Value;

use crate::style_attach::StyleProp;

/// A pre-decomposed reactive text binding — the new-core carrier of
/// `runtime_shared::JsBindingSpec` (the f-string fast path: signal ids +
/// template parts, so a JS-binding-capable backend fans out per fire
/// WITHOUT running a Rust effect per leaf).
///
/// Field contract mirrors the old spec (`template_parts` has exactly
/// `signal_ids.len() + 1` entries; captured non-signal values are
/// pre-formatted into the adjacent parts), plus one new-core-only
/// column: `tracked_reads`. Old-core web hooked delivery into
/// `Signal::set` itself; world signals have no write hook, so the text
/// handler installs ONE world-root notifier effect per signal (the
/// signal-class pattern — see `style_attach::ensure_signal_notifier`),
/// and that effect needs a TRACKED per-signal read to subscribe with.
pub struct JsTextBinding {
    pub signal_ids: Vec<u64>,
    pub template_parts: Vec<String>,
    pub initial_values: Vec<String>,
    /// Whole-text recompute, TRACKED — the Bound-shape effect body on
    /// backends without JS bindings.
    pub compute_fallback: Rc<dyn Fn() -> String>,
    /// Per-signal UNTRACKED read+format (the backend registration
    /// contract — parallel to `signal_ids`).
    pub stringifiers: Vec<Rc<dyn Fn() -> String>>,
    /// Per-signal TRACKED read+format (parallel to `signal_ids`) — each
    /// notifier effect's body.
    pub tracked_reads: Vec<Rc<dyn Fn() -> String>>,
}

/// A text node's content source (`walker/text.rs`):
///
/// - `Value(Const)` — static content, one `create_text`, no effects;
/// - `Value(Dyn)` — reactive content: `create_text_with_id("")` when the
///   backend offers the batched-id fast path (updates via
///   `update_text_by_id`, released on teardown), else `create_text("")` +
///   per-fire `update_text`;
/// - `JsBinding` — the f-string fast path: handed to
///   `register_reactive_text_binding` on JS-binding backends (one
///   per-signal notifier effect, zero per-leaf effects), Bound-shape
///   fallback (via `compute_fallback`) elsewhere;
/// - `Runs` — basic inline-styled runs via `create_styled_text` (the
///   theme-cohort re-realization the old walker layers on native is
///   deferred — see the crate docs' deferred set).
pub enum TextSourceProp {
    Value(Value<String>),
    /// Boxed: the binding spec carries six Vec/Rc columns (~136 bytes),
    /// and an unboxed variant sets the size of EVERY text payload —
    /// each builder-chain move memcpys it (part of the create-rows
    /// bench residual; see `StyleProp::Sheet`'s note).
    JsBinding(Box<JsTextBinding>),
    Runs(Vec<TextRun>),
}

/// The `text` primitive.
pub struct TextPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub content: TextSourceProp,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(TextHandle)>>,
}

/// The `button` primitive (`walker/button.rs`). A `Dyn` label creates
/// the button at the closure's initial value and installs an
/// `update_button_label` binding effect; a `Const` label installs no
/// effect. `on_press` is a full [`Action`] so generator backends keep
/// the structured metadata, exactly as the old walker passes it.
pub struct ButtonPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub label: Value<String>,
    pub on_press: Action,
    pub leading_icon: Option<IconData>,
    pub trailing_icon: Option<IconData>,
    pub disabled: Option<Value<bool>>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ButtonHandle)>>,
}
