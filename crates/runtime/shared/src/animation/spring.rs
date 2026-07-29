//! Damped harmonic oscillator. Use when you want motion that
//! responds to the current state of the value — gestures handing
//! off into "settle to rest", overshoot/bounce, interruptible
//! motion.
//!
//! Use [`SpringTo`] at the call site:
//!
//! ```ignore
//! value.animate(SpringTo::new(target).stiffness(170).damping(26));
//! ```
//!
//! On handoff, springs inherit the value handle's current velocity
//! unless the author overrode it via
//! [`SpringTo::initial_velocity`]. This is what makes
//! drag→release→settle feel right — the thumb keeps moving in the
//! direction the finger threw it.
//!
//! # Integration
//!
//! Semi-implicit Euler: velocity is updated using the current
//! acceleration, then position is updated using the *new* velocity.
//! Stable across the spring-parameter ranges UI work uses (~10 to
//! ~2000 stiffness, ~5 to ~100 damping) without needing
//! sub-stepping.
//!
//! For pathologically stiff springs at low frame rates, the
//! [`MAX_FRAME_DT`](crate::animation::MAX_FRAME_DT) clamp on the
//! incoming slice keeps integration stable; the spring catches up
//! over the next few frames rather than blowing up in one step.

use std::time::Duration;

use super::animator::{Animator, AnimatorFactory, Sample, MAX_FRAME_DT};
use super::Animatable;

/// Default spring stiffness. 170 ≈ the React Spring / Framer Motion
/// "default" stiffness — produces ~500ms settling for 1.0 unit of
/// displacement at the default damping, which reads as snappy-but-
/// natural to users with web-animation muscle memory.
pub const DEFAULT_SPRING_STIFFNESS: f32 = 170.0;

/// Default spring damping. 26 sits *just* under critical damping
/// for stiffness 170 at mass 1.0 (`2 * sqrt(170) ≈ 26.08`), giving
/// a very small overshoot that reads as "alive" without obvious
/// bounce. Pair-locked with [`DEFAULT_SPRING_STIFFNESS`].
pub const DEFAULT_SPRING_DAMPING: f32 = 26.0;

/// Default spring mass. Authors rarely tune this — stiffness and
/// damping are the two knobs people reach for. We keep it at 1.0
/// so stiffness/damping numerics match the published references
/// from React Spring / Framer Motion / SwiftUI.
pub const DEFAULT_SPRING_MASS: f32 = 1.0;

/// Magnitude below which the displacement-from-target is considered
/// settled. Squared at construction; the per-frame test compares
/// `norm_sq` against the squared threshold to skip a `sqrt`.
pub const DEFAULT_SPRING_REST_DISPLACEMENT: f32 = 0.005;

/// Magnitude below which the velocity is considered settled.
pub const DEFAULT_SPRING_REST_VELOCITY: f32 = 0.01;

/// Floor on mass to avoid divide-by-zero in the acceleration step.
/// If the author passes `0.0` or a negative number we silently
/// clamp — better than panicking from a typo at the call site.
const MIN_MASS: f32 = 1e-3;

/// Author-facing factory for a spring animator.
#[derive(Clone)]
pub struct SpringTo<T: Animatable> {
    pub target: T,
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
    pub rest_displacement: f32,
    pub rest_velocity: f32,
    /// Override the seeded velocity. When `None`, the spring
    /// inherits the value handle's current velocity at handoff
    /// time. When `Some`, the supplied value wins — used for
    /// gesture-driven flows where the gesture itself measured a
    /// throw velocity that the framework doesn't know about.
    pub initial_velocity: Option<T>,
}

impl<T: Animatable> SpringTo<T> {
    pub fn new(target: T) -> Self {
        Self {
            target,
            stiffness: DEFAULT_SPRING_STIFFNESS,
            damping: DEFAULT_SPRING_DAMPING,
            mass: DEFAULT_SPRING_MASS,
            rest_displacement: DEFAULT_SPRING_REST_DISPLACEMENT,
            rest_velocity: DEFAULT_SPRING_REST_VELOCITY,
            initial_velocity: None,
        }
    }

