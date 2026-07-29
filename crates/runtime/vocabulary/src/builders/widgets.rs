//! Form-control builders: `toggle()`, `slider()`,
//! `activity_indicator()`, `text_input()`, `text_area()`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::activity_indicator::{
    ActivityIndicatorHandle, ActivityIndicatorSize,
};
use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use runtime_shared::primitives::slider::SliderHandle;
use runtime_shared::primitives::text_area::TextAreaHandle;
use runtime_shared::primitives::text_input::TextInputHandle;
use runtime_shared::primitives::toggle::ToggleHandle;
use runtime_shared::Color;
use runtime_scene::{item, Element};
use runtime_world::{IntoValue, Value};

use crate::prims::{
    ActivityIndicatorPrim, PrimCell, SliderPrim, TextAreaPrim, TextInputPrim, TogglePrim,
};
use crate::style_attach::IntoStyleProp;

/// Start a `toggle` (controlled): `value` is the source of truth,
/// `on_change` reports native flips.
pub fn toggle() -> ToggleBuilder {
    ToggleBuilder {
        prim: TogglePrim {
            test_id: None,
            value: Value::Const(false),
            on_change: Rc::new(|_| {}),
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct ToggleBuilder {
    prim: TogglePrim,
}

impl ToggleBuilder {
    pub fn value(mut self, value: impl IntoValue<bool>) -> Self {
        self.prim.value = value.into_value();
        self
    }

    pub fn on_change(mut self, f: impl Fn(bool) + 'static) -> Self {
        self.prim.on_change = Rc::new(f);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(ToggleHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `slider` (controlled), default range `0.0..=1.0`, no step.
pub fn slider() -> SliderBuilder {
    SliderBuilder {
        prim: SliderPrim {
            test_id: None,
            value: Value::Const(0.0),
            on_change: Rc::new(|_| {}),
            min: 0.0,
            max: 1.0,
            step: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct SliderBuilder {
    prim: SliderPrim,
}

impl SliderBuilder {
    pub fn value(mut self, value: impl IntoValue<f32>) -> Self {
        self.prim.value = value.into_value();
        self
    }

    pub fn on_change(mut self, f: impl Fn(f32) + 'static) -> Self {
        self.prim.on_change = Rc::new(f);
        self
    }

    /// Inclusive min/max.
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.prim.min = min;
        self.prim.max = max;
        self
    }

    /// Discrete step (values snap before dispatch, uniformly across
    /// backends).
    pub fn step(mut self, step: f32) -> Self {
        self.prim.step = Some(step);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(SliderHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start an `activity_indicator` (loading spinner), default `Small`.
pub fn activity_indicator() -> ActivityIndicatorBuilder {
    ActivityIndicatorBuilder {
        prim: ActivityIndicatorPrim {
            test_id: None,
            size: Value::Const(ActivityIndicatorSize::default()),
            color: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct ActivityIndicatorBuilder {
    prim: ActivityIndicatorPrim,
}

impl ActivityIndicatorBuilder {
    pub fn size(mut self, size: ActivityIndicatorSize) -> Self {
        self.prim.size = Value::Const(size);
        self
    }

    /// Live size — the spinner resizes in place.
    pub fn size_dyn(mut self, f: impl Fn() -> ActivityIndicatorSize + 'static) -> Self {
        self.prim.size = Value::Dyn(Box::new(f));
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.prim.color = Some(color);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(ActivityIndicatorHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `text_input` (controlled single-line editor).
pub fn text_input() -> TextInputBuilder {
    TextInputBuilder {
        prim: TextInputPrim {
            test_id: None,
            value: Value::Const(String::new()),
            on_change: Rc::new(|_| {}),
            on_key_down: None,
            on_blur: None,
            on_focus: None,
            placeholder: Value::Const(None),
            secure: Value::Const(false),
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct TextInputBuilder {
    prim: TextInputPrim,
}

impl TextInputBuilder {
    pub fn value(mut self, value: impl IntoValue<String>) -> Self {
        self.prim.value = value.into_value();
        self
    }

    pub fn on_change(mut self, f: impl Fn(String) + 'static) -> Self {
        self.prim.on_change = Rc::new(f);
        self
    }

    pub fn on_key_down(mut self, f: impl Fn(&KeyEvent) -> KeyOutcome + 'static) -> Self {
        self.prim.on_key_down = Some(Rc::new(f));
        self
    }

    /// Blur notifier; the returned
    /// [`BlurOutcome`](runtime_shared::primitives::text_input::BlurOutcome)
    /// lets the handler veto/redirect the blur (same contract as the old
    /// element's `on_blur`).
    pub fn on_blur(
        mut self,
        f: impl Fn() -> runtime_shared::primitives::text_input::BlurOutcome + 'static,
    ) -> Self {
        self.prim.on_blur = Some(Rc::new(f));
        self
    }

    /// Focus-change notifier (`true` on focus gain).
    pub fn on_focus(mut self, f: impl Fn(bool) + 'static) -> Self {
        self.prim.on_focus = Some(Rc::new(f));
        self
    }

    /// Static placeholder.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.prim.placeholder = Value::Const(Some(text.into()));
        self
    }

    /// Live placeholder (`None` clears).
    pub fn placeholder_dyn(mut self, f: impl Fn() -> Option<String> + 'static) -> Self {
        self.prim.placeholder = Value::Dyn(Box::new(f));
        self
    }

    /// Secure-entry (mask) mode — static or live (a `Dyn` source toggles
    /// the mask in place without rebuilding the input).
    pub fn secure(mut self, secure: impl IntoValue<bool>) -> Self {
        self.prim.secure = secure.into_value();
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(TextInputHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `text_area` (controlled multi-line editor; soft-wrap on,
/// unbounded autogrow by default).
pub fn text_area() -> TextAreaBuilder {
    TextAreaBuilder {
        prim: TextAreaPrim {
            test_id: None,
            value: Value::Const(String::new()),
            on_change: Rc::new(|_| {}),
            on_key_down: None,
            placeholder: None,
            wrap: true,
            min_rows: None,
            max_rows: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct TextAreaBuilder {
    prim: TextAreaPrim,
}

impl TextAreaBuilder {
    pub fn value(mut self, value: impl IntoValue<String>) -> Self {
        self.prim.value = value.into_value();
        self
    }

    pub fn on_change(mut self, f: impl Fn(String) + 'static) -> Self {
        self.prim.on_change = Rc::new(f);
        self
    }

    pub fn on_key_down(mut self, f: impl Fn(&KeyEvent) -> KeyOutcome + 'static) -> Self {
        self.prim.on_key_down = Some(Rc::new(f));
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.prim.placeholder = Some(text.into());
        self
    }

    pub fn wrap(mut self, wrap: bool) -> Self {
        self.prim.wrap = wrap;
        self
    }

    pub fn min_rows(mut self, rows: u32) -> Self {
        self.prim.min_rows = Some(rows);
        self
    }

    pub fn max_rows(mut self, rows: u32) -> Self {
        self.prim.max_rows = Some(rows);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(TextAreaHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
