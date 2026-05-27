//! `Card` — surface container, built on the extensible Variant trait.
//!
//! ```ignore
//! use idea_ui::extensible::card::{card, CardProps, variant};
//!
//! ui! {
//!     Card(variant = variant::Elevated, padding = CardPadding::Md) {
//!         Typography(content = "Stats", kind = typography::H2)
//!         Typography(content = "Today's activity")
//!     }
//! }
//! ```
//!
//! Two built-in variants: [`variant::Flat`] (surface bg, no shadow) and
//! [`variant::Elevated`] (surface-alt bg, drop shadow). Both use
//! [`IdeaTheme::colors`](idea_theme::theme::IdeaTheme::colors) directly
//! for the background — no intent palette is involved, so Card's
//! variants ignore the `tone` field of `ResolutionCtx`.
//!
//! Padding stays a closed enum (`CardPadding`) — it directly indexes
//! the theme's spacing scale.

use std::rc::Rc;

use runtime_core::{ui, ChildList, Primitive, StyleApplication, StyleRules};

use idea_theme::extensible::{tone as tones, ResolutionCtx, Variant};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::Card as CardSheet;
pub use crate::stylesheets::{CardPadding, CardTone};

/// Built-in Card variants. Card's variants don't consume a Tone (a
/// surface container isn't intent-colored) — they read the theme's
/// surface colors directly via `ctx.theme.colors()`.
pub mod variant {
    use runtime_core::{Color, StyleRules, Tokenized};

    use idea_theme::extensible::{ResolutionCtx, Variant};

    /// Flat — page-surface background, no shadow.
    #[derive(Copy, Clone, Default)]
    pub struct Flat;

    impl Variant for Flat {
        fn key(&self) -> &'static str {
            "flat"
        }
        fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
            StyleRules {
                background: Some(ctx.theme.colors().surface.clone()),
                ..Default::default()
            }
        }
    }

    /// Elevated — raised surface with a soft drop shadow. Uses
    /// `surface_alt` so the card reads as a layer above the page's
    /// `surface`, distinct even without the shadow on platforms that
    /// don't render shadows (terminal, paper).
    #[derive(Copy, Clone, Default)]
    pub struct Elevated;

    impl Variant for Elevated {
        fn key(&self) -> &'static str {
            "elevated"
        }
        fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
            StyleRules {
                background: Some(ctx.theme.colors().surface_alt.clone()),
                shadow: Some(runtime_core::Shadow {
                    x: 0.0,
                    y: 2.0,
                    blur: 8.0,
                    color: Color("rgba(0,0,0,0.10)".into()),
                }),
                // Tokenized literal-passthrough — the shadow's
                // displacement isn't tokenized today.
                ..Default::default()
            }
        }
    }

    // Suppress unused-import warning when this module compiles in
    // isolation (the macros aren't currently used here but the import
    // shape keeps with the other components' patterns).
    #[allow(dead_code)]
    fn _unused() {
        let _ = Tokenized::<f32>::Literal(0.0);
    }
}

pub struct CardProps {
    pub variant: Rc<dyn Variant>,
    pub padding: CardPadding,
    pub children: Vec<Primitive>,
}

impl Default for CardProps {
    fn default() -> Self {
        Self {
            variant: Rc::new(variant::Flat),
            padding: CardPadding::default(),
            children: Vec::new(),
        }
    }
}

fn padding_key(p: CardPadding) -> &'static str {
    use runtime_core::VariantEnum;
    p.as_variant_str()
}

pub fn card(props: CardProps) -> Primitive {
    let variant = props.variant.clone();
    let padding = props.padding;

    let cache_key = format!("card+{}+{}", variant.key(), padding_key(padding));

    let style = move || {
        let _ = idea_theme::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let var = variant.clone();
        let compute = move || -> StyleRules {
            let theme = idea_theme::active_theme();
            let theme_ref = theme
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            // Card's variants don't use tone — but ResolutionCtx
            // requires one. Pass Neutral as an unused placeholder.
            let neutral = tones::Neutral;
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &neutral,
            };
            var.render(&ctx)
        };
        // Re-use the existing Card stylesheet for its padding axis.
        // The computed layer overlays the variant's bg + shadow.
        StyleApplication::new(CardSheet::sheet())
            .with("padding", padding_key(padding).to_string())
            .with_computed(cache_key.clone(), compute)
    };

    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
