//! Built-in variants — pure skeletons for tone-aware surfaces.
//!
//! Variants set ONLY the properties they're responsible for — the
//! background/text/border slots. Padding, font-size, and border-radius
//! come from the Size/Shape modifiers. Variants also set border-widths
//! explicitly (to 0 for non-bordered variants) so the resolved
//! `StyleRules` content is fully specified — without this, the
//! framework's content-key hashing would differ between variant arms
//! that "have border" vs "don't mention border", breaking pregen
//! cache equality.
//!
//! | Variant | Background | Border | Text |
//! |---|---|---|---|
//! | [`Filled`]  | tone's `fill_bg` | none (width=0) | tone's `fill_fg` |
//! | [`Soft`]    | tone's `soft_bg` | none (width=0) | tone's `soft_fg` |
//! | [`Outlined`]| transparent | tone's `stroke_color` (1px) | tone's `stroke_fg` |
//! | [`Ghost`]   | transparent | none (width=0) | tone's `ghost_fg` |

use runtime_core::{Color, IdealystSchema, Shadow, StyleRules, Tokenized};

/// Blur of the hover/press glow a Filled surface casts. Geometry lives here,
/// the color in the theme's `Effects`, so one stylesheet serves every theme.
pub const GLOW_BLUR: f32 = 18.0;

/// How long a glow takes to grow in or fade out (`shadow_transition` on the
/// clickable sheets' base).
pub const GLOW_TRANSITION_MS: u32 = 180;

use super::{ResolutionCtx, Variant};

/// Which interaction a [`variant_state_overlay`] represents.
#[derive(Copy, Clone)]
pub enum InteractState {
    Hover,
    Press,
}

/// The hover/press feedback overlay for a given variant — the heart of the
/// "background-fill interactivity" upgrade.
///
/// A single global state overlay can't read correctly across all variants:
/// setting a `background` on a Filled button would *replace* its tone fill
/// (turning a brand-colored button grey on hover), because `background` is a
/// single replacing property. So the feedback is chosen per variant:
///
/// - **Ghost / Outlined** (transparent-resting): a translucent neutral
///   `background` fill (`theme.hover_overlay()` / `pressed_overlay()`). This is
///   the toolbar-button feel — for these variants the fill *is* the affordance.
/// - **Filled / Soft** (and any custom variant): a uniform `opacity` dim, which
///   never clobbers the tone fill. (Compositing a translucent layer over a
///   tone fill would need a real overlay layer; opacity is the faithful,
///   tone-preserving stand-in.)
///
/// Registered as per-`(tone, variant)` `compound`s in the sheet builders so the
/// overlay merges *after* the appearance arm and only on the active variant.
pub fn variant_state_overlay(
    variant_key: &str,
    ctx: &ResolutionCtx,
    state: InteractState,
) -> StyleRules {
    match variant_key {
        "ghost" | "outlined" => {
            let bg = match state {
                InteractState::Hover => ctx.theme.hover_overlay(),
                InteractState::Press => ctx.theme.pressed_overlay(),
            };
            StyleRules {
                background: Some(bg),
                ..Default::default()
            }
        }
        // filled, soft, and any custom variant: a tone-preserving opacity dim.
        // Filled and Gradient also cast the tone's glow (transparent unless
        // the theme lights it — see `Effects`). Only those two: their opaque
        // fill hides the shadow's interior on every backend, where a
        // translucent Soft fill would show it through on Apple/GPU but not on
        // web (`box-shadow` paints outside the border box only).
        _ => {
            let o = match state {
                InteractState::Hover => 0.92,
                InteractState::Press => 0.85,
            };
            let glow = matches!(variant_key, "filled" | "gradient")
                .then(|| ctx.tone.glow(ctx.theme))
                .flatten()
                .map(|color| Shadow { x: 0.0, y: 0.0, blur: GLOW_BLUR, color });
            StyleRules {
                opacity: Some(Tokenized::Literal(o)),
                shadow: glow,
                ..Default::default()
            }
        }
    }
}