    pub fn stiffness(mut self, stiffness: f32) -> Self {
        self.stiffness = stiffness;
        self
    }
    pub fn damping(mut self, damping: f32) -> Self {
        self.damping = damping;
        self
    }
    pub fn mass(mut self, mass: f32) -> Self {
        self.mass = mass;
        self
    }
    pub fn rest_displacement(mut self, rest_displacement: f32) -> Self {
        self.rest_displacement = rest_displacement;
        self
    }
    pub fn rest_velocity(mut self, rest_velocity: f32) -> Self {
        self.rest_velocity = rest_velocity;
        self
    }
    /// Override the velocity seeded into the spring. By default
    /// the spring uses the value handle's current velocity (which
    /// is what produces correct handoff). Use this when *you*
    /// know the throw velocity better than the framework does —
    /// the gesture system, an external simulation, etc.
    pub fn initial_velocity(mut self, initial_velocity: T) -> Self {
        self.initial_velocity = Some(initial_velocity);
        self
    }
}

impl<T: Animatable> AnimatorFactory<T> for SpringTo<T> {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        let seed_velocity = self.initial_velocity.unwrap_or(velocity);
        Box::new(Spring {
            target: self.target,
            current,
            velocity: seed_velocity,
            stiffness: self.stiffness,
            damping: self.damping,
            mass: self.mass.max(MIN_MASS),
            rest_displacement_sq: self.rest_displacement * self.rest_displacement,
            rest_velocity_sq: self.rest_velocity * self.rest_velocity,
        })
    }
}

/// The animator produced by [`SpringTo::build`].
///
/// Semi-implicit Euler over `(current, velocity)` toward `target`
/// with a Hooke's-law spring force and proportional damping.
pub struct Spring<T: Animatable> {
    target: T,
    current: T,
    velocity: T,
    stiffness: f32,
    damping: f32,
    mass: f32,
    rest_displacement_sq: f32,
    rest_velocity_sq: f32,
}

