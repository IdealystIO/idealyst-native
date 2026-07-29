//! Form-control payloads: `toggle`, `slider`, `activity_indicator`,
//! `text_input`, `text_area`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::activity_indicator::{
    ActivityIndicatorHandle, ActivityIndicatorSize,
};
use runtime_shared::primitives::key::KeyDownHandler;
use runtime_shared::primitives::slider::SliderHandle;
use runtime_shared::primitives::text_area::TextAreaHandle;
use runtime_shared::primitives::text_input::{BlurHandler, FocusHandler, TextInputHandle};
use runtime_shared::primitives::toggle::ToggleHandle;
use runtime_shared::Color;
use runtime_world::Value;

use crate::style_attach::StyleProp;

/// The `toggle` primitive (`walker/toggle.rs`) — controlled pattern:
/// `value` is the source of truth, written back into the widget by the
/// binding (one `update_toggle_value` at mount, then per change);
/// `on_change` reports native flips upward.
pub struct TogglePrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub value: Value<bool>,
    pub on_change: Rc<dyn Fn(bool)>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ToggleHandle)>>,
}

/// The `slider` primitive (`walker/slider.rs`). The handler wraps
/// `on_change` to snap to `step` before dispatch so every backend
/// produces identical values (the walker's snap, ported).
pub struct SliderPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub value: Value<f32>,
    pub on_change: Rc<dyn Fn(f32)>,
    pub min: f32,
    pub max: f32,
    pub step: Option<f32>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(SliderHandle)>>,
}

/// The `activity_indicator` primitive (`walker/activity_indicator.rs`).
/// A `Dyn` size creates at the closure's initial value and resizes in
/// place (`update_activity_indicator_size`).
pub struct ActivityIndicatorPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub size: Value<ActivityIndicatorSize>,
    pub color: Option<Color>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ActivityIndicatorHandle)>>,
}

/// The `text_input` primitive (`walker/text_input.rs::build_text_input`)
/// — controlled `value` (written back per change, first write at mount),
/// in-place `secure`/`placeholder` updates for `Dyn` sources only (the
/// walker's `Reactive::Dynamic` gate), and the focus notifier installed
/// after style.
pub struct TextInputPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub value: Value<String>,
    pub on_change: Rc<dyn Fn(String)>,
    pub on_key_down: Option<KeyDownHandler>,
    pub on_blur: Option<BlurHandler>,
    pub on_focus: Option<FocusHandler>,
    pub placeholder: Value<Option<String>>,
    pub secure: Value<bool>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(TextInputHandle)>>,
}

/// The `text_area` primitive (`walker/text_input.rs::build_text_area`).
/// Placeholder/wrap/rows are create-time config (the old element carries
/// them statically); `value` is the controlled signal.
pub struct TextAreaPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub value: Value<String>,
    pub on_change: Rc<dyn Fn(String)>,
    pub on_key_down: Option<KeyDownHandler>,
    pub placeholder: Option<String>,
    pub wrap: bool,
    pub min_rows: Option<u32>,
    pub max_rows: Option<u32>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(TextAreaHandle)>>,
}
