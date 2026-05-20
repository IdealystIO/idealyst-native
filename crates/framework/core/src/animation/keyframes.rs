//! Multi-stop keyframe animation: linearly interpolate (with a
//! per-segment or shared curve) between any number of value
//! waypoints over a fixed total duration.
//!
//! ```ignore
//! // Bounce-in: shoot past target, settle.
//! value.animate(
//!     KeyframesTo::new(Duration::from_millis(400))
//!         .stop(0.0, 0.0)
//!         .stop(0.6, 1.1)
//!         .stop(1.0, 1.0)
//!         .curve(Easing::EaseOut)
//! );
//! ```
//!
//! Keyframe stops are `(offset, value)` pairs with `offset` in
//! `0..=1`. The first call to [`KeyframesTo::stop`] anchors the
//! starting point; the last anchors the end. Offsets must be
//! sorted ascending — out-of-order entries are caught at sample
//! time (we sort defensively rather than panicking, but the
//! intent is "feed them in order").
//!
//! # Why not segment-tweens?
//!
//! A `Sequence` of N tweens could express the same shape, but it
//! does so by computing per-segment durations from the offsets.
//! Keyframes is the more direct authoring shape — "I want the
//! value to be X at 60% of the way through" — and gives you a
//! single duration to tune. Authors who need different per-
//! segment behaviour (different curves) get them by chaining
//! `Sequence` instead.

use std::time::Duration;

use crate::style::Easing;

use super::animator::{Animator, AnimatorFactory, Sample};
use super::curve::apply_easing;
use super::Animatable;

/// Author-facing factory for a keyframe animation.
#[derive(Clone)]
pub struct KeyframesTo<T: Animatable> {
    pub duration: Duration,
    pub curve: Easing,
    /// `(offset, value)` pairs. Offsets in `0..=1`, sorted
    /// ascending. The factory's `build` sorts defensively before
    /// constructing the animator.
    pub stops: Vec<(f32, T)>,
}

impl<T: Animatable> KeyframesTo<T> {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            curve: Easing::default(),
            stops: Vec::new(),
        }
    }

    /// Add a `(offset, value)` stop. Offsets should be in
    /// ascending order; out-of-order calls are corrected at
    /// `build` time.
    pub fn stop(mut self, offset: f32, value: T) -> Self {
        self.stops.push((offset.clamp(0.0, 1.0), value));
        self
    }

    /// Set the easing curve applied to each segment's local
    /// `0..=1` time parameter. Default `Easing::Ease`.
    pub fn curve(mut self, curve: Easing) -> Self {
        self.curve = curve;
        self
    }
}

impl<T: Animatable> AnimatorFactory<T> for KeyframesTo<T> {
    fn build(mut self, current: T, _velocity: T) -> Box<dyn Animator<T>> {
        // Defensive sort — out-of-order stops yield surprising
        // visual jumps if we trust the order blindly. Stable
        // sort so equal-offset stops keep their insertion order
        // (last wins on a tie at sample time).
        self.stops.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Box::new(KeyframesAnimator {
            stops: self.stops,
            duration: self.duration,
            curve: self.curve,
            elapsed: Duration::ZERO,
            seed: current,
            last_value: None,
        })
    }
}

/// The animator produced by [`KeyframesTo::build`].
pub struct KeyframesAnimator<T: Animatable> {
    stops: Vec<(f32, T)>,
    duration: Duration,
    curve: Easing,
    elapsed: Duration,
    /// Value the value handle had at build time. Used as the
    /// implicit starting point when no stop is anchored at offset
    /// `0.0`, so a `KeyframesTo::new(d).stop(1.0, target)` reads
    /// as "tween from current value to target."
    seed: T,
    /// Previous sample's value, for finite-difference velocity.
    last_value: Option<T>,
}

impl<T: Animatable> Animator<T> for KeyframesAnimator<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        // Same dt policy as Tween: no clamping, elapsed
        // saturates at duration. A long sleep snaps to the last
        // stop rather than dragging us through the animation.
        self.elapsed = self.elapsed.saturating_add(dt);

        if self.duration.is_zero() {
            let value = self
                .stops
                .last()
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| self.seed.clone());
            return Sample {
                value,
                velocity: T::zero(),
                finished: true,
            };
        }

        let global_t = (self.elapsed.as_secs_f32() / self.duration.as_secs_f32()).min(1.0);
        let value = self.evaluate(global_t);

        let finished = self.elapsed >= self.duration;
        if finished {
            return Sample {
                value,
                velocity: T::zero(),
                finished: true,
            };
        }

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

