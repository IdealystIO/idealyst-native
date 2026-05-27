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

use runtime_core::{text, IntoPrimitive, Primitive, StyleApplication, StyleRules, VariantEnum};

use idea_theme::extensible::{tone, variant, ResolutionCtx, ToneRef, VariantRef};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::IconButton as IconButtonSheet;
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

pub fn icon_button(props: &IconButtonProps) -> Primitive {
    let glyph = props.glyph.clone();
    let on_click = props.on_click.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size;
    let disabled = props.disabled.clone();

    // Cache key reflects every input that affects the resolved style:
    // size (via stylesheet variant axis) + tone + variant (via computed
    // layer). The stylesheet's `size` axis handles width/height; the
    // computed layer handles tone-driven appearance.
    let cache_key = format!(
        "icon-button+{}+{}+{}",
        variant.key(),
        tone.key(),
        size.as_variant_str(),
    );

    let style = move || {
        let _ = idea_theme::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let var = variant.clone();
        let tn = tone.clone();
        let compute = move || -> StyleRules {
            let theme = idea_theme::active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &*tn,
            };
            var.render(&ctx)
        };
        StyleApplication::new(IconButtonSheet::sheet())
            .with("size", size.as_variant_str().to_string())
            .with_computed(cache_key.clone(), compute)
    };

    let glyph_child = text(glyph).into_primitive();
    let mut bound = runtime_core::pressable(vec![glyph_child], move || (on_click)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    bound.into_primitive()
}
