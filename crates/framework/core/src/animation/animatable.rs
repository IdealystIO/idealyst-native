//! The [`Animatable`] trait: anything that can be interpolated and
//! integrated by an [`Animator`](crate::animation::Animator).
//!
//! Two operations are load-bearing:
//!
//! - `sub(a, b) -> Self`         displacement (used by spring force +
//!                                tween delta)
//! - `add_scaled(base, d, k)`     integration step (used by tween
//!                                interpolation + spring velocity
//!                                integration)
//!
//! Everything else (lerp, settling check) is expressible on top.
//!
//! The trait deliberately models *values as their own delta type* —
//! `f32 - f32 = f32`, `(f32,f32) - (f32,f32) = (f32,f32)`. This keeps
//! the surface tiny; we don't need a separate `Delta` associated type
//! the way `nalgebra` does. For animation work it's correct: a colour
//! displacement is just another colour, a 2D-position displacement is
//! just another 2D position.
//!
//! `norm_sq` reports the squared magnitude — used by [`Spring`] to
//! decide when a value has settled. Squared (not magnitude) so impls
//! avoid a `sqrt` on every frame for every spring; the comparison
//! threshold is squared too.

/// A value type that can flow through the animation system.
///
/// Implement for any `T` you want to animate. Default impls live for
/// `f32`, fixed-size `f32` tuples (`(f32, f32)`, `(f32, f32, f32)`,
/// `(f32, f32, f32, f32)`) and `f32` arrays of the same arities.
///
/// # Required laws
///
/// - `add_scaled(a, zero(), k) == a`            (additive identity)
/// - `sub(a, a) == zero()`                       (self-displacement)
/// - `add_scaled(a, sub(b, a), 1.0) ≈ b`         (round-trip)
/// - `norm_sq(zero()) == 0.0`                    (zero settles)
/// - `norm_sq(x) >= 0.0`                          (non-negative)
///
/// These let the framework rely on a single algebraic shape across
/// all animators — interpolating, integrating, settling.
pub trait Animatable: Clone + 'static {
    /// Component-wise `base + delta * scale`.
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self;

    /// Component-wise `a - b`.
    fn sub(a: &Self, b: &Self) -> Self;

    /// Squared magnitude. Spring settling tests against squared
    /// threshold to avoid per-frame `sqrt`.
    fn norm_sq(value: &Self) -> f32;

    /// Additive identity.
    fn zero() -> Self;

    /// Linear interpolation. Default impl is correct for any
    /// well-behaved implementor; override only for performance or to
    /// change the interpolation space (e.g. colours in OKLCH).
    #[inline]
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let delta = Self::sub(b, a);
        Self::add_scaled(a, &delta, t)
    }
}

impl Animatable for f32 {
    #[inline]
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self {
        base + delta * scale
    }

    #[inline]
    fn sub(a: &Self, b: &Self) -> Self {
        a - b
    }

    #[inline]
    fn norm_sq(value: &Self) -> f32 {
        value * value
    }

    #[inline]
    fn zero() -> Self {
        0.0
    }

    #[inline]
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl Animatable for (f32, f32) {
    #[inline]
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self {
        (base.0 + delta.0 * scale, base.1 + delta.1 * scale)
    }

    #[inline]
    fn sub(a: &Self, b: &Self) -> Self {
        (a.0 - b.0, a.1 - b.1)
    }

    #[inline]
    fn norm_sq(value: &Self) -> f32 {
        value.0 * value.0 + value.1 * value.1
    }

    #[inline]
    fn zero() -> Self {
        (0.0, 0.0)
    }
}

impl Animatable for (f32, f32, f32) {
    #[inline]
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self {
        (
            base.0 + delta.0 * scale,
            base.1 + delta.1 * scale,
            base.2 + delta.2 * scale,
        )
    }

    #[inline]
    fn sub(a: &Self, b: &Self) -> Self {
        (a.0 - b.0, a.1 - b.1, a.2 - b.2)
    }

    #[inline]
    fn norm_sq(value: &Self) -> f32 {
        value.0 * value.0 + value.1 * value.1 + value.2 * value.2
    }

    #[inline]
    fn zero() -> Self {
        (0.0, 0.0, 0.0)
    }
}

impl Animatable for (f32, f32, f32, f32) {
    #[inline]
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self {
        (
            base.0 + delta.0 * scale,
            base.1 + delta.1 * scale,
            base.2 + delta.2 * scale,
            base.3 + delta.3 * scale,
        )
    }

