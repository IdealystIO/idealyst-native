//! `Button` — the styled clickable, built on the extensible
//! Variant/Tone/Size/Shape trait surface.
//!
//! ```ignore
//! use idea_ui::extensible::{button, tone, variant, size, shape, ButtonProps};
//!
//! ui! {
//!     Button(
//!         label = "Save",
//!         on_click = on_save,
//!         tone = tone::Primary,
//!         variant = variant::Filled,
//!         size = size::Md,
//!         shape = shape::Md,
//!     )
//! }
//! ```
//!
//! The props store each axis as `Rc<dyn Trait>`. The blanket
//! `From<T> for Rc<dyn Trait>` conversions in
//! [`super`](crate::extensible) let call sites pass the ZST directly.

use std::rc::Rc;

use runtime_core::{
    text, FontWeight, IntoPrimitive, PressableHandle, Primitive, Ref, StyleApplication, StyleRules,
    Tokenized,
};

use idea_theme::extensible::{
    modifier_defaults, ButtonSizeRef, ResolutionCtx, ShapeRef, ToneRef, VariantRef,
};
use idea_theme::theme::IdeaThemeRef;
use crate::stylesheets::Button as ButtonSheet;

/// Props for the extensible Button. Each modifier axis is a typed
/// handle (`*Ref` newtype) so call sites can write
/// `tone: tone::Primary.into()` instead of `Rc::new(...)`. Built-in
/// defaults route to Filled/Primary/Md/Md.
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ButtonProps {
    pub label: String,
    pub on_click: Rc<dyn Fn()>,
    pub tone: ToneRef,
    pub variant: VariantRef,
    pub size: ButtonSizeRef,
    pub shape: ShapeRef,
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
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ButtonSizeRef::default(),
            shape: ShapeRef::default(),
            disabled: None,
            bind_to: None,
        }
    }
}

/// Render the Button. Builds a `StyleApplication` whose computed
/// layer invokes the active variant against the current modifier set
/// (tone, size, shape) and the active theme. The cache key is the
/// concatenation of the four modifier `key()`s, so identical modifier
/// combinations across many instances share one resolved class.
pub fn button(props: &ButtonProps) -> Primitive {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size.clone();
    let shape = props.shape.clone();
    let disabled = props.disabled.clone();
    let bind_to = props.bind_to;

    // Cache key for the computed layer — same modifier set produces
    // the same StyleRules, so the framework reuses one Rc per unique
    // (variant, tone, size, shape) combination per theme.
    let cache_key = format!(
        "{}+{}+{}+{}",
        variant.key(),
        tone.key(),
        size.key(),
        shape.key(),
    );

    let style = move || {
        // Touch the active theme so the apply-style Effect subscribes
        // to theme swaps. The downcast also gives us a typed
        // IdeaThemeRef for the inner `compute` closure to walk through.
        let _ = idea_theme::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");

        // Modifier handles cloned into the inner closure. The closure
        // is invoked by the framework on cache miss; on cache hit the
        // closure never runs.
        let var = variant.clone();
        let tn = tone.clone();
        let sz = size.clone();
        let sh = shape.clone();
        let compute = move || -> StyleRules {
            let theme = idea_theme::active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            // Button composes both axes: Size + Shape contribute
            // padding/font-size/border-radius via `modifier_defaults`;
            // Variant contributes the tone-driven skeleton (bg, color,
            // border) on top. Merge order = variant wins where they
            // overlap.
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &*tn,
            };
            modifier_defaults(&*sz, &*sh).merge(&var.render(&ctx))
        };

        // Base sheet carries the uniform Button properties (text
        // alignment, weight, letter spacing). The computed layer
        // provides the variant-driven properties on top.
        StyleApplication::new(ButtonSheet::sheet())
            .with_computed(cache_key.clone(), compute)
            // Font weight on Button is always SemiBold — uniform
            // across all variants. Set via override since the base
            // sheet already has it but we want to ensure it survives
            // the computed-layer merge (which doesn't touch weight).
            // No-op when the base already matches.
            .override_font_size(Tokenized::Literal(runtime_core::Length::Px(14.0)))
    };
    // Suppress unused-import warning for FontWeight — kept for future
    // when override_font_weight lands.
    let _ = FontWeight::SemiBold;

    let children: Vec<Primitive> = vec![text(label).into_primitive()];
    let on_click_for_p = on_click.clone();
    let mut bound = runtime_core::pressable(children, move || (on_click_for_p)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    bound.into_primitive()
}
