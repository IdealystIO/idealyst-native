//! Node transform composition.
//!
//! A framework node's final on-screen placement is the composition of
//! three things, in this order (outermost first):
//!
//! 1. **Layout position** — the Taffy frame origin (`x`, `y`) inside
//!    the parent, assigned in [`crate::LinuxBackend::finish`].
//! 2. **Static author transform** — the `transform: [...]` list from
//!    the node's stylesheet (`apply_style`).
//! 3. **Animated transform** — the per-frame `TranslateX/Y`, `Scale`,
//!    `RotateZ` writes from [`crate::LinuxBackend::set_animated_f32`].
//!
//! GTK's `GtkFixed` gives each child exactly **one** `GskTransform`
//! slot (the same slot `gtk_fixed_put` writes a translate into). So we
//! can't keep layout position and the author transform in separate
//! GTK properties the way UIKit keeps `frame.origin` and
//! `layer.transform` apart — we must fold all three into one
//! `GskTransform` and hand it to `Fixed::set_child_transform`. That is
//! what [`build_child_transform`] does.
//!
//! Static and animated components are stored *separately* on the node
//! (see [`NodeTransform`]) and only combined at build time, so a
//! restyle that rewrites the static transform can't stomp an in-flight
//! animation and vice-versa — the same separation the macOS backend
//! keeps in its `AnimatedState`.
//!
//! **Pivot**: scale + rotate always pivot around the node's geometric
//! center (transform-origin is center-only, matching the macOS
//! backend, which likewise doesn't honor `transform_origin`). GSK
//! transforms pivot at the local origin, so we bracket scale/rotate
//! with `translate(center)` / `translate(-center)`.

use gtk4::graphene;
use gtk4::gsk;
use runtime_core::{Length, Transform};

/// Folded author transform, resolved from a `Vec<Transform>`.
///
/// Translations keep their [`Length`] so a percent translate (the sun
/// glare wrapper's `translate(50%, -50%)`) can be resolved against the
/// node's *own* box size at build time — the box size isn't known when
/// `apply_style` runs (before layout), so we defer the percent→px
/// resolution to [`build_child_transform`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticTransform {
    pub translate_x: Length,
    pub translate_y: Length,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotate_deg: f32,
}

impl Default for StaticTransform {
    fn default() -> Self {
        Self {
            translate_x: Length::Px(0.0),
            translate_y: Length::Px(0.0),
            scale_x: 1.0,
            scale_y: 1.0,
            rotate_deg: 0.0,
        }
    }
}

/// Per-frame animated transform, written by `set_animated_f32`.
/// Translations are always device pixels here (the animation system
/// resolves percents author-side before calling the backend).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatedTransform {
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotate_deg: f32,
}

impl Default for AnimatedTransform {
    fn default() -> Self {
        Self {
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotate_deg: 0.0,
        }
    }
}

/// Both halves of a node's transform, stored together on the node and
/// combined only at build time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeTransform {
    pub statik: StaticTransform,
    pub animated: AnimatedTransform,
    /// `position: sticky` pin offset along Y, in px (see
    /// [`crate::sticky`]). Kept apart from `statik`/`animated` because
    /// it's derived from scroll state, not authored: a restyle or an
    /// in-flight animation must not clobber the pin, and the pin must
    /// not survive into either of theirs.
    pub sticky_dy: f32,
}

/// Fold a `transform: [...]` list into a single [`StaticTransform`].
///
/// Multiple entries of the same kind compose the CSS way: translates
/// add, scales multiply, rotations add. `Skew*` is dropped (not
/// representable in a `GskTransform` scale/rotate/translate chain — the
/// macOS backend drops it too). Percent translates are preserved as
/// [`Length::Percent`] for later resolution.
pub fn fold_static(transforms: &[Transform]) -> StaticTransform {
    let mut out = StaticTransform::default();
    let mut tx_px = 0.0f32;
    let mut ty_px = 0.0f32;
    let mut tx_pct = 0.0f32;
    let mut ty_pct = 0.0f32;
    for t in transforms {
        match *t {
            Transform::TranslateX(Length::Px(v)) => tx_px += v,
            Transform::TranslateX(Length::Percent(v)) => tx_pct += v,
            Transform::TranslateX(Length::Auto) => {}
            Transform::TranslateY(Length::Px(v)) => ty_px += v,
            Transform::TranslateY(Length::Percent(v)) => ty_pct += v,
            Transform::TranslateY(Length::Auto) => {}
            Transform::Scale(s) => {
                out.scale_x *= s;
                out.scale_y *= s;
            }
            Transform::ScaleXY { x, y } => {
                out.scale_x *= x;
                out.scale_y *= y;
            }
            Transform::Rotate(deg) => out.rotate_deg += deg,
            Transform::SkewX(_) | Transform::SkewY(_) => {}
        }
    }
    // A node can't mix px and percent on the same axis in practice
    // (welcome never does), but if it did we can only carry one
    // Length; prefer percent when present (the meaningful case here is
    // the sun-glare wrapper's pure `translate(50%, -50%)`), else px.
    out.translate_x = if tx_pct != 0.0 {
        Length::Percent(tx_pct)
    } else {
        Length::Px(tx_px)
    };
    out.translate_y = if ty_pct != 0.0 {
        Length::Percent(ty_pct)
    } else {
        Length::Px(ty_px)
    };
    out
}