impl<T: Animatable> KeyframesAnimator<T> {
    /// Evaluate the keyframe curve at global time `t` in `0..=1`.
    /// Implicit-start behaviour: if no stop anchors offset `0.0`
    /// the seed value fills that role. Likewise an implicit
    /// final stop at offset `1.0` is the last explicit stop's
    /// value, so an animation with stops at `0.3` and `0.7` will
    /// hold the `0.7` value through the tail.
    fn evaluate(&self, t: f32) -> T {
        if self.stops.is_empty() {
            return self.seed.clone();
        }

        // Implicit start at (0.0, seed) if no explicit stop is
        // at or before offset 0.
        let first = &self.stops[0];
        if t <= first.0 {
            if first.0 == 0.0 {
                return first.1.clone();
            }
            // Implicit (0.0, seed) → (first.0, first.1).
            let local_t = if first.0 == 0.0 {
                0.0
            } else {
                (t / first.0).clamp(0.0, 1.0)
            };
            let eased = apply_easing(local_t, self.curve);
            return T::lerp(&self.seed, &first.1, eased);
        }

        // Find the segment t falls into.
        for window in self.stops.windows(2) {
            let (lo_off, lo_val) = &window[0];
            let (hi_off, hi_val) = &window[1];
            if t <= *hi_off {
                let span = (hi_off - lo_off).max(f32::EPSILON);
                let local_t = ((t - lo_off) / span).clamp(0.0, 1.0);
                let eased = apply_easing(local_t, self.curve);
                return T::lerp(lo_val, hi_val, eased);
            }
        }

        // Past the last stop — hold its value.
        self.stops
            .last()
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| self.seed.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_millis(16);
    const MAX_FRAMES: usize = 1_000;

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    fn run_to_finish(mut a: Box<dyn Animator<f32>>) -> f32 {
        for _ in 0..MAX_FRAMES {
            let s = a.sample(STEP);
            if s.finished {
                return s.value;
            }
        }
        panic!("keyframes did not finish");
    }

    #[test]
    fn no_stops_holds_seed() {
        let factory: KeyframesTo<f32> = KeyframesTo::new(Duration::from_millis(100));
        let v = run_to_finish(factory.build(2.5, 0.0));
        assert_eq!(v, 2.5);
    }

    #[test]
    fn single_terminal_stop_tweens_from_seed() {
        let factory = KeyframesTo::<f32>::new(Duration::from_millis(100))
            .stop(1.0, 10.0)
            .curve(Easing::Linear);
        let v = run_to_finish(factory.build(0.0, 0.0));
        assert!(approx_eq(v, 10.0, 1e-4));
    }

    #[test]
    fn mid_stop_is_reached_at_offset() {
        // Stop at offset 0.5, value 5.0, terminal at offset 1.0,
        // value 0.0. At 50% elapsed the sample should be ~5.0.
        let factory = KeyframesTo::<f32>::new(Duration::from_millis(200))
            .stop(0.5, 5.0_f32)
            .stop(1.0, 0.0_f32)
            .curve(Easing::Linear);
        let mut a = factory.build(0.0, 0.0);
        // Sample at exactly 50% — drive 100ms across multiple
        // steps to avoid hitting exact-boundary edge cases.
        let mut s = a.sample(Duration::from_millis(100));
        // Two samples of 100ms - first sample lands at t=100ms = 50%.
        assert!(approx_eq(s.value, 5.0, 0.2), "value {} at 50%", s.value);
        // Drive to end.
        for _ in 0..20 {
            s = a.sample(STEP);
            if s.finished {
                break;
            }
        }
        assert!(approx_eq(s.value, 0.0, 1e-4));
    }

    #[test]
    fn unsorted_stops_get_sorted_at_build() {
        // Same end state regardless of insertion order.
        let factory = KeyframesTo::<f32>::new(Duration::from_millis(100))
            .stop(1.0, 10.0)
            .stop(0.0, 0.0)
            .stop(0.5, 5.0)
            .curve(Easing::Linear);
        let v = run_to_finish(factory.build(99.0, 0.0));
        assert!(approx_eq(v, 10.0, 1e-4));
    }

    #[test]
    fn zero_duration_snaps_to_last_stop() {
        let factory = KeyframesTo::<f32>::new(Duration::ZERO)
            .stop(0.0, 0.0)
            .stop(1.0, 7.0);
        let mut a = factory.build(0.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.finished);
        assert_eq!(s.value, 7.0);
    }

    #[test]
    fn velocity_finite_difference_nonzero_mid_curve() {
        let factory = KeyframesTo::<f32>::new(Duration::from_millis(200))
            .stop(0.0, 0.0_f32)
            .stop(1.0, 10.0_f32)
            .curve(Easing::Linear);
        let mut a = factory.build(0.0, 0.0);
        // First sample seeds last_value, second produces a
        // measurable velocity.
        let _ = a.sample(STEP);
        let s = a.sample(STEP);
        // Linear 0→10 over 200ms = 50 units/s.
        assert!(approx_eq(s.velocity, 50.0, 5.0), "velocity {}", s.velocity);
    }

    #[test]
    fn long_sleep_snaps_to_end() {
        let factory = KeyframesTo::<f32>::new(Duration::from_millis(100))
            .stop(1.0, 3.0_f32)
            .curve(Easing::Linear);
        let mut a = factory.build(0.0, 0.0);
        let s = a.sample(Duration::from_secs(10));
        assert!(s.finished);
        assert_eq!(s.value, 3.0);
    }
}
