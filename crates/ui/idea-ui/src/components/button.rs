//! `Button` — the styled clickable. Pairs an [`IntentTag`] (semantic
//! action vocabulary) with a [`ButtonKind`] (visual treatment).
//!
//! ```ignore
//! ui! {
//!     Button(
//!         label = "Save",
//!         on_click = on_save,
//!         intent = IntentTag::Primary,
//!         kind = ButtonKind::Solid,
//!         size = ButtonSize::Md,
//!     )
//! }
//! ```
//!
//! Built on top of [`framework_core::pressable`] (a tappable `<div>`
//! with no UA chrome on web), so the entire visual is owned by the
//! `Button` stylesheet's `appearance` variant axis. The component
//! joins `intent` + `kind` into the appearance key (e.g.
//! `(Danger, Outlined) → "danger_outlined"`).

use std::rc::Rc;

use framework_core::{
    text, IntoPrimitive, PressableHandle, Primitive, Ref, StyleApplication, VariantEnum,
};

use crate::stylesheets::Button;
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::ButtonSize;

/// Which of the seven semantic intents this button represents.
///
/// Built-in intent tags. Custom intents are out of scope at the
/// component layer for now — apps that want a "Hype" or "Beta"
/// coloring should add it as a theme extension and we'll wire a
/// custom-intent escape hatch in a follow-up.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum IntentTag {
    #[default]
    Primary,
    Secondary,
    Neutral,
    Success,
    Danger,
    Warning,
    Info,
}

impl IntentTag {
    /// Stable lowercase string used to compose the appearance axis
    /// key (`<intent>_<kind>`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Neutral => "neutral",
            Self::Success => "success",
            Self::Danger => "danger",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    /// All seven intent tags, in display order. Used by DocControls
    /// to populate the picker.
    pub fn all() -> &'static [IntentTag] {
        &[
            IntentTag::Primary,
            IntentTag::Secondary,
            IntentTag::Neutral,
            IntentTag::Success,
            IntentTag::Danger,
            IntentTag::Warning,
            IntentTag::Info,
        ]
    }
}

impl framework_core::VariantEnum for IntentTag {
    fn as_variant_str(self) -> &'static str {
        self.as_str()
    }
    fn all_variants() -> &'static [Self] {
        Self::all()
    }
}

/// Visual treatment a button uses to render its intent.
///
/// - [`Solid`](Self::Solid): filled background, contrasting text.
/// - [`Soft`](Self::Soft): tinted background, intent-colored text.
/// - [`Outlined`](Self::Outlined): transparent, intent-colored border + text.
/// - [`Ghost`](Self::Ghost): borderless transparent, intent-colored text.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ButtonKind {
    #[default]
    Solid,
    Soft,
    Outlined,
    Ghost,
}

impl ButtonKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Soft => "soft",
            Self::Outlined => "outlined",
            Self::Ghost => "ghost",
        }
    }
    pub fn all() -> &'static [ButtonKind] {
        &[
            ButtonKind::Solid,
            ButtonKind::Soft,
            ButtonKind::Outlined,
            ButtonKind::Ghost,
        ]
    }
}

impl framework_core::VariantEnum for ButtonKind {
    fn as_variant_str(self) -> &'static str {
        self.as_str()
    }
    fn all_variants() -> &'static [Self] {
        Self::all()
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ButtonProps {
    pub label: String,
    pub on_click: Rc<dyn Fn()>,
    pub intent: IntentTag,
    pub kind: ButtonKind,
    pub size: ButtonSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
    /// When `Some`, fills the given `Ref<PressableHandle>` on mount.
    /// Useful for anchoring an `Overlay` to this button.
    pub bind_to: Option<Ref<PressableHandle>>,
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            on_click: Rc::new(|| {}),
            intent: IntentTag::default(),
            kind: ButtonKind::default(),
            size: ButtonSize::default(),
            disabled: None,
            bind_to: None,
        }
    }
}

pub fn button(props: &ButtonProps) -> Primitive {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    let size = props.size;
    let intent = props.intent;
    let kind = props.kind;
    let disabled = props.disabled.clone();
    let bind_to = props.bind_to;

    // The `appearance` variant key is `<intent>_<kind>` — joined
    // here at the closure boundary so the stylesheet's pregenerated
    // class for that combo is hit.
    let appearance = format!("{}_{}", intent.as_str(), kind.as_str());

    let style = move || {
        // Tickle the theme so the apply-style effect subscribes —
        // theme swaps refresh every button without our help.
        let _ = framework_core::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(Button::sheet())
            .with("size", size.as_variant_str().to_string())
            .with("appearance", appearance.clone())
    };

    let children: Vec<Primitive> = vec![text(label).into_primitive()];
    let on_click_for_p = on_click.clone();
    let mut bound = framework_core::pressable(children, move || (on_click_for_p)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    bound.into_primitive()
}
