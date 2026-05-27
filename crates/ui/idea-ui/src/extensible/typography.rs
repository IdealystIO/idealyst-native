//! Built-in typography kinds. Each kind carries its full set of
//! per-variant characteristics (font size, weight, line height, letter
//! spacing) so apps that add a new kind specify everything in one
//! place — no implicit fallbacks.
//!
//! Apps add custom kinds (e.g. `SexySubtitle`, `BrandHeading`) by
//! implementing [`super::TypographyKind`] on a marker struct.

use runtime_core::{FontWeight, Length, Tokenized};

use super::TypographyKind;

macro_rules! builtin_kind {
    (
        $name:ident,
        key = $key:literal,
        size_token = $size_tok:literal,
        size_fallback = $size_fallback:expr,
        weight = $weight:expr,
        line_height = $lh:expr,
        letter_spacing = $ls:expr $(,)?
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

builtin_kind!(
    Display,
    key = "display",
    size_token = "typography-display-size",
    size_fallback = 56.0,
    weight = FontWeight::Bold,
    line_height = 1.05,
    letter_spacing = -1.5,
);
builtin_kind!(
    H1,
    key = "h1",
    size_token = "typography-h1-size",
    size_fallback = 36.0,
    weight = FontWeight::Bold,
    line_height = 1.15,
    letter_spacing = -0.6,
);
builtin_kind!(
    H2,
    key = "h2",
    size_token = "typography-h2-size",
    size_fallback = 28.0,
    weight = FontWeight::SemiBold,
    line_height = 1.2,
    letter_spacing = -0.3,
);
builtin_kind!(
    H3,
    key = "h3",
    size_token = "typography-h3-size",
    size_fallback = 20.0,
    weight = FontWeight::SemiBold,
    line_height = 1.3,
    letter_spacing = 0.0,
);
builtin_kind!(
    BodyXl,
    key = "body-xl",
    size_token = "typography-body-xl-size",
    size_fallback = 20.0,
    weight = FontWeight::Normal,
    line_height = 1.5,
    letter_spacing = 0.0,
);
builtin_kind!(
    BodyLg,
    key = "body-lg",
    size_token = "typography-body-lg-size",
    size_fallback = 18.0,
    weight = FontWeight::Normal,
    line_height = 1.5,
    letter_spacing = 0.0,
);
builtin_kind!(
    Body,
    key = "body",
    size_token = "typography-body-size",
    size_fallback = 14.0,
    weight = FontWeight::Normal,
    line_height = 1.45,
    letter_spacing = 0.0,
);
builtin_kind!(
    BodySm,
    key = "body-sm",
    size_token = "typography-body-sm-size",
    size_fallback = 13.0,
    weight = FontWeight::Normal,
    line_height = 1.4,
    letter_spacing = 0.0,
);
builtin_kind!(
    Caption,
    key = "caption",
    size_token = "typography-caption-size",
    size_fallback = 12.0,
    weight = FontWeight::Normal,
    line_height = 1.35,
    letter_spacing = 0.2,
);
builtin_kind!(
    Overline,
    key = "overline",
    size_token = "typography-overline-size",
    size_fallback = 11.0,
    weight = FontWeight::SemiBold,
    line_height = 1.3,
    letter_spacing = 1.2,
);