/// Resolve a [`Length`] translate against a basis (the node's own
/// width for X, height for Y). `Auto` and percent-of-unknown collapse
/// to 0.
pub fn resolve_translate(len: Length, basis: f32) -> f32 {
    match len {
        Length::Px(v) => v,
        Length::Percent(p) => basis * p / 100.0,
        Length::Auto => 0.0,
    }
}

/// Effective translate on each axis (static + animated), with the
/// static percent resolved against `size`. Broken out for unit testing
/// without a GTK context.
pub fn effective_translate(nt: &NodeTransform, size: (f32, f32)) -> (f32, f32) {
    let sx = resolve_translate(nt.statik.translate_x, size.0);
    let sy = resolve_translate(nt.statik.translate_y, size.1);
    (sx + nt.animated.translate_x, sy + nt.animated.translate_y)
}

/// Effective scale on each axis: static × uniform-animated ×
/// per-axis-animated. `AnimProp::Scale` and `ScaleX/Y` compose
/// multiplicatively per the trait contract.
pub fn effective_scale(nt: &NodeTransform) -> (f32, f32) {
    (
        nt.statik.scale_x * nt.animated.scale * nt.animated.scale_x,
        nt.statik.scale_y * nt.animated.scale * nt.animated.scale_y,
    )
}

/// Build the combined `GskTransform` for a `GtkFixed` child.
///
/// `pos` is the Taffy frame origin inside the parent; `size` is the
/// node's own `(w, h)` (for percent-translate resolution + center
/// pivot). Returns identity-free translate when there's no transform
/// so unstyled nodes still get positioned.
pub fn build_child_transform(
    nt: &NodeTransform,
    pos: (f32, f32),
    size: (f32, f32),
) -> gsk::Transform {
    let (tx, ty) = effective_translate(nt, size);
    let (sx, sy) = effective_scale(nt);
    let rot = nt.statik.rotate_deg + nt.animated.rotate_deg;
    let (cx, cy) = (size.0 / 2.0, size.1 / 2.0);

    // Outermost: place the box at layout position + author/animated
    // translate. Then pivot to center, scale + rotate, pivot back.
    let mut xf = gsk::Transform::new().translate(&graphene::Point::new(
        pos.0 + tx,
        pos.1 + ty + nt.sticky_dy,
    ));

    let has_scale = (sx - 1.0).abs() > f32::EPSILON || (sy - 1.0).abs() > f32::EPSILON;
    let has_rot = rot.abs() > f32::EPSILON;
    if has_scale || has_rot {
        xf = xf.translate(&graphene::Point::new(cx, cy));
        if has_rot {
            xf = xf.rotate(rot);
        }
        if has_scale {
            xf = xf.scale(sx, sy);
        }
        xf = xf.translate(&graphene::Point::new(-cx, -cy));
    }
    xf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_add_scales_multiply_rotations_add() {
        let folded = fold_static(&[
            Transform::TranslateX(Length::Px(10.0)),
            Transform::TranslateX(Length::Px(5.0)),
            Transform::Scale(2.0),
            Transform::ScaleXY { x: 1.5, y: 1.0 },
            Transform::Rotate(30.0),
            Transform::Rotate(15.0),
        ]);
        assert_eq!(folded.translate_x, Length::Px(15.0));
        assert!((folded.scale_x - 3.0).abs() < 1e-4); // 2.0 * 1.5
        assert!((folded.scale_y - 2.0).abs() < 1e-4);
        assert!((folded.rotate_deg - 45.0).abs() < 1e-4);
    }

    #[test]
    fn skew_is_dropped() {
        let folded = fold_static(&[Transform::SkewX(20.0), Transform::Scale(2.0)]);
        assert!((folded.scale_x - 2.0).abs() < 1e-4);
    }

    #[test]
    fn sun_glare_wrapper_percent_translate_preserved_then_resolved() {
        // sun_glare.rs wrapper: translate(50%, -50%) pinning the disc
        // center to the top-right corner.
        let folded = fold_static(&[
            Transform::TranslateX(Length::Percent(50.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
        ]);
        assert_eq!(folded.translate_x, Length::Percent(50.0));
        let nt = NodeTransform {
            statik: folded,
            animated: AnimatedTransform::default(),
            ..Default::default()
        };
        // On a 200×200 box: +50% X = +100px, −50% Y = −100px.
        let (tx, ty) = effective_translate(&nt, (200.0, 200.0));
        assert!((tx - 100.0).abs() < 1e-4);
        assert!((ty + 100.0).abs() < 1e-4);
    }

    #[test]
    fn animated_scale_composes_with_static() {
        let nt = NodeTransform {
            statik: StaticTransform {
                scale_x: 2.0,
                scale_y: 2.0,
                ..Default::default()
            },
            animated: AnimatedTransform {
                scale: 1.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let (sx, sy) = effective_scale(&nt);
        assert!((sx - 3.0).abs() < 1e-4);
        assert!((sy - 3.0).abs() < 1e-4);
    }

    #[test]
    fn animated_translate_adds_to_resolved_static() {
        let nt = NodeTransform {
            statik: StaticTransform {
                translate_x: Length::Percent(50.0),
                ..Default::default()
            },
            animated: AnimatedTransform {
                translate_x: 12.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, _) = effective_translate(&nt, (100.0, 100.0));
        assert!((tx - 62.0).abs() < 1e-4); // 50% of 100 + 12
    }
}