    #[inline]
    fn sub(a: &Self, b: &Self) -> Self {
        (a.0 - b.0, a.1 - b.1, a.2 - b.2, a.3 - b.3)
    }

    #[inline]
    fn norm_sq(value: &Self) -> f32 {
        value.0 * value.0
            + value.1 * value.1
            + value.2 * value.2
            + value.3 * value.3
    }

    #[inline]
    fn zero() -> Self {
        (0.0, 0.0, 0.0, 0.0)
    }
}

/// Const-generic impl for `f32` arrays of any arity. Covers the
/// `[r, g, b, a]` shape the wgpu renderer uses for colors as well
/// as 2D/3D vector arrays.
impl<const N: usize> Animatable for [f32; N] {
    #[inline]
    fn add_scaled(base: &Self, delta: &Self, scale: f32) -> Self {
        std::array::from_fn(|i| base[i] + delta[i] * scale)
    }

    #[inline]
    fn sub(a: &Self, b: &Self) -> Self {
        std::array::from_fn(|i| a[i] - b[i])
    }

    #[inline]
    fn norm_sq(value: &Self) -> f32 {
        let mut s = 0.0;
        for v in value.iter() {
            s += v * v;
        }
        s
    }

    #[inline]
    fn zero() -> Self {
        [0.0; N]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn f32_round_trip() {
        // add_scaled(a, sub(b, a), 1.0) should approximate b.
        let a = 3.5_f32;
        let b = 7.25_f32;
        let d = f32::sub(&b, &a);
        let recovered = f32::add_scaled(&a, &d, 1.0);
        assert!(approx_eq(recovered, b));
    }

    #[test]
    fn f32_lerp_endpoints() {
        let a = 2.0_f32;
        let b = 6.0_f32;
        assert!(approx_eq(f32::lerp(&a, &b, 0.0), a));
        assert!(approx_eq(f32::lerp(&a, &b, 1.0), b));
        assert!(approx_eq(f32::lerp(&a, &b, 0.5), 4.0));
    }

    #[test]
    fn f32_zero_and_norm() {
        let z: f32 = f32::zero();
        assert!(approx_eq(z, 0.0));
        assert!(approx_eq(f32::norm_sq(&z), 0.0));
        assert!(approx_eq(f32::norm_sq(&3.0), 9.0));
    }

    #[test]
    fn tuple2_arithmetic() {
        let a = (1.0_f32, 2.0_f32);
        let b = (4.0_f32, 6.0_f32);
        let d = <(f32, f32) as Animatable>::sub(&b, &a);
        assert_eq!(d, (3.0, 4.0));
        let recovered = <(f32, f32) as Animatable>::add_scaled(&a, &d, 1.0);
        assert_eq!(recovered, b);
        let mid = <(f32, f32) as Animatable>::lerp(&a, &b, 0.5);
        assert!(approx_eq(mid.0, 2.5));
        assert!(approx_eq(mid.1, 4.0));
    }

    #[test]
    fn tuple4_norm() {
        let v = (1.0_f32, 2.0_f32, 2.0_f32, 0.0_f32);
        // 1 + 4 + 4 + 0 = 9
        assert!(approx_eq(<(f32, f32, f32, f32) as Animatable>::norm_sq(&v), 9.0));
    }

    #[test]
    fn array4_lerp() {
        let a: [f32; 4] = [0.0, 0.5, 1.0, 1.0];
        let b: [f32; 4] = [1.0, 0.5, 0.0, 1.0];
        let mid = <[f32; 4] as Animatable>::lerp(&a, &b, 0.5);
        assert!(approx_eq(mid[0], 0.5));
        assert!(approx_eq(mid[1], 0.5));
        assert!(approx_eq(mid[2], 0.5));
        assert!(approx_eq(mid[3], 1.0));
    }

    #[test]
    fn array_const_generic_norm() {
        let v: [f32; 3] = [3.0, 0.0, 4.0];
        // 9 + 0 + 16 = 25
        assert!(approx_eq(<[f32; 3] as Animatable>::norm_sq(&v), 25.0));
        let z: [f32; 3] = <[f32; 3] as Animatable>::zero();
        assert_eq!(z, [0.0, 0.0, 0.0]);
    }
}
