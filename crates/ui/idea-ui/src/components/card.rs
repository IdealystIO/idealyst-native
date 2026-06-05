//! `Card` — surface container, built on the extensible Variant trait.
//!
//! ```ignore
//! ui! {
//!     Card(variant = card::variant::Elevated, padding = CardPadding::Md) {
//!         Typography(content = "Stats", kind = typography_kind::H2)
//!     }
//! }
//! ```
//!
//! Two built-in variants: [`variant::Flat`] (surface bg) and
//! [`variant::Elevated`] (surface-alt bg + drop shadow). They read the
//! theme's surface colors directly — no intent palette — so they ignore
//! the `tone` field of `ResolutionCtx`.
//!
//! The Card stylesheet is built programmatically (variant × padding
//! axes) and installed lazily on first use. Apps with custom Card
//! variants install an extended sheet via [`install_card_sheet`]
//! before mounting.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    component, ui, ChildList, Easing, IdealystSchema, Length, Element, StyleApplication, StyleRules,
    StyleSheet, Tokenized, Transition, VariantEnum, VariantSet,
};

use idea_theme::active_theme;
use idea_theme::extensible::{tone as tones, ResolutionCtx, ToneRef, VariantRef};
use idea_theme::theme::IdeaThemeRef;

pub use crate::stylesheets::CardPadding;

/// Built-in Card variants. Card's variants don't consume a Tone (a
/// surface container isn't intent-colored) — they read the theme's
/// surface colors directly via `ctx.theme.colors()`.
pub mod variant {
    use idea_theme::extensible::{ResolutionCtx, Variant};
    use runtime_core::{Color, StyleRules};

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
    /// `surface`, distinct even on platforms that don't render shadows.
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
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                }),
                ..Default::default()
            }
        }
    }
}

thread_local! {
    static CARD_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

/// Install a custom Card stylesheet (e.g. with app-defined variants).
/// Call before the first Card mounts. If never called, the default
/// sheet (Flat + Elevated variants) is installed lazily on first use.
pub fn install_card_sheet(sheet: Rc<StyleSheet>) {
    CARD_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}

fn card_sheet() -> Rc<StyleSheet> {
    CARD_SHEET.with(|s| {
        if s.borrow().is_none() {
            let built = build_card_sheet(vec![variant::Flat.into(), variant::Elevated.into()]);
            *s.borrow_mut() = Some(built);
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Build a Card stylesheet from a list of variants. The padding axis
/// is fixed (none/sm/md/lg → theme spacing tokens). Each variant arm
/// pulls its background/shadow from `variant.render(ctx)` (Card
/// variants ignore the tone, so a placeholder Neutral is passed).
pub fn build_card_sheet(variants: Vec<VariantRef>) -> Rc<StyleSheet> {
    let radius = || Tokenized::token("radius-lg", Length::Px(12.0));

    let mut sheet = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        gap: Some(Tokenized::token("spacing-sm", Length::Px(8.0))),
        border_top_left_radius: Some(radius()),
        border_top_right_radius: Some(radius()),
        border_bottom_left_radius: Some(radius()),
        border_bottom_right_radius: Some(radius()),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(Tokenized::token(
            "color-border",
            runtime_core::Color("#e4e6ef".into()),
        )),
        border_right_color: Some(Tokenized::token(
            "color-border",
            runtime_core::Color("#e4e6ef".into()),
        )),
        border_bottom_color: Some(Tokenized::token(
            "color-border",
            runtime_core::Color("#e4e6ef".into()),
        )),
        border_left_color: Some(Tokenized::token(
            "color-border",
            runtime_core::Color("#e4e6ef".into()),
        )),
        background_transition: Some(Transition::new(250, Easing::EaseInOut)),
        color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        border_top_color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        ..Default::default()
    });

    for v in &variants {
        let v_c = v.clone();
        sheet = sheet.variant("variant", v.key(), move |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let neutral = tones::Neutral;
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &neutral,
            };
            v_c.0.render(&ctx)
        });
    }

    let pad = |tok: &'static str, px: f32| Tokenized::token(tok, Length::Px(px));
    sheet = sheet
        .variant("padding", "none", |_vs| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_left: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_right: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        })
        .variant("padding", "sm", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-sm", 8.0)),
            padding_bottom: Some(pad("spacing-sm", 8.0)),
            padding_left: Some(pad("spacing-sm", 8.0)),
            padding_right: Some(pad("spacing-sm", 8.0)),
            ..Default::default()
        })
        .variant("padding", "md", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-lg", 16.0)),
            padding_bottom: Some(pad("spacing-lg", 16.0)),
            padding_left: Some(pad("spacing-lg", 16.0)),
            padding_right: Some(pad("spacing-lg", 16.0)),
            ..Default::default()
        })
        .variant("padding", "lg", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-xl", 24.0)),
            padding_bottom: Some(pad("spacing-xl", 24.0)),
            padding_left: Some(pad("spacing-xl", 24.0)),
            padding_right: Some(pad("spacing-xl", 24.0)),
            ..Default::default()
        })
        .variant_default("variant", "flat")
        .variant_default("padding", "md");

    Rc::new(sheet)
}

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CardProps {
    /// Surface skeleton: built-in [`variant::Flat`] (page surface) or
    /// [`variant::Elevated`] (raised surface + shadow), or an
    /// app-installed custom variant. Default Flat.
    pub variant: VariantRef,
    /// Inner padding scale (None/Sm/Md/Lg → theme spacing tokens).
    /// Default Md.
    pub padding: CardPadding,
    /// Optional intent tint. When `Some`, the card paints a muted
    /// tone-tinted background and matching border (the same "Soft"
    /// treatment Alert uses) instead of the variant's surface color —
    /// for support/crisis/info panels that need to read as intent-colored.
    /// When `None` (the default), Flat/Elevated keep their surface look.
    pub tone: Option<ToneRef>,
    /// Card contents. Incoming fragments are flattened via
    /// `ChildList::append_to` before rendering inside the surface.
    pub children: Vec<Element>,
}

