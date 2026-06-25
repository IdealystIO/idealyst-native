//! Built-in typography kinds. Each kind carries its full set of
//! per-variant characteristics (font size, weight, line height, letter
//! spacing) so apps that add a new kind specify everything in one
//! place — no implicit fallbacks.
//!
//! Apps add custom kinds (e.g. `SexySubtitle`, `BrandHeading`) by
//! implementing [`super::TypographyKind`] on a marker struct.
//!
//! **Units note.** `line_height` and `letter_spacing` are interpreted
//! as absolute pixels by the framework's web/native backends — these
//! values are *px*, not CSS unitless multipliers. Old `line_height:
//! 1.5` ratios would render as 1.5px and crush text lines on top of
//! each other.

use runtime_core::{FontWeight, Length, Tokenized};

use super::TypographyKind;

macro_rules! builtin_kind {
    (
        $name:ident,
        key = $key:literal,
        size_token = $size_tok:literal,
        size_fallback = $size_fallback:expr,
        weight = $weight:expr,
        line_height_px = $lh:expr,
        letter_spacing_px = $ls:expr $(,)?
    ) => {
        /// Built-in typography variant.
        #[derive(Copy, Clone, Default)]
        pub struct $name;

        impl TypographyKind for $name {
            fn key(&self) -> &'static str {
                $key
            }
            fn font_size(&self) -> Tokenized<Length> {
                Tokenized::token($size_tok, Length::Px($size_fallback))
            }
            fn font_weight(&self) -> FontWeight {
                $weight
            }
            fn line_height(&self) -> Tokenized<f32> {
                Tokenized::Literal($lh)
            }
            fn letter_spacing(&self) -> Tokenized<f32> {
                Tokenized::Literal($ls)
            }
        }
    };
}

// Px values mirror the original closed-enum Typography stylesheet so
// the visual output is identical to the pre-migration baseline.

// Sizes, line-heights, weights and letter-spacing mirror the idea-ui
// design type scale: a tight display/heading ramp (display 40 → h3 19)
// with negative tracking on the large roles, and generous body
// line-heights (~1.5–1.6×) for reading comfort. The `size_fallback`s
// match the theme's typography tokens so an un-themed surface still
// renders the canonical scale.
builtin_kind!(
    Display,
    key = "display",
    size_token = "typography-display-size",
    size_fallback = 40.0,
    weight = FontWeight::Bold,
    line_height_px = 44.0,
    letter_spacing_px = -1.0,
);
builtin_kind!(
    H1,
    key = "h1",
    size_token = "typography-h1-size",
    size_fallback = 32.0,
    weight = FontWeight::Bold,
    line_height_px = 36.0,
    letter_spacing_px = -0.8,
);
builtin_kind!(
    H2,
    key = "h2",
    size_token = "typography-h2-size",
    size_fallback = 24.0,
    weight = FontWeight::SemiBold,
    line_height_px = 28.0,
    letter_spacing_px = -0.5,
);
builtin_kind!(
    H3,
    key = "h3",
    size_token = "typography-h3-size",
    size_fallback = 19.0,
    weight = FontWeight::SemiBold,
    line_height_px = 24.0,
    letter_spacing_px = -0.2,
);
builtin_kind!(
    BodyXl,
    key = "body-xl",
    size_token = "typography-body-xl-size",
    size_fallback = 18.0,
    weight = FontWeight::Normal,
    line_height_px = 27.0,
    letter_spacing_px = 0.0,
);
builtin_kind!(
    BodyLg,
    key = "body-lg",
    size_token = "typography-body-lg-size",
    size_fallback = 16.0,
    weight = FontWeight::Normal,
    line_height_px = 25.0,
    letter_spacing_px = 0.0,
);
builtin_kind!(
    Body,
    key = "body",
    size_token = "typography-body-size",
    size_fallback = 14.0,
    weight = FontWeight::Normal,
    line_height_px = 22.0,
    letter_spacing_px = 0.0,
);
builtin_kind!(
    BodySm,
    key = "body-sm",
    size_token = "typography-body-sm-size",
    size_fallback = 13.0,
    weight = FontWeight::Normal,
    line_height_px = 21.0,
    letter_spacing_px = 0.0,
);
builtin_kind!(
    Caption,
    key = "caption",
    size_token = "typography-caption-size",
    size_fallback = 12.0,
    weight = FontWeight::Medium,
    line_height_px = 17.0,
    letter_spacing_px = 0.0,
);
builtin_kind!(
    Overline,
    key = "overline",
    size_token = "typography-overline-size",
    size_fallback = 11.0,
    weight = FontWeight::Bold,
    line_height_px = 14.0,
    letter_spacing_px = 1.5,
);