/// No visible border: zero width, and a transparent colour. The colour
/// matters where a later layer reserves a width without naming a colour
/// (an interactive Chip reserves 1px for its focus ring, see
/// `tone_variant_sheet`). Without it the border would draw in the
/// platform default, `currentColor` on web.
pub fn no_border() -> StyleRules {
    let zero = Tokenized::Literal(0.0);
    let clear = Tokenized::Literal(Color("transparent".into()));
    StyleRules {
        border_top_width: Some(zero.clone()),
        border_right_width: Some(zero.clone()),
        border_bottom_width: Some(zero.clone()),
        border_left_width: Some(zero),
        border_top_color: Some(clear.clone()),
        border_right_color: Some(clear.clone()),
        border_bottom_color: Some(clear.clone()),
        border_left_color: Some(clear),
        ..Default::default()
    }
}

/// Filled — solid background, contrasting text.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "VariantRef", via = "idea_ui::variant")]
pub struct Filled;

impl Variant for Filled {
    fn key(&self) -> &'static str {
        "filled"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = no_border();
        s.background = Some(ctx.tone.fill_bg(ctx.theme));
        s.color = Some(ctx.tone.fill_fg(ctx.theme));
        s
    }
}

/// `Solid` — an alias for [`Filled`]. "Solid" is the instinctive name for a
/// solid-background button, so we accept it as a synonym rather than fail the
/// build. It's a `const` (not a `type` alias) so `variant::Solid` works in
/// value position (`variant = variant::Solid`); it *is* a `Filled`, so it
/// carries the same `"filled"` key and shares Filled's pre-generated styles.
#[allow(non_upper_case_globals)]
pub const Solid: Filled = Filled;

/// Soft — tinted background, intent-colored text.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "VariantRef", via = "idea_ui::variant")]
pub struct Soft;

impl Variant for Soft {
    fn key(&self) -> &'static str {
        "soft"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = no_border();
        s.background = Some(ctx.tone.soft_bg(ctx.theme));
        s.color = Some(ctx.tone.soft_fg(ctx.theme));
        s
    }
}

/// Outlined — transparent fill, intent-colored 1px border + text.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "VariantRef", via = "idea_ui::variant")]
pub struct Outlined;

impl Variant for Outlined {
    fn key(&self) -> &'static str {
        "outlined"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let stroke = ctx.tone.stroke_color(ctx.theme);
        let one_px = Tokenized::Literal(1.0);
        StyleRules {
            background: Some(Tokenized::Literal(Color("transparent".into()))),
            color: Some(ctx.tone.stroke_fg(ctx.theme)),
            border_top_width: Some(one_px.clone()),
            border_right_width: Some(one_px.clone()),
            border_bottom_width: Some(one_px.clone()),
            border_left_width: Some(one_px),
            border_top_color: Some(stroke.clone()),
            border_right_color: Some(stroke.clone()),
            border_bottom_color: Some(stroke.clone()),
            border_left_color: Some(stroke),
            ..Default::default()
        }
    }
}

/// Ghost — transparent fill, no border, intent-colored text only.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "VariantRef", via = "idea_ui::variant")]
pub struct Ghost;

impl Variant for Ghost {
    fn key(&self) -> &'static str {
        "ghost"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = no_border();
        s.background = Some(Tokenized::Literal(Color("transparent".into())));
        s.color = Some(ctx.tone.ghost_fg(ctx.theme));
        s
    }
}

/// Angle of the accent gradient a [`Gradient`] surface paints, CSS
/// convention (135° = top-left → bottom-right).
pub const GRADIENT_ANGLE_DEG: f32 = 135.0;

/// Gradient — the theme's accent gradient (`Effects::gradient_*`) as the
/// fill, its `gradient_text` on top. Tone-independent in color, but a
/// semantic tone still picks the hover glow.
///
/// Opt-in, not one of [`VariantRef::builtins`](super::VariantRef::builtins):
/// every built-in variant multiplies the appearance arms of every tone ×
/// variant sheet, so an app that wants it registers it on the builders it
/// uses — `ButtonSheetBuilder::new().add_variant(variant::Gradient)`.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "VariantRef", via = "idea_ui::variant")]
pub struct Gradient;

