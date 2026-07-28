//! Text-family payloads: `text`, `button`.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::icon::IconData;
use runtime_core::styled_text::TextRun;
use runtime_core::{Action, ButtonHandle, TextHandle};
use runtime_world::Value;

use crate::style_attach::StyleProp;

/// A text node's content source (`walker/text.rs`):
///
/// - `Value(Const)` — static content, one `create_text`, no effects;
/// - `Value(Dyn)` — reactive content: `create_text_with_id("")` when the
///   backend offers the batched-id fast path (updates via
///   `update_text_by_id`, released on teardown), else `create_text("")` +
///   per-fire `update_text`;
/// - `Runs` — basic inline-styled runs via `create_styled_text`. The
///   theme-cohort re-realization the old walker layers on native (and the
///   `JsBinding` fan-out fast path) are deferred — see the crate docs'
///   deferred set.
pub enum TextSourceProp {
    Value(Value<String>),
    Runs(Vec<TextRun>),
}

/// The `text` primitive.
pub struct TextPrim {
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
    pub label: Value<String>,
    pub on_press: Action,
    pub leading_icon: Option<IconData>,
    pub trailing_icon: Option<IconData>,
    pub disabled: Option<Value<bool>>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ButtonHandle)>>,
}