impl<T: Animatable> Animator<T> for Spring<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        let dt_s = dt.min(MAX_FRAME_DT).as_secs_f32();

        // displacement = current - target
        let displacement = T::sub(&self.current, &self.target);

        // accel = (-stiffness * displacement + -damping * velocity) / mass
        //
        // We fold the /mass into both factors at construction-time
        // semantics: spring_factor and damping_factor below carry
        // the sign and the mass divide together.
        let spring_factor = -self.stiffness / self.mass;
        let damping_factor = -self.damping / self.mass;

        // accel_from_spring = displacement * spring_factor
        let accel_from_spring =
            T::add_scaled(&T::zero(), &displacement, spring_factor);
        // accel = accel_from_spring + velocity * damping_factor
        let accel = T::add_scaled(&accel_from_spring, &self.velocity, damping_factor);

        // Semi-implicit Euler: integrate velocity first, then
        // position using the NEW velocity. Stable for stiff
        // springs at UI frame rates.
        self.velocity = T::add_scaled(&self.velocity, &accel, dt_s);
        self.current = T::add_scaled(&self.current, &self.velocity, dt_s);

        // Settled when both displacement and velocity are below
        // their (squared) thresholds. Snap to target on settle so
        // visual outputs land exactly on the requested value.
        let new_displacement = T::sub(&self.current, &self.target);
        let displacement_norm_sq = T::norm_sq(&new_displacement);
        let velocity_norm_sq = T::norm_sq(&self.velocity);

        let finished = displacement_norm_sq <= self.rest_displacement_sq
            && velocity_norm_sq <= self.rest_velocity_sq;

        if finished {
            self.current = self.target.clone();
            self.velocity = T::zero();
            return Sample {
                value: self.target.clone(),
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
    const MAX_FRAMES: usize = 1_000; // ~16 seconds — plenty.

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    /// Run a spring to settled and return (last value, last
    /// velocity, frame count). Asserts it settled within
    /// `MAX_FRAMES` to catch divergent integrations.
    fn run_to_settle(mut a: Box<dyn Animator<f32>>) -> (f32, f32, usize) {
        for n in 1..=MAX_FRAMES {
            let s = a.sample(STEP);
            if s.finished {
                return (s.value, s.velocity, n);
            }
        }
        panic!("spring did not settle within {} frames", MAX_FRAMES);
    }

    #[test]
    fn spring_settles_at_target() {
        let factory = SpringTo::new(1.0_f32);
        let a = factory.build(0.0, 0.0);
        let (value, velocity, _) = run_to_settle(a);
        assert!(approx_eq(value, 1.0, 1e-4));
        assert_eq!(velocity, 0.0);
    }

    #[test]
    fn spring_settles_with_initial_velocity() {
        // Throwing toward the target with extra velocity should
        // still settle there, just sooner (or with overshoot).
        let factory = SpringTo::new(1.0_f32);
        let a = factory.build(0.0, 5.0);
        let (value, _, _) = run_to_settle(a);
        assert!(approx_eq(value, 1.0, 1e-4));
    }

    #[test]
    fn spring_settles_with_velocity_away_from_target() {
        // Velocity initially pulling AWAY: spring should overcome
        // it and still settle at target. Verifies sign handling.
        let factory = SpringTo::new(1.0_f32);
        let a = factory.build(0.0, -3.0);
        let (value, _, _) = run_to_settle(a);
        assert!(approx_eq(value, 1.0, 1e-4));
    }

    #[test]
    fn stiffer_spring_settles_faster() {
        let soft = SpringTo::new(1.0_f32).stiffness(50.0).damping(15.0);
        let stiff = SpringTo::new(1.0_f32).stiffness(500.0).damping(45.0);

        let (_, _, soft_frames) = run_to_settle(soft.build(0.0, 0.0));
        let (_, _, stiff_frames) = run_to_settle(stiff.build(0.0, 0.0));
        assert!(
            stiff_frames < soft_frames,
            "stiff {} should settle before soft {}",
            stiff_frames,
            soft_frames
        );
    }

    #[test]
    fn handoff_preserves_velocity_by_default() {
        // A SpringTo with no explicit initial_velocity uses the
        // velocity argument supplied at build time. We verify by
        // measuring the very first sample's velocity: a spring
        // seeded with velocity > 0 will exit the first frame
        // moving toward the target.
        let factory = SpringTo::new(1.0_f32);
        let mut a = factory.build(0.0, 50.0);
        let s = a.sample(STEP);
        assert!(s.velocity > 0.0, "velocity was {}", s.velocity);
    }

    #[test]
    fn explicit_initial_velocity_overrides_handoff() {
        // If author supplies initial_velocity, the handoff
        // velocity is ignored.
        let factory = SpringTo::new(1.0_f32).initial_velocity(-10.0);
        let mut a = factory.build(0.0, 100.0); // pass big handoff velocity
        let s = a.sample(STEP);
        // First sample's velocity should reflect the -10 seed
        // (modulated by one step of spring force toward target).
        // It should be negative (still moving away initially).
        assert!(s.velocity < 0.0, "velocity was {}", s.velocity);
    }

    #[test]
    fn zero_mass_is_safe() {
        // Pathological input clamps to MIN_MASS rather than
        // dividing by zero.
        let factory = SpringTo::new(1.0_f32).mass(0.0);
        let mut a = factory.build(0.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.value.is_finite());
        assert!(s.velocity.is_finite());
    }

    #[test]
    fn settled_value_lands_exactly_on_target() {
        // After settle, sample should report `target` exactly,
        // not `target + tiny_drift`. We snap on settle to avoid
        // sub-pixel residue in backend property writes.
        let mut a = SpringTo::new(0.5_f32).build(0.0, 0.0);
        let mut last_value = 0.0_f32;
        for _ in 0..MAX_FRAMES {
            let s = a.sample(STEP);
            last_value = s.value;
            if s.finished {
                break;
            }
        }
        assert_eq!(last_value, 0.5);
    }

    #[test]
    fn finished_sample_is_idempotent() {
        let mut a = SpringTo::new(1.0_f32).build(0.0, 0.0);
        for _ in 0..MAX_FRAMES {
            if a.sample(STEP).finished {
                break;
            }
        }
        // Continuing to sample should keep returning settled.
        let s = a.sample(STEP);
        assert!(s.finished);
        assert_eq!(s.value, 1.0);
        assert_eq!(s.velocity, 0.0);
    }
}
