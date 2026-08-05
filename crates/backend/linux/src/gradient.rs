//! Gradient resolution + GSK painting.
//!
//! A [`runtime_shared::style::Gradient`] is resolved once (in
//! `apply_style`) into a [`GradientPaint`] stored on the node's
//! [`crate::view::IdealystView`]. Storing the *resolved* stops (offset
//! + straight sRGB `[f32; 4]`) is what makes `GradientStopColor`
//! animation cheap: `set_animated_color` overwrites one stop's color
//! in place and calls `queue_draw`, with no re-parse of the author
//! `Gradient`. This is exactly the welcome scene's per-frame sun /
//! vignette pulse.
//!
//! GSK exposes native linear + radial gradient nodes
//! (`Snapshot::append_linear_gradient` / `append_radial_gradient`),
//! which map 1:1 onto the framework's two [`GradientKind`]s. The only
//! real work is geometry: turning a CSS angle into start/end points
//! (linear) and the `radius × extent` model into a pixel radius
//! (radial). Both are pure functions so they can be unit-tested
//! without a GTK context.

use gtk4::graphene;
use gtk4::gsk;
use gtk4::prelude::*;
use runtime_shared::{Gradient, GradientKind, RadialExtent};

use crate::color;

/// Resolved gradient kind — geometry only, colors live in
/// [`GradientPaint::stops`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradKind {
    Linear { angle_deg: f32 },
    Radial {
        center: (f32, f32),
        radius: f32,
        farthest: bool,
    },
}

/// A gradient resolved for painting. `stops` are `(offset, sRGB)` pairs
/// in ascending-offset order; the color half is mutated per-frame by
/// `GradientStopColor` animation.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientPaint {
    pub kind: GradKind,
    pub stops: Vec<(f32, [f32; 4])>,
}

/// Resolve an author [`Gradient`] into a [`GradientPaint`].
pub fn resolve(g: &Gradient) -> GradientPaint {
    let kind = match g.kind {
        GradientKind::Linear { angle_deg } => GradKind::Linear { angle_deg },
        GradientKind::Radial {
            center,
            radius,
            extent,
        } => GradKind::Radial {
            center,
            radius,
            farthest: matches!(extent, RadialExtent::FarthestCorner),
        },
    };
    let stops = g
        .stops
        .iter()
        .map(|s| (s.offset, color::to_srgb(&s.color)))
        .collect();
    GradientPaint { kind, stops }
}

/// Start/end points (in widget pixels) for a linear gradient.
///
/// CSS angle convention (matching [`GradientKind::Linear`]): `0°` =
/// bottom→top, `90°` = left→right, `180°` = top→bottom, `270°` =
/// right→left. Solved directly in GTK's y-down widget space, where the
/// unit direction from the `offset 0` end toward the `offset 1` end is
/// `(sin θ, −cos θ)`:
///   - θ=0 → (0, −1): end at top  ✓
///   - θ=90 → (1, 0):  end at right ✓
///   - θ=180 → (0, 1): end at bottom ✓
///
/// The gradient line is centered on the box; its half-length is the
/// axis-projected box extent so an axis-aligned band (every welcome
/// vignette band uses 0/90/180/270) fills its short dimension exactly.
pub fn linear_points(angle_deg: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
    let rad = angle_deg.to_radians();
    let dx = rad.sin();
    let dy = -rad.cos();
    let half = (dx.abs() * w + dy.abs() * h) / 2.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let start = (cx - dx * half, cy - dy * half);
    let end = (cx + dx * half, cy + dy * half);
    (start, end)
}

/// Pixel radius for a radial gradient's `offset 1.0` stop.
///
/// `ClosestSide` (`farthest == false`) references half the shorter box
/// side — the disc reaches the nearest edge midpoint at `radius: 1.0`
/// (welcome's suns/planets, all on aspect-ratio-1 boxes). `FarthestCorner`
/// references the distance from the center to the farthest corner —
/// what a screen-filling glow needs on a non-square box.
pub fn radial_radius(center: (f32, f32), radius: f32, farthest: bool, w: f32, h: f32) -> f32 {
    let reference = if farthest {
        let cx = center.0 * w;
        let cy = center.1 * h;
        let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
        corners
            .iter()
            .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
            .fold(0.0_f32, f32::max)
    } else {
        w.min(h) / 2.0
    };
    (reference * radius).max(0.0)
}

