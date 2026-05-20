//! Vocabulary of animatable properties shared across backends.
//!
//! [`AnimProp`] is the *contract* between the per-frame value
//! handles (`AnimatedValue<f32>`, `AnimatedValue<[f32; 4]>`, etc.)
//! and the platform-specific backend that ultimately writes to a
//! native widget. Each backend implements the
//! `Backend::set_animated_*` family for whichever properties it
//! supports natively; everything else is a documented no-op
//! (default trait impl) so author code remains portable.
//!
//! # Family split
//!
//! [`AnimProp`] is partitioned into scalar (`f32`) and color
//! (`[f32; 4]` sRGB) variants, mirroring the way most platforms
//! split their property-animator APIs. The variant determines
//! which `Backend::set_animated_*` method receives the value —
//! sending a `BackgroundColor` to `set_animated_f32` is a
//! compile-time non-issue (it's an enum) and a runtime no-op
//! (backends ignore unrecognized variants).
//!
//! # Why an enum instead of typed handles
//!
//! Backends can route a single `set_animated_f32(node, prop, v)`
//! method through one dispatch site (a `match` on the variant)
//! to dozens of property setters without one trait method per
//! property. Adding a new variant is one line in core + one match
//! arm per backend that wants to support it.

/// A single animatable property of a node. Backends interpret
/// each variant as a write to whatever native machinery exposes
/// that visual effect — `Opacity` is a CSS `opacity` style on
/// web, `UIView.alpha` on iOS, `View.alpha` on Android, etc.
///
/// Variants in the scalar family take an `f32` (typically in
/// `0..=1` for `Opacity`, in degrees for `RotateZ`, multiplicative
/// for `Scale*`, in DIPs for translates). The color family takes
/// sRGB `[r, g, b, a]` with channels in `0..=1`.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum AnimProp {
    // --- Scalar (f32) ---
    /// Compositing opacity. `0.0` = invisible, `1.0` = opaque.
    Opacity,
    /// Horizontal translate applied on top of layout position.
    /// Units: device-independent pixels. Positive moves right.
    TranslateX,
    /// Vertical translate applied on top of layout position.
    /// Units: DIPs. Positive moves down.
    TranslateY,
    /// Uniform scale factor. `1.0` = identity.
    Scale,
    /// Independent X scale factor. `1.0` = identity. Composes
    /// with [`AnimProp::Scale`] multiplicatively (`Scale * ScaleX`
    /// on the X axis).
    ScaleX,
    /// Independent Y scale factor. `1.0` = identity. Composes
    /// with [`AnimProp::Scale`] multiplicatively.
    ScaleY,
    /// Rotation around the Z axis, in degrees, clockwise.
    RotateZ,

    // --- Color ([f32; 4]) ---
    /// Background fill color. sRGB `[r, g, b, a]`.
    BackgroundColor,
    /// Foreground (text / icon stroke) color. sRGB `[r, g, b, a]`.
    ForegroundColor,
}

impl AnimProp {
    /// Whether this variant carries a scalar (`f32`) value. The
    /// complement is the color family. Used by backends that want
    /// to assert at debug time that the caller routed the right
    /// `set_animated_*` method.
    pub fn is_scalar(self) -> bool {
        matches!(
            self,
            AnimProp::Opacity
                | AnimProp::TranslateX
                | AnimProp::TranslateY
                | AnimProp::Scale
                | AnimProp::ScaleX
                | AnimProp::ScaleY
                | AnimProp::RotateZ
        )
    }

    /// Whether this variant carries a color (`[f32; 4]`) value.
    pub fn is_color(self) -> bool {
        !self.is_scalar()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_partition_is_exhaustive() {
        // Every variant must answer is_scalar XOR is_color.
        for prop in [
            AnimProp::Opacity,
            AnimProp::TranslateX,
            AnimProp::TranslateY,
            AnimProp::Scale,
            AnimProp::ScaleX,
            AnimProp::ScaleY,
            AnimProp::RotateZ,
            AnimProp::BackgroundColor,
            AnimProp::ForegroundColor,
        ] {
            assert!(prop.is_scalar() ^ prop.is_color(), "{:?}", prop);
        }
    }
}