impl Default for CardProps {
    fn default() -> Self {
        Self {
            variant: variant::Flat.into(),
            padding: CardPadding::default(),
            tone: None,
            children: Vec::new(),
        }
    }
}

/// Surface container that wraps its children in a themed, bordered,
/// rounded panel. The `variant` picks the background/shadow treatment
/// and `padding` the inner spacing.
#[component(children)]
pub fn Card(props: CardProps) -> Element {
    let variant_key = props.variant.key().to_string();
    let padding_key = props.padding.as_variant_str().to_string();

    // Static style — build-time apply, no flicker (see Button).
    let mut style = StyleApplication::new(card_sheet())
        .with("variant", variant_key)
        .with("padding", padding_key);

    // Intent tint — when a tone is set, overlay the variant's surface
    // bg/border with the tone's Soft slots (the same tint Alert's Soft
    // variant uses). Rides a computed layer keyed on the tone so the
    // framework caches one resolved StyleRules per tone. Without a tone
    // the layer is absent and Flat/Elevated keep their surface look.
    if let Some(tone) = props.tone.clone() {
        let tone_for_key = tone.clone();
        style = style.with_computed(format!("tone_{}", tone_for_key.key()), move || {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let bg = tone.soft_bg(theme_ref);
            let border = tone.stroke_color(theme_ref);
            let fg = tone.soft_fg(theme_ref);
            StyleRules {
                background: Some(bg),
                color: Some(fg),
                border_top_color: Some(border.clone()),
                border_right_color: Some(border.clone()),
                border_bottom_color: Some(border.clone()),
                border_left_color: Some(border),
                ..Default::default()
            }
        });
    }

    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::extensible::{tone, Tone};
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleSource};

    fn theme() {
        install_idea_theme(light_theme());
    }

    fn view_style(card: Element) -> StyleApplication {
        match card {
            Element::View { style, .. } => match style.expect("Card view has a style") {
                StyleSource::Static(a) => a,
                _ => panic!("Card uses a static style source"),
            },
            _ => panic!("Card renders a view"),
        }
    }

    // D7: a toned Card paints the tone's Soft tint as its background,
    // distinct from the surface bg a tone-less Flat card renders.
    #[test]
    fn tone_tints_background_distinct_from_surface() {
        theme();
        let toned = CardProps {
            tone: Some(tone::Danger.into()),
            ..Default::default()
        };
        let toned_bg = resolve_style(&view_style(Card(toned)))
            .background
            .clone()
            .expect("toned card sets a background");

        let plain = CardProps::default();
        let plain_bg = resolve_style(&view_style(Card(plain)))
            .background
            .clone()
            .expect("Flat card sets a surface background");

        assert_ne!(
            toned_bg, plain_bg,
            "a Danger-toned card must read differently from a plain surface card"
        );
        // The tint matches the Danger tone's Soft slot (the same tint
        // Alert's Soft variant uses).
        let theme_rc = active_theme();
        let expected =
            tone::Danger.soft_bg(theme_rc.downcast_ref::<IdeaThemeRef>().unwrap());
        assert_eq!(toned_bg, expected, "tint is the tone's soft_bg");
    }

    // D7: with no tone, Flat/Elevated keep their surface look unchanged —
    // the computed tint layer is absent entirely.
    #[test]
    fn no_tone_keeps_surface_look() {
        theme();
        let plain = CardProps::default();
        let app = view_style(Card(plain));
        assert!(
            app.computed().is_none(),
            "a tone-less Card attaches no tint layer"
        );
    }
}
