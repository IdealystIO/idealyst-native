//! `IconButton` — square clickable for a glyph, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use idea_ui::extensible::icon_button::{icon_button, IconButtonProps, IconButtonSize};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     IconButton(
//!         glyph = "×",
//!         on_click = on_dismiss,
//!         tone = tone::Neutral,
//!         variant = variant::Ghost,
//!         size = IconButtonSize::Md,
//!     )
//! }
//! ```
//!
//! Tone + Variant are extensible (trait objects); `size` stays a
//! closed enum because it controls the square's width/height — a
//! continuous extension would require additional theme tokens that
//! aren't part of the `ButtonSize` slot vocabulary.

use std::rc::Rc;

use runtime_core::{component, text, IdealystSchema, IntoElement, Element, StyleApplication, VariantEnum};

use idea_theme::extensible::{installed_icon_button_sheet, tone, variant, ToneRef, VariantRef};

pub use crate::stylesheets::IconButtonSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct IconButtonProps {
    /// The single glyph/character rendered inside the square (e.g. `"×"`).
    pub glyph: String,
    /// Fires on press/click.
    pub on_click: Rc<dyn Fn()>,
    /// Semantic color palette (Neutral, Primary, Danger, …). Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Filled, Ghost, Soft, …). Default Filled.
    pub variant: VariantRef,
    /// Square dimension preset (Sm, Md, Lg). Default Md.
    pub size: IconButtonSize,
    /// When `Some`, the closure is polled to drive the disabled state;
    /// returning `true` blocks the press and dims the button.
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: String::new(),
            on_click: Rc::new(|| {}),
            tone: tone::Neutral.into(),
            variant: variant::Filled.into(),
            size: IconButtonSize::default(),
            disabled: None,
        }
    }
}

/// Renders a square, single-glyph clickable styled by the tone × variant
/// × size axes of the installed IconButton sheet.
#[component]
pub fn IconButton(props: &IconButtonProps) -> Element {
    let glyph = props.glyph.clone();
    let on_click = props.on_click.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size;
    let disabled = props.disabled.clone();

    let appearance_key = format!("{}_{}", tone.key(), variant.key());
    let size_key = size.as_variant_str().to_string();

    // Static style — build-time apply, no flicker (see Button).
    let style = StyleApplication::new(installed_icon_button_sheet())
        .with("appearance", appearance_key)
        .with("size", size_key);

    let glyph_child = text(glyph).into_element();
    let mut bound = runtime_core::pressable(vec![glyph_child], move || (on_click)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    bound.into_element()
}
