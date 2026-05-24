//! Velocity-driven exponential decay. The "throw and rest" model
//! that backs flick-scroll, fling-pan, and toss-to-dismiss gestures.
//!
//! Use [`DecayFrom`] at the call site:
//!
//! ```ignore
//! // User releases a drag with measured velocity. The value drifts
//! // to rest on its own.
//! value.animate(DecayFrom::new(release_velocity));
//! ```
//!
//! Unlike [`SpringTo`](crate::animation::SpringTo) there is no
//! target — the resting value is wherever momentum carries the
//! value before friction wins. This is what makes "fling" gestures
//! feel right: the user's throw determines where it lands.
//!
//! # Integration
//!
//! Closed-form exponential decay, evaluated per step:
//!
//! ```text
//! v(t + dt) = v(t) * exp(-friction * dt)
//! x(t + dt) = x(t) + (v(t) - v(t + dt)) / friction
//! ```
//!
//! Exact (not Euler-approximated), so a 60ms slice produces the
//! same final state as four 16ms slices — frame-rate independent
//! by construction.

use std::time::Duration;

use super::animator::{Animator, AnimatorFactory, Sample, MAX_FRAME_DT};
use super::Animatable;

/// Default friction. ~3 inverse-seconds — velocity decays by ~`e`
/// per ~330ms, which produces a glide that feels "responsive but
/// natural" for the majority of UI drift effects. Authors tune
/// per use case (low friction for scroll fling, high friction for
/// short-tossed badges).
pub const DEFAULT_DECAY_FRICTION: f32 = 3.0;

/// Magnitude below which velocity is considered rest. Squared at
/// construction; per-frame test compares `norm_sq` against the
/// squared threshold to skip a `sqrt`.
pub const DEFAULT_DECAY_REST_VELOCITY: f32 = 0.01;

/// Floor on friction. Zero friction means the value would drift
/// forever — never settles, never finishes, holds the clock open.
/// We clamp at construction to a small positive value so the
/// animator always eventually reports `finished`.
const MIN_FRICTION: f32 = 1e-3;

/// Author-facing factory for a decay animator.
#[derive(Clone)]
pub struct DecayFrom<T: Animatable> {
    pub initial_velocity: T,
    pub friction: f32,
    pub rest_velocity: f32,
}

impl<T: Animatable> DecayFrom<T> {
    pub fn new(initial_velocity: T) -> Self {
        Self {
            initial_velocity,
            friction: DEFAULT_DECAY_FRICTION,
            rest_velocity: DEFAULT_DECAY_REST_VELOCITY,
        }
    }

    pub fn friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    pub fn rest_velocity(mut self, rest_velocity: f32) -> Self {
        self.rest_velocity = rest_velocity;
        self
    }
}

impl<T: Animatable> AnimatorFactory<T> for DecayFrom<T> {
    fn build(self, current: T, _velocity: T) -> Box<dyn Animator<T>> {
        Box::new(Decay {
            current,
            velocity: self.initial_velocity,
            friction: self.friction.max(MIN_FRICTION),
            rest_velocity_sq: self.rest_velocity * self.rest_velocity,
        })
    }
}

/// The animator produced by [`DecayFrom::build`].
pub struct Decay<T: Animatable> {
    current: T,
    velocity: T,
    friction: f32,
    rest_velocity_sq: f32,
}

