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
    /// Z-order within the node's stacking context (i.e. against its
    /// sibling views). Higher values render in front of lower
    /// values. Lets a single view's z-position change at frame rate
    /// without re-mounting / re-ordering the tree — the use case is
    /// e.g. orbiting planets that pass in front of and behind a
    /// foreground element.
    ///
    /// Units: backend-defined ordering scalar. Within one backend
    /// the comparison is just `<`, so the absolute value doesn't
    /// matter — only the relative ordering. Web rounds to integer
    /// for `style.zIndex`; iOS writes a CGFloat to
    /// `layer.zPosition`; Android writes a float to
    /// `View.setTranslationZ` (in dp, converted to device px). All
    /// three are sibling-relative.
    ZIndex,

    // --- Color ([f32; 4]) ---
    /// Background fill color. sRGB `[r, g, b, a]`.
    BackgroundColor,
    /// Foreground (text / icon stroke) color. sRGB `[r, g, b, a]`.
    ForegroundColor,
    /// One stop in the node's `background_gradient`, indexed from
    /// `0` (innermost / first stop) up to `stops.len() - 1`. sRGB
    /// `[r, g, b, a]`. Updating one stop preserves the gradient's
    /// kind, center, radius, and all other stops — the backend
    /// rewrites only the targeted color and re-applies.
    ///
    /// Why a per-stop variant instead of a `transition` on the
    /// whole gradient: CSS `transition`s on gradients don't
    /// interpolate across browsers (web snaps, iOS interpolates
    /// natively, Android needs a per-frame ValueAnimator). The
    /// performance characteristics diverge wildly enough that
    /// hiding it behind one transition field would be a perf trap.
    /// `GradientStopColor` puts the per-frame cost at the call
    /// site — author drives it through an explicit
    /// `AnimatedValue<(f32,f32,f32,f32)>`, with the same shape and
    /// cadence as the other color AVs.
    GradientStopColor(u8),
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
                | AnimProp::ZIndex
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
            AnimProp::ZIndex,
            AnimProp::BackgroundColor,
            AnimProp::ForegroundColor,
            AnimProp::GradientStopColor(0),
        ] {
            assert!(prop.is_scalar() ^ prop.is_color(), "{:?}", prop);
        }
    }
}
