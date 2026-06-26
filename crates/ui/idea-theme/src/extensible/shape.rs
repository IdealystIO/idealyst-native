//! Built-in shapes — corner-radius steps. Resolves through the theme's
//! `radius-*` tokens so themes that retune the radius scale flow
//! through every component using a shape.
//!
//! Apps add custom shapes (e.g. `Squircle`) by implementing
//! [`super::Shape`] on a marker struct.

use runtime_core::{Length, Tokenized};

use super::Shape;

/// Small radius — subtle corner softening.
#[derive(Copy, Clone, Default)]
pub struct Sm;

impl Shape for Sm {
    fn key(&self) -> &'static str {
        "sm"
    }
    fn border_radius(&self) -> Tokenized<Length> {
        Tokenized::token("radius-sm", Length::Px(4.0))
    }
}

/// Medium radius — the default.
#[derive(Copy, Clone, Default)]
pub struct Md;

impl Shape for Md {
    fn key(&self) -> &'static str {
        "md"
    }
    fn border_radius(&self) -> Tokenized<Length> {
        Tokenized::token("radius-md", Length::Px(8.0))
    }
}

/// Large radius — pronounced rounding.
#[derive(Copy, Clone, Default)]
pub struct Lg;

impl Shape for Lg {
    fn key(&self) -> &'static str {
        "lg"
    }
    fn border_radius(&self) -> Tokenized<Length> {
        Tokenized::token("radius-lg", Length::Px(12.0))
    }
}

/// Pill — fully rounded (clamped by the backend to half the shorter
/// dimension on platforms that don't support `999px` as
/// "use the full radius").
#[derive(Copy, Clone, Default)]
pub struct Pill;

impl Shape for Pill {
    fn key(&self) -> &'static str {
        "pill"
    }
    fn border_radius(&self) -> Tokenized<Length> {
        Tokenized::token("radius-pill", Length::Px(999.0))
    }
}

// Reactive-prop coercion: `shape = shape::Pill` into a `#[props]`-wrapped
// `Reactive<ShapeRef>` field (see typography.rs::builtin_kind!).
macro_rules! shape_reactive_coercion {
    ($($name:ident),*) => { $(
        impl ::core::convert::From<$name> for ::runtime_core::Reactive<super::ShapeRef> {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(super::ShapeRef::from(marker))
            }
        }
    )* };
}
shape_reactive_coercion!(Sm,Md,Lg,Pill);
