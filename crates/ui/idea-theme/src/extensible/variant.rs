//! Built-in variants — pure skeletons for tone-aware surfaces. Each
//! sets ONLY tone-driven properties (background, text color, optional
//! border). Padding/font/border-radius are the component's job.
//!
//! | Variant | Background | Border | Text |
//! |---|---|---|---|
//! | [`Filled`]  | tone's `fill_bg` | none | tone's `fill_fg` |
//! | [`Soft`]    | tone's `soft_bg` (tinted) | none | tone's `soft_fg` |
//! | [`Outlined`]| transparent | tone's `stroke_color` (1px) | tone's `stroke_fg` |
//! | [`Ghost`]   | transparent | none | tone's `ghost_fg` |

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
        StyleRules {
            background: Some(ctx.tone.fill_bg(ctx.theme)),
            color: Some(ctx.tone.fill_fg(ctx.theme)),
            ..Default::default()
        }
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
        StyleRules {
            background: Some(ctx.tone.soft_bg(ctx.theme)),
            color: Some(ctx.tone.soft_fg(ctx.theme)),
            ..Default::default()
        }
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
        StyleRules {
            background: Some(Tokenized::Literal(Color("transparent".into()))),
            color: Some(ctx.tone.ghost_fg(ctx.theme)),
            ..Default::default()
        }
    }
}
