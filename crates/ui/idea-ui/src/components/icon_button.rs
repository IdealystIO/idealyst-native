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

use runtime_core::{text, IntoElement, Element, StyleApplication, VariantEnum};

use idea_theme::extensible::{installed_icon_button_sheet, tone, variant, ToneRef, VariantRef};

pub use crate::stylesheets::IconButtonSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct IconButtonProps {
    pub glyph: String,
    pub on_click: Rc<dyn Fn()>,
    pub tone: ToneRef,
    pub variant: VariantRef,
    pub size: IconButtonSize,
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

pub fn icon_button(props: &IconButtonProps) -> Element {
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
