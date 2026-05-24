//! Easing curves shared by [`Tween`](crate::animation::Tween) and the
//! style-level transition system.
//!
//! `Curve` is a *parametric* easing — given a linear `0..=1` time
//! parameter it returns an eased `0..=1` value. It is intentionally
//! decoupled from physics-based animators like [`Spring`] and
//! [`Decay`] which compute their values directly from state and
//! never go through this curve table.
//!
//! The cubic-bezier solver here is the canonical UI-grade
//! Newton-Raphson approximation: a few iterations of inverse-solving
//! `curve_x(u) = t` for `u`, then evaluating `curve_y(u)`. Cheap
//! (sub-microsecond), accurate to well under a pixel for any UI
//! duration. The same routine backs the wgpu renderer's tween
//! engine — kept in one place here so all backends agree on what an
//! `Easing::Ease` actually looks like.

use crate::style::Easing;

/// Maximum iterations for the Newton-Raphson root-finder on the
/// inverse curve. Six is the empirical sweet spot for UI work:
/// accurate to far below a pixel for any animatable property, and
/// runs in well under a microsecond.
pub const BEZIER_NEWTON_ITERATIONS: usize = 6;

/// Below this derivative magnitude the Newton step is undefined —
/// fall out of the loop and accept the current `u`. Only ever hit
/// for degenerate curves (e.g. control points colinear with the
/// endpoints).
const BEZIER_DERIVATIVE_EPS: f32 = 1e-6;

/// Apply an [`Easing`] to a linear `0..=1` time parameter.
///
/// Values outside `0..=1` are clamped before evaluation. The named
/// curves use the standard CSS control-point sets so this matches
/// browser behaviour for the same name.
pub fn apply_easing(t: f32, easing: Easing) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::Ease => cubic_bezier_y(t, 0.25, 0.1, 0.25, 1.0),
        Easing::EaseIn => cubic_bezier_y(t, 0.42, 0.0, 1.0, 1.0),
        Easing::EaseOut => cubic_bezier_y(t, 0.0, 0.0, 0.58, 1.0),
        Easing::EaseInOut => cubic_bezier_y(t, 0.42, 0.0, 0.58, 1.0),
        Easing::CubicBezier(x1, y1, x2, y2) => cubic_bezier_y(t, x1, y1, x2, y2),
    }
}

/// Approximate `y` on a cubic Bezier curve `(0,0) → (x1,y1) →
/// (x2,y2) → (1,1)` at horizontal position `x = t`.
///
/// Standard UI implementation: a few Newton-Raphson iterations to
/// solve for the curve parameter that produces `x`, then evaluate
/// `y` at that parameter.
pub fn cubic_bezier_y(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let ax = 3.0 * x1 - 3.0 * x2 + 1.0;
    let bx = -6.0 * x1 + 3.0 * x2;
    let cx = 3.0 * x1;
    let curve_x = |u: f32| ((ax * u + bx) * u + cx) * u;
    let curve_dx = |u: f32| (3.0 * ax * u + 2.0 * bx) * u + cx;

    let ay = 3.0 * y1 - 3.0 * y2 + 1.0;
    let by = -6.0 * y1 + 3.0 * y2;
    let cy = 3.0 * y1;
    let curve_y = |u: f32| ((ay * u + by) * u + cy) * u;

    let mut u = x;
    for _ in 0..BEZIER_NEWTON_ITERATIONS {
        let cx_u = curve_x(u);
        let dx_u = curve_dx(u);
        if dx_u.abs() < BEZIER_DERIVATIVE_EPS {
            break;
        }
        u -= (cx_u - x) / dx_u;
        u = u.clamp(0.0, 1.0);
    }
    curve_y(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Curves must pass through (0,0) and (1,1) to be sane.
    const ENDPOINT_TOL: f32 = 1e-3;
    // Mid-evaluation tolerance is looser because Newton-Raphson is
    // an approximation; 1e-2 is well under a pixel for typical
    // values.
    const MID_TOL: f32 = 1e-2;

    #[test]
    fn linear_is_identity() {
        for t in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let y = apply_easing(t, Easing::Linear);
            assert!((y - t).abs() < ENDPOINT_TOL);
        }
    }

    #[test]
    fn named_curves_pass_through_endpoints() {
        for easing in [
            Easing::Ease,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            let y0 = apply_easing(0.0, easing);
            let y1 = apply_easing(1.0, easing);
            assert!(y0.abs() < ENDPOINT_TOL, "{:?}: y(0) = {}", easing, y0);
            assert!(
                (y1 - 1.0).abs() < ENDPOINT_TOL,
                "{:?}: y(1) = {}",
                easing,
                y1
            );
        }
    }

    #[test]
    fn cubic_bezier_custom_endpoints() {
        // Arbitrary control points still anchor at (0,0) and (1,1).
        let y0 = cubic_bezier_y(0.0, 0.1, 0.9, 0.9, 0.1);
        let y1 = cubic_bezier_y(1.0, 0.1, 0.9, 0.9, 0.1);
        assert!(y0.abs() < ENDPOINT_TOL);
        assert!((y1 - 1.0).abs() < ENDPOINT_TOL);
    }

    #[test]
    fn clamps_out_of_range() {
        let below = apply_easing(-0.5, Easing::Ease);
        let above = apply_easing(1.5, Easing::Ease);
        assert!(below.abs() < ENDPOINT_TOL);
        assert!((above - 1.0).abs() < ENDPOINT_TOL);
    }

    #[test]
    fn ease_in_starts_slow() {
        // EaseIn should produce y < t in the first half.
        let t = 0.25_f32;
        let y = apply_easing(t, Easing::EaseIn);
        assert!(y < t, "EaseIn at t=0.25 was {}, expected < 0.25", y);
    }

    #[test]
    fn ease_out_starts_fast() {
        // EaseOut should produce y > t in the first half.
        let t = 0.25_f32;
        let y = apply_easing(t, Easing::EaseOut);
        assert!(y > t, "EaseOut at t=0.25 was {}, expected > 0.25", y);
    }

    #[test]
    fn ease_in_out_symmetric_about_midpoint() {
        let lo = apply_easing(0.25, Easing::EaseInOut);
        let hi = apply_easing(0.75, Easing::EaseInOut);
        // Sum of symmetric points on EaseInOut should approximate 1.
        assert!(((lo + hi) - 1.0).abs() < MID_TOL, "lo={} hi={}", lo, hi);
    }
}
