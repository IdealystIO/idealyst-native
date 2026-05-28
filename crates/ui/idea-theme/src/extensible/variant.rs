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
