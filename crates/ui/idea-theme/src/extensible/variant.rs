//! Helpers for tone-aware surface recipes ([`Variant`](super::Variant)
//! impls): the hover/press overlay a recipe's surface takes, the glow
//! geometry, and [`no_border`].
//!
//! A recipe sets ONLY the surface slots — background, text, border — and
//! sets border widths explicitly (0 where there is none), so every arm
//! fully specifies the border: a live switch from an outlined arm to a
//! filled one must clear the stroke, and a native backend leaves an unset
//! property as it was. idea-ui's `variant::{Filled, Soft, Outlined,
//! Ghost, Gradient}` are the built-in recipes.

use runtime_core::{Color, Shadow, StyleRules, Tokenized};

/// Blur of the hover/press glow a Filled surface casts. Geometry lives here,
/// the color in the theme's `Effects`, so one stylesheet serves every theme.
pub const GLOW_BLUR: f32 = 18.0;

/// How long a glow takes to grow in or fade out (`shadow_transition` on the
/// clickable sheets' base).
pub const GLOW_TRANSITION_MS: u32 = 180;

use super::ResolutionCtx;

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
/// Registered as per-`(tone, variant)` compounds by
/// [`tone_variant_state_overlays`](super::tone_variant_state_overlays), so
/// the overlay merges *after* the appearance arm and only on the active
/// variant.
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
/// (an interactive Chip reserves 1px for its focus ring). Without it the border would draw in the
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

/// Angle of the accent gradient a gradient surface paints, CSS
/// convention (135° = top-left → bottom-right).
pub const GRADIENT_ANGLE_DEG: f32 = 135.0;

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
