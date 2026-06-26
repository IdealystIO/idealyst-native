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

use runtime_core::{Color, StyleRules, Tokenized};

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
        _ => {
            let o = match state {
                InteractState::Hover => 0.92,
                InteractState::Press => 0.85,
            };
            StyleRules {
                opacity: Some(Tokenized::Literal(o)),
                ..Default::default()
            }
        }
    }
}

fn no_border() -> StyleRules {
    let zero = Tokenized::Literal(0.0);
    StyleRules {
        border_top_width: Some(zero.clone()),
        border_right_width: Some(zero.clone()),
        border_bottom_width: Some(zero.clone()),
        border_left_width: Some(zero),
        ..Default::default()
    }
}

/// Filled — solid background, contrasting text.
#[derive(Copy, Clone, Default)]
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

/// Soft — tinted background, intent-colored text.
#[derive(Copy, Clone, Default)]
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
#[derive(Copy, Clone, Default)]
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
#[derive(Copy, Clone, Default)]
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
variant_reactive_coercion!(Filled, Soft, Outlined, Ghost);