fn color_stops(stops: &[(f32, [f32; 4])]) -> Vec<gsk::ColorStop> {
    stops
        .iter()
        .map(|(offset, c)| gsk::ColorStop::new(*offset, color::to_gdk(*c)))
        .collect()
}

/// Append the gradient into `snapshot`, filling `(0, 0, w, h)`.
pub fn append(snapshot: &gtk4::Snapshot, w: f32, h: f32, paint: &GradientPaint) {
    if paint.stops.is_empty() || w <= 0.0 || h <= 0.0 {
        return;
    }
    let bounds = graphene::Rect::new(0.0, 0.0, w, h);
    let stops = color_stops(&paint.stops);
    match paint.kind {
        GradKind::Linear { angle_deg } => {
            let (start, end) = linear_points(angle_deg, w, h);
            snapshot.append_linear_gradient(
                &bounds,
                &graphene::Point::new(start.0, start.1),
                &graphene::Point::new(end.0, end.1),
                &stops,
            );
        }
        GradKind::Radial {
            center,
            radius,
            farthest,
        } => {
            let r = radial_radius(center, radius, farthest, w, h);
            let cp = graphene::Point::new(center.0 * w, center.1 * h);
            // start/end are the fractional radii the stop offsets map
            // between: 0.0 (center) .. 1.0 (the resolved radius `r`).
            snapshot.append_radial_gradient(&bounds, &cp, r, r, 0.0, 1.0, &stops);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::{Color, GradientStop};

    fn approx(a: (f32, f32), b: (f32, f32)) {
        assert!((a.0 - b.0).abs() < 1e-3, "x: {a:?} vs {b:?}");
        assert!((a.1 - b.1).abs() < 1e-3, "y: {a:?} vs {b:?}");
    }

    #[test]
    fn linear_0deg_is_bottom_to_top() {
        let (start, end) = linear_points(0.0, 100.0, 200.0);
        approx(start, (50.0, 200.0)); // bottom-center
        approx(end, (50.0, 0.0)); // top-center
    }

    #[test]
    fn linear_90deg_is_left_to_right() {
        let (start, end) = linear_points(90.0, 100.0, 200.0);
        approx(start, (0.0, 100.0));
        approx(end, (100.0, 100.0));
    }

    #[test]
    fn linear_180deg_is_top_to_bottom() {
        let (start, end) = linear_points(180.0, 100.0, 200.0);
        approx(start, (50.0, 0.0));
        approx(end, (50.0, 200.0));
    }

    #[test]
    fn radial_closest_side_is_half_shorter_side() {
        // 100×200 centered → half the shorter side = 50.
        let r = radial_radius((0.5, 0.5), 1.0, false, 100.0, 200.0);
        assert!((r - 50.0).abs() < 1e-3);
    }

    #[test]
    fn radial_farthest_corner_reaches_corner() {
        // 100×200 centered → dist to corner = √(50²+100²) ≈ 111.8.
        let r = radial_radius((0.5, 0.5), 1.0, true, 100.0, 200.0);
        assert!((r - 111.803).abs() < 1e-2);
    }

    #[test]
    fn radius_multiplier_scales_reference() {
        let r = radial_radius((0.5, 0.5), 2.0, false, 100.0, 100.0);
        assert!((r - 100.0).abs() < 1e-3); // 50 * 2
    }

    #[test]
    fn resolve_preserves_stop_order_and_alpha() {
        let g = Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: RadialExtent::ClosestSide,
            },
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color("#ffffff".into()),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color("#ffffff00".into()),
                },
            ],
        };
        let p = resolve(&g);
        assert_eq!(p.stops.len(), 2);
        assert_eq!(p.stops[0].0, 0.0);
        assert!((p.stops[0].1[3] - 1.0).abs() < 1e-3);
        assert!(p.stops[1].1[3].abs() < 1e-3); // outer stop transparent
        assert!(matches!(p.kind, GradKind::Radial { farthest: false, .. }));
    }
}
