//! Shared building blocks for idea-ui's tone-aware component styles.
//!
//! A tone-aware component (Button, Badge, Tag, Alert, …) declares `tone`
//! and `variant` axes on its slots and draws one compound per `(tone,
//! variant)` pair. These helpers are the `extend`-hook halves every one of
//! them shares — the surface fill, the text foreground — plus the prop →
//! axis plumbing for the shared `tone` palette.

use runtime_core::component_styles::ComponentStyles;
use runtime_core::{AxisValues, Reactive, SlotRestyle, StyleRules, Tokenized};

use idea_theme::extensible::{tone_variant_arms, with_idea_theme, ResolutionCtx, Tone, ToneRef};

use crate::modifiers::variant;

/// One compound per `(tone, built-in variant)` carrying the variant
/// recipe's full surface (fill, border, foreground).
pub(crate) fn surface_arms<S: ComponentStyles>(s: &mut SlotRestyle<'_, S>, tones: &[ToneRef]) {
    for (key, recipe) in variant::builtin_recipes() {
        tone_variant_arms(s, tones, key, recipe);
    }
}

/// One compound per `(tone, built-in variant)` carrying ONLY the recipe's
/// foreground `color` — for a text node inside a tone-aware surface.
/// Native text (`UILabel` / `TextView` / `NSTextField`) inherits nothing
/// from its container — only web's cascade does — so a label carries the
/// fill's foreground on its own node, as enumerated arms that premint.
pub(crate) fn foreground_arms<S: ComponentStyles>(s: &mut SlotRestyle<'_, S>, tones: &[ToneRef]) {
    for (key, recipe) in variant::builtin_recipes() {
        with_idea_theme(|theme| {
            for tone in tones {
                let ctx = ResolutionCtx { theme, tone: &*tone.0 };
                let color = recipe.render(&ctx).color;
                s.raw_compound(&[("tone", tone.key()), ("variant", key)], StyleRules { color, ..Default::default() });
            }
        });
    }
}

/// The themed focus ring a focusable tone-aware control draws: a border of
/// `width` in the theme's focus-ring color. The web `:focus` rule kills
/// the browser outline and macOS suppresses its native ring, so this is
/// the sole indicator.
pub(crate) fn focus_ring(width: f32) -> StyleRules {
    with_idea_theme(|theme| {
        use idea_theme::theme::IdeaTheme;
        let ring = theme.colors().focus_ring.clone();
        let w = Tokenized::Literal(width);
        StyleRules {
            border_top_width: Some(w.clone()),
            border_right_width: Some(w.clone()),
            border_bottom_width: Some(w.clone()),
            border_left_width: Some(w),
            border_top_color: Some(ring.clone()),
            border_right_color: Some(ring.clone()),
            border_bottom_color: Some(ring.clone()),
            border_left_color: Some(ring),
            ..Default::default()
        }
    })
}

/// Select the `tone` axis from a tone prop: a constant for a static tone,
/// a live read for a reactive one.
pub(crate) fn with_tone<S: ComponentStyles>(values: AxisValues<S>, tone: &Reactive<ToneRef>) -> AxisValues<S> {
    match tone.clone() {
        Reactive::Static(t) => values.raw("tone", t.key()),
        live => values.raw_with("tone", move || live.get().key()),
    }
}
