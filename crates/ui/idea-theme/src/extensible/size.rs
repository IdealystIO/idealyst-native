//! Built-in button sizes — three steps on the scale. Resolves through
//! the theme's `spacing-*` and `typography-*-size` tokens so light/dark
//! swap reflows naturally.
//!
//! Apps add custom sizes (e.g. `Xxxxs`, `Xl`) by implementing
//! [`super::ButtonSize`] on a marker struct.

use runtime_core::{Length, Tokenized};

use super::ButtonSize;

/// Small.
#[derive(Copy, Clone, Default)]
pub struct Sm;

impl ButtonSize for Sm {
    fn key(&self) -> &'static str {
        "sm"
    }
    fn padding_vertical(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-xs", Length::Px(4.0))
    }
    fn padding_horizontal(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-md", Length::Px(12.0))
    }
    fn font_size(&self) -> Tokenized<Length> {
        Tokenized::token("typography-body-sm-size", Length::Px(13.0))
    }
}

/// Medium — the default size.
#[derive(Copy, Clone, Default)]
pub struct Md;

impl ButtonSize for Md {
    fn key(&self) -> &'static str {
        "md"
    }
    fn padding_vertical(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-sm", Length::Px(8.0))
    }
    fn padding_horizontal(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-lg", Length::Px(16.0))
    }
    fn font_size(&self) -> Tokenized<Length> {
        Tokenized::token("typography-body-size", Length::Px(14.0))
    }
}

/// Large.
#[derive(Copy, Clone, Default)]
pub struct Lg;

impl ButtonSize for Lg {
    fn key(&self) -> &'static str {
        "lg"
    }
    fn padding_vertical(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-md", Length::Px(12.0))
    }
    fn padding_horizontal(&self) -> Tokenized<Length> {
        Tokenized::token("spacing-xl", Length::Px(24.0))
    }
    fn font_size(&self) -> Tokenized<Length> {
        Tokenized::token("typography-body-lg-size", Length::Px(18.0))
    }
}

// Reactive-prop coercion: `size = size::Md` into a `#[props]`-wrapped
// `Reactive<ButtonSizeRef>` field (marker → ref → Reactive can't go through
// one `.into()`; see typography.rs::builtin_kind!).
macro_rules! size_reactive_coercion {
    ($($name:ident),*) => { $(
        impl ::core::convert::From<$name> for ::runtime_core::Reactive<super::ButtonSizeRef> {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(super::ButtonSizeRef::from(marker))
            }
        }
    )* };
}
size_reactive_coercion!(Sm, Md, Lg);