impl<T: Animatable> Animator<T> for Decay<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        let dt_s = dt.min(MAX_FRAME_DT).as_secs_f32();

        // Closed-form step:
        //   v_new = v * exp(-friction * dt)
        //   x_new = x + (v - v_new) / friction
        let decay_factor = (-self.friction * dt_s).exp();

        // new_velocity = velocity * decay_factor
        let new_velocity = T::add_scaled(&T::zero(), &self.velocity, decay_factor);
        // velocity_delta = velocity - new_velocity
        let velocity_delta = T::sub(&self.velocity, &new_velocity);
        // current = current + velocity_delta / friction
        self.current = T::add_scaled(&self.current, &velocity_delta, 1.0 / self.friction);
        self.velocity = new_velocity;

        let velocity_norm_sq = T::norm_sq(&self.velocity);
        if velocity_norm_sq <= self.rest_velocity_sq {
            self.velocity = T::zero();
            return Sample {
                value: self.current.clone(),
                velocity: T::zero(),
                finished: true,
            };
        }

        Sample {
            value: self.current.clone(),
            velocity: self.velocity.clone(),
            finished: false,
        }
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

    fn run_to_rest(mut a: Box<dyn Animator<f32>>) -> (f32, usize) {
        for n in 1..=MAX_FRAMES {
            let s = a.sample(STEP);
            if s.finished {
                return (s.value, n);
            }
        }
        panic!("decay did not settle within {} frames", MAX_FRAMES);
    }

    #[test]
    fn zero_velocity_settles_immediately() {
        let mut a = DecayFrom::new(0.0_f32).build(5.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.finished);
        assert_eq!(s.value, 5.0);
    }

    #[test]
    fn positive_velocity_moves_positive() {
        let mut a = DecayFrom::new(10.0_f32).build(0.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.value > 0.0);
        assert!(s.velocity > 0.0);
        assert!(s.velocity < 10.0, "velocity must decrease each step");
    }

    #[test]
    fn negative_velocity_moves_negative() {
        let mut a = DecayFrom::new(-10.0_f32).build(0.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.value < 0.0);
        assert!(s.velocity < 0.0);
        assert!(s.velocity > -10.0);
    }

    #[test]
    fn higher_friction_settles_sooner() {
        let slow = DecayFrom::new(10.0_f32).friction(1.0);
        let fast = DecayFrom::new(10.0_f32).friction(10.0);
        let (_, slow_frames) = run_to_rest(slow.build(0.0, 0.0));
        let (_, fast_frames) = run_to_rest(fast.build(0.0, 0.0));
        assert!(
            fast_frames < slow_frames,
            "fast {} should settle before slow {}",
            fast_frames,
            slow_frames
        );
    }

    #[test]
    fn closed_form_is_dt_invariant() {
        // Two animators with the same initial conditions should
        // converge to the same final position regardless of frame
        // slice size — that's the closed-form guarantee.
        let coarse = DecayFrom::new(10.0_f32).build(0.0, 0.0);
        let fine = DecayFrom::new(10.0_f32).build(0.0, 0.0);

        let mut coarse = coarse;
        for _ in 0..MAX_FRAMES {
            if coarse.sample(Duration::from_millis(32)).finished {
                break;
            }
        }
        let mut fine = fine;
        for _ in 0..MAX_FRAMES {
            if fine.sample(Duration::from_millis(8)).finished {
                break;
            }
        }
        let coarse_final = coarse.sample(STEP).value;
        let fine_final = fine.sample(STEP).value;
        assert!(
            approx_eq(coarse_final, fine_final, 0.05),
            "coarse={} fine={}",
            coarse_final,
            fine_final
        );
    }

    #[test]
    fn zero_friction_does_not_panic() {
        // friction(0.0) would divide by zero; we clamp to
        // MIN_FRICTION at construction. The animator might not
        // *settle quickly* with tiny friction (its purpose is
        // safety, not pace), but each sample must remain finite
        // and produce a velocity that monotonically approaches
        // zero.
        let factory = DecayFrom::new(10.0_f32).friction(0.0);
        let mut a = factory.build(0.0, 0.0);
        let mut prev_velocity = 10.0_f32;
        for _ in 0..200 {
            let s = a.sample(STEP);
            assert!(s.value.is_finite());
            assert!(s.velocity.is_finite());
            // Velocity must be |v| ≤ |prev_v| each frame —
            // friction can only remove energy.
            assert!(s.velocity.abs() <= prev_velocity.abs() + 1e-3);
            prev_velocity = s.velocity;
        }
    }

    #[test]
    fn settled_sample_is_idempotent() {
        let mut a = DecayFrom::new(10.0_f32).build(0.0, 0.0);
        let mut last_value = 0.0_f32;
        for _ in 0..MAX_FRAMES {
            let s = a.sample(STEP);
            last_value = s.value;
            if s.finished {
                break;
            }
        }
        let s = a.sample(STEP);
        assert!(s.finished);
        assert_eq!(s.value, last_value);
        assert_eq!(s.velocity, 0.0);
    }
}
