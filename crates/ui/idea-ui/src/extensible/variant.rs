//! Built-in variants — skeletons for button-like interactive surfaces.
//!
//! Four canonical forms:
//!
//! | Variant | Background | Border | Text |
//! |---|---|---|---|
//! | [`Filled`]  | tone's `fill_bg` | none | tone's `fill_fg` |
//! | [`Soft`]    | tinted fill_bg (alpha) | none | tone's `stroke_fg` |
//! | [`Outlined`]| transparent | tone's `stroke_color` (1px) | tone's `stroke_fg` |
//! | [`Ghost`]   | transparent | none | tone's `ghost_fg` |
//!
//! Each starts from `ctx.modifier_defaults()` (padding + font-size from
//! `size`, border-radius from `shape`) and overlays variant-specific
//! properties (`background`, `color`, optional border).

use runtime_core::{Color, StyleRules, Tokenized};

use super::{ResolutionCtx, Variant};

/// Filled — solid background, contrasting text.
#[derive(Copy, Clone, Default)]
pub struct Filled;

impl Variant for Filled {
    fn key(&self) -> &'static str {
        "filled"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = ctx.modifier_defaults();
        s.background = Some(ctx.tone.fill_bg(ctx.theme));
        s.color = Some(ctx.tone.fill_fg(ctx.theme));
        s
    }
}

/// Soft — tinted background, intent-colored text. Pulls from the
/// tone's `soft_bg`/`soft_fg` slots, which built-in tones map onto
/// the theme's soft palette (alpha-blended intent colors).
#[derive(Copy, Clone, Default)]
pub struct Soft;

impl Variant for Soft {
    fn key(&self) -> &'static str {
        "soft"
    }
    fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
        let mut s = ctx.modifier_defaults();
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
        let mut s = ctx.modifier_defaults();
        s.background = Some(Tokenized::Literal(Color("transparent".into())));
        s.color = Some(ctx.tone.stroke_fg(ctx.theme));
        let stroke = ctx.tone.stroke_color(ctx.theme);
        let one_px = Tokenized::Literal(1.0);
        s.border_top_width = Some(one_px.clone());
        s.border_right_width = Some(one_px.clone());
        s.border_bottom_width = Some(one_px.clone());
        s.border_left_width = Some(one_px);
        s.border_top_color = Some(stroke.clone());
        s.border_right_color = Some(stroke.clone());
        s.border_bottom_color = Some(stroke.clone());
        s.border_left_color = Some(stroke);
        s
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
        let mut s = ctx.modifier_defaults();
        s.background = Some(Tokenized::Literal(Color("transparent".into())));
        s.color = Some(ctx.tone.ghost_fg(ctx.theme));
        s
    }
}
