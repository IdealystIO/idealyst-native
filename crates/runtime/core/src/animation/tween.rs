//! Duration + curve based interpolation between two values.
//!
//! Use [`TweenTo`] at the call site:
//!
//! ```ignore
//! value.animate(TweenTo::new(target, Duration::from_millis(200)).ease_out());
//! ```
//!
//! Tweens are the "default" animation primitive — fixed start, fixed
//! end, fixed duration, eased. They do *not* preserve velocity on
//! handoff (the curve dictates motion); for that, use
//! [`SpringTo`](crate::animation::SpringTo).

use std::time::Duration;

use crate::style::Easing;

use super::animator::{Animator, AnimatorFactory, Sample};
use super::curve::apply_easing;
use super::Animatable;

/// Author-facing factory for a duration-based tween.
///
/// Constructed with `TweenTo::new(target, duration)`; the curve
/// defaults to `Easing::Ease` (CSS default — quick start, slow
/// end). Use the builder methods to set a different curve.
#[derive(Clone, Copy, Debug)]
pub struct TweenTo<T: Animatable> {
    pub target: T,
    pub duration: Duration,
    pub curve: Easing,
}

impl<T: Animatable> TweenTo<T> {
    pub fn new(target: T, duration: Duration) -> Self {
        Self {
            target,
            duration,
            curve: Easing::default(),
        }
    }

    pub fn curve(mut self, curve: Easing) -> Self {
        self.curve = curve;
        self
    }

    pub fn linear(self) -> Self {
        self.curve(Easing::Linear)
    }
    pub fn ease(self) -> Self {
        self.curve(Easing::Ease)
    }
    pub fn ease_in(self) -> Self {
        self.curve(Easing::EaseIn)
    }
    pub fn ease_out(self) -> Self {
        self.curve(Easing::EaseOut)
    }
    pub fn ease_in_out(self) -> Self {
        self.curve(Easing::EaseInOut)
    }
    pub fn cubic_bezier(self, x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        self.curve(Easing::CubicBezier(x1, y1, x2, y2))
    }
}

impl<T: Animatable> AnimatorFactory<T> for TweenTo<T> {
    fn build(self, current: T, _velocity: T) -> Box<dyn Animator<T>> {
        Box::new(Tween {
            from: current,
            to: self.target,
            duration: self.duration,
            elapsed: Duration::ZERO,
            curve: self.curve,
            last_value: None,
        })
    }
}

/// The animator produced by [`TweenTo::build`]. Owns the elapsed
/// clock and reports a finite-difference velocity (so handoff from
/// a tween into a spring still feels right).
pub struct Tween<T: Animatable> {
    from: T,
    to: T,
    duration: Duration,
    elapsed: Duration,
    curve: Easing,
    /// Previous sample's value, for velocity finite-difference.
    /// `None` on first sample → velocity reports zero.
    last_value: Option<T>,
}

impl<T: Animatable> Animator<T> for Tween<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        // Tweens do NOT clamp `dt`. The integrator only reads
        // `elapsed / duration`, which saturates against the
        // tween's duration on the next branch — so a 5-second
        // gap (sleep, tab background) correctly snaps to the
        // endpoint instead of catching up slowly. Spring/decay
        // *do* clamp because their integrators would explode on
        // a long slice; a tween has nothing to integrate.
        self.elapsed = self.elapsed.saturating_add(dt);

        // Zero-duration tweens snap to target on the first sample.
        if self.duration.is_zero() || self.elapsed >= self.duration {
            return Sample {
                value: self.to.clone(),
                velocity: T::zero(),
                finished: true,
            };
        }

        let t = self.elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let eased = apply_easing(t, self.curve);
        let value = T::lerp(&self.from, &self.to, eased);

        let velocity = match (&self.last_value, dt.as_secs_f32()) {
            (Some(prev), dts) if dts > 0.0 => {
                let delta = T::sub(&value, prev);
                T::add_scaled(&T::zero(), &delta, 1.0 / dts)
            }
            _ => T::zero(),
        };

        self.last_value = Some(value.clone());

        Sample {
            value,
            velocity,
            finished: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_millis(16);

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn linear_tween_hits_endpoints() {
        let factory = TweenTo::new(10.0_f32, Duration::from_millis(100)).linear();
        let mut a = factory.build(0.0, 0.0);

        // First sample at t=16ms → ~1.6
        let s = a.sample(STEP);
        assert!(!s.finished);
        assert!(approx_eq(s.value, 1.6, 0.01));

        // Drive to completion.
        let s = a.sample(Duration::from_millis(200));
        assert!(s.finished);
        assert!(approx_eq(s.value, 10.0, 1e-5));
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn zero_duration_snaps() {
        let mut a = TweenTo::new(42.0_f32, Duration::ZERO)
            .linear()
            .build(0.0, 0.0);
        let s = a.sample(Duration::from_millis(1));
        assert!(s.finished);
        assert_eq!(s.value, 42.0);
    }

    #[test]
    fn velocity_finite_difference_nonzero_mid_tween() {
        // Linear tween 0→10 over 100ms: velocity should be ~100/s.
        let mut a = TweenTo::new(10.0_f32, Duration::from_millis(100))
            .linear()
            .build(0.0, 0.0);
        let _ = a.sample(Duration::from_millis(20));
        let s = a.sample(Duration::from_millis(20));
        // Per-second velocity: ~100 units / s on a linear 0→10 / 100ms ramp.
        assert!(
            approx_eq(s.velocity, 100.0, 5.0),
            "velocity was {}",
            s.velocity
        );
    }

    #[test]
    fn ease_in_starts_slower_than_linear() {
        let linear = TweenTo::new(1.0_f32, Duration::from_millis(100)).linear();
        let ease_in = TweenTo::new(1.0_f32, Duration::from_millis(100)).ease_in();
        let mut la = linear.build(0.0, 0.0);
        let mut ea = ease_in.build(0.0, 0.0);
        let ls = la.sample(Duration::from_millis(25));
        let es = ea.sample(Duration::from_millis(25));
        assert!(
            es.value < ls.value,
            "ease_in {} should be < linear {}",
            es.value,
            ls.value
        );
    }

    #[test]
    fn finishes_idempotently_after_completion() {
        let mut a = TweenTo::new(1.0_f32, Duration::from_millis(50))
            .linear()
            .build(0.0, 0.0);
        let _ = a.sample(Duration::from_millis(100));
        // Subsequent samples must keep returning the settled value.
        let s = a.sample(Duration::from_millis(100));
        assert!(s.finished);
        assert_eq!(s.value, 1.0);
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn dt_clamping_doesnt_skip_endpoint() {
        // A 5-second slice should still drive a 100ms tween to its
        // endpoint — the *elapsed* clock saturates past duration,
        // not the slice itself.
        let mut a = TweenTo::new(1.0_f32, Duration::from_millis(100))
            .linear()
            .build(0.0, 0.0);
        let s = a.sample(Duration::from_secs(5));
        assert!(s.finished);
        assert_eq!(s.value, 1.0);
    }

    #[test]
    fn handoff_uses_current_as_start() {
        // Author says "tween to 1.0"; value handle hands off
        // current = 0.4. The built tween's `from` is 0.4, not the
        // factory's. Verified by the first non-zero sample's value
        // being between 0.4 and 1.0.
        let mut a = TweenTo::new(1.0_f32, Duration::from_millis(100))
            .linear()
            .build(0.4, 0.0);
        let s = a.sample(Duration::from_millis(50));
        assert!(s.value > 0.4 && s.value < 1.0, "value was {}", s.value);
    }
}