impl Variant for Gradient {
    fn key(&self) -> &'static str {
        "gradient"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let e = ctx.theme.effects();
        let mut s = no_border();
        // Solid fallback under the gradient: what shows if a backend can't
        // paint the gradient, and the color text contrast is checked against.
        s.background = Some(e.gradient_start.clone());
        s.background_gradient = Some(runtime_core::Gradient {
            kind: runtime_core::GradientKind::Linear { angle_deg: GRADIENT_ANGLE_DEG },
            stops: vec![
                runtime_core::GradientStop { offset: 0.0, color: e.gradient_start.clone() },
                runtime_core::GradientStop { offset: 0.5, color: e.gradient_mid.clone() },
                runtime_core::GradientStop { offset: 1.0, color: e.gradient_end.clone() },
            ],
        });
        s.color = Some(e.gradient_text.clone());
        s
    }
}

// Reactive-prop coercion for the built-in variants: lets a `ui!` call site
// pass a bare marker (`variant = variant::Filled`) to a `#[props]`-wrapped
// `Reactive<VariantRef>` field. The marker → ref → `Reactive` chain can't go
// through one `.into()`; see `extensible/typography.rs::builtin_kind!`.
macro_rules! variant_reactive_coercion {
    ($($name:ident),* $(,)?) => { $(
        impl ::core::convert::From<$name> for ::runtime_core::Reactive<super::VariantRef> {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(super::VariantRef::from(marker))
            }
        }
    )* };
}
variant_reactive_coercion!(Filled, Soft, Outlined, Ghost, Gradient);

// A remote bundle names these by key (see `crate::__remote_marker!`).
crate::__remote_marker!(super::VariantRef, Filled);
crate::__remote_marker!(super::VariantRef, Soft);
crate::__remote_marker!(super::VariantRef, Outlined);
crate::__remote_marker!(super::VariantRef, Ghost);
crate::__remote_marker!(super::VariantRef, Gradient);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensible::tone;
    use crate::theme::{light_theme, IdeaThemeRef};

    fn overlay(variant: &str, t: &dyn super::super::Tone, state: InteractState) -> StyleRules {
        let theme = IdeaThemeRef::new(light_theme());
        variant_state_overlay(variant, &ResolutionCtx { theme: &theme, tone: t }, state)
    }

    /// A Filled surface of a semantic tone glows on hover AND press (touch
    /// has no hover), with the intent's `effect-glow-*` TOKEN — never a baked
    /// color, or a build-time stylesheet could not follow a theme swap.
    #[test]
    fn filled_semantic_tone_glows_with_its_intent_token() {
        for state in [InteractState::Hover, InteractState::Press] {
            let sh = overlay("filled", &tone::Primary, state).shadow.expect("filled primary glows");
            assert_eq!(sh.color.name(), Some("effect-glow-primary"));
            assert_eq!((sh.x, sh.y, sh.blur), (0.0, 0.0, GLOW_BLUR));
        }
        let danger = overlay("filled", &tone::Danger, InteractState::Hover).shadow.unwrap();
        assert_eq!(danger.color.name(), Some("effect-glow-danger"));
    }

    /// The opt-in Gradient variant paints the theme's accent gradient by
    /// TOKEN (so it follows a swap) and glows like Filled.
    #[test]
    fn gradient_variant_paints_the_theme_accent_gradient() {
        let theme = IdeaThemeRef::new(light_theme());
        let rules = Gradient.render(&ResolutionCtx { theme: &theme, tone: &tone::Primary });
        let g = rules.background_gradient.expect("gradient fill");
        let names: Vec<_> = g.stops.iter().map(|s| s.color.name()).collect();
        assert_eq!(
            names,
            [Some("effect-gradient-start"), Some("effect-gradient-mid"), Some("effect-gradient-end")]
        );
        assert_eq!(rules.color.and_then(|c| c.name()), Some("effect-gradient-text"));
        assert!(overlay("gradient", &tone::Primary, InteractState::Hover).shadow.is_some());
    }

    /// Only Filled glows: a translucent Soft fill would show the shadow's
    /// interior on Apple/GPU but not on web. Palette hues have no glow slot.
    #[test]
    fn soft_ghost_and_palette_tones_cast_no_glow() {
        assert!(overlay("soft", &tone::Primary, InteractState::Hover).shadow.is_none());
        assert!(overlay("ghost", &tone::Primary, InteractState::Hover).shadow.is_none());
        assert!(overlay("outlined", &tone::Primary, InteractState::Hover).shadow.is_none());
        assert!(overlay("filled", &tone::Red, InteractState::Hover).shadow.is_none());
    }
}
