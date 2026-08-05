//! Text-family builders: `text()`, `button()`.

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::IntoAction;
use runtime_shared::primitives::icon::IconData;
use runtime_shared::styled_text::TextRun;
use runtime_shared::{ButtonHandle, TextHandle};
use runtime_scene::{item, Element};
use runtime_world::{IntoValue, Value};

use crate::prims::{ButtonPrim, PrimCell, TextPrim, TextSourceProp};
use crate::style_attach::IntoStyleProp;

use super::TextContent;

/// Start a `text` — a text leaf. Content via [`content`](TextBuilder::content)
/// (or [`child`](TextBuilder::child), the idea-lite spelling), styled
/// runs via [`runs`](TextBuilder::runs).
pub fn text() -> TextBuilder {
    TextBuilder {
        prim: TextPrim {
            test_id: None,
            content: TextSourceProp::Value(Value::Const(String::new())),
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct TextBuilder {
    prim: TextPrim,
}

impl TextBuilder {
    /// Set the content — anything printable, static or reactive
    /// ([`TextContent`] coercion). An f-string assembly
    /// (`glue::AssembledText`) routes its pre-decomposed `JsBinding`
    /// through here too.
    pub fn content(mut self, content: impl TextContent) -> Self {
        self.prim.content = content.into_content_prop();
        self
    }

    /// idea-lite spelling of [`content`](Self::content): the macro puts a
    /// text tag's brace block here.
    pub fn child(self, content: impl TextContent) -> Self {
        self.content(content)
    }

    /// Inline-styled runs (basic path: one `create_styled_text`; theme
    /// re-realization is deferred — see crate docs).
    pub fn runs(mut self, runs: Vec<TextRun>) -> Self {
        self.prim.content = TextSourceProp::Runs(runs);
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

    pub fn on_handle(mut self, fill: impl FnOnce(TextHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `button` — label + press action.
pub fn button() -> ButtonBuilder {
    ButtonBuilder {
        prim: ButtonPrim {
            test_id: None,
            label: Value::Const(String::new()),
            on_press: IntoAction::into_action(|| {}),
            leading_icon: None,
            trailing_icon: None,
            disabled: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct ButtonBuilder {
    prim: ButtonPrim,
}

impl ButtonBuilder {
    pub fn label(mut self, label: impl TextContent) -> Self {
        self.prim.label = label.into_content();
        self
    }

    /// The press action. Accepts a plain closure or a full
    /// [`Action`](runtime_shared::Action) (server-fn metadata preserved).
    pub fn on_press(mut self, action: impl IntoAction) -> Self {
        self.prim.on_press = action.into_action();
        self
    }

    pub fn leading_icon(mut self, icon: IconData) -> Self {
        self.prim.leading_icon = Some(icon);
        self
    }

    pub fn trailing_icon(mut self, icon: IconData) -> Self {
        self.prim.trailing_icon = Some(icon);
        self
    }

    pub fn disabled(mut self, disabled: impl IntoValue<bool>) -> Self {
        self.prim.disabled = Some(disabled.into_value());
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

    pub fn on_handle(mut self, fill: impl FnOnce(ButtonHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
