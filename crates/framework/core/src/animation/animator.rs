//! The [`Animator`] trait: anything that can produce a sample of
//! `(value, velocity, finished)` given an elapsed-time slice.
//!
//! Each implementor *owns its own state* — a tween tracks its
//! elapsed time, a spring tracks its current value/velocity. The
//! framework's value handle ([`AnimatedValue`](crate::animation::AnimatedValue))
//! just stores the last sampled tuple so that when a new animator
//! is installed mid-flight ("handoff"), the factory can build the
//! new animator initialised with that state.
//!
//! See [`AnimatorFactory`] for the user-facing builders — the trait
//! callers actually compose with. Implementing `Animator` directly
//! is only needed for novel motion logic; for tweens, springs and
//! decays the canonical builders ([`TweenTo`](crate::animation::TweenTo),
//! [`SpringTo`](crate::animation::SpringTo),
//! [`DecayFrom`](crate::animation::DecayFrom)) handle it.

use std::time::Duration;

use super::Animatable;

/// One frame of an animator's output.
///
/// `value` is the new sampled value at the end of the slice.
/// `velocity` is the instantaneous rate of change in `T`-per-second
/// at the same moment — used for handoff when a new animator
/// replaces this one. `finished` is `true` once the animator has
/// no more motion to produce (tween elapsed past its duration,
/// spring settled within tolerance, decay's velocity dropped to
/// rest).
///
/// After `finished` is reported, the framework is allowed to drop
/// the animator. A subsequent `sample()` call on the same animator
/// is permitted but must remain idempotent — return the resting
/// value with zero velocity.
#[derive(Clone, Debug)]
pub struct Sample<T: Animatable> {
    pub value: T,
    pub velocity: T,
    pub finished: bool,
}

impl<T: Animatable> Sample<T> {
    /// A sample that has fully settled at `value` with no motion.
    /// Convenience for animators whose sample loop is past the end.
    pub fn settled(value: T) -> Self {
        Self {
            value,
            velocity: T::zero(),
            finished: true,
        }
    }
}

/// Per-frame motion source for a single value.
///
/// Implementors are owned by an
/// [`AnimatedValue`](crate::animation::AnimatedValue); the value
/// handle calls [`Animator::sample`] once per frame with the
/// elapsed slice and writes the result into its current/velocity
/// state.
///
/// `sample` is `&mut self` because most animators integrate state
/// across calls (elapsed time, spring position/velocity). This is
/// the contract that lets the public surface stay `!Send` and
/// stay compatible with the framework's `Rc<RefCell<…>>`
/// reactivity model.
pub trait Animator<T: Animatable>: 'static {
    /// Advance the animator by `dt` and return the new
    /// `(value, velocity, finished)`.
    ///
    /// `dt` is the wall-clock time since the previous `sample`
    /// call (or since the animator was built, for the first call).
    /// Implementors may clamp the slice — long pauses (system
    /// sleep, tab backgrounding) can pass huge `dt` values and a
    /// tween that ate a 10-second slice in one step would skip
    /// past its endpoint; spring integration would explode. The
    /// canonical implementations cap at
    /// [`MAX_FRAME_DT`].
    fn sample(&mut self, dt: Duration) -> Sample<T>;
}

/// Upper bound on a single integration slice. Animators that
/// integrate state over time (spring, decay) clamp incoming `dt`
/// to this value so a paused or backgrounded session that resumes
/// after seconds doesn't ship a single multi-second step (which
/// would either skip past a tween's endpoint or blow up a spring
/// integrator).
///
/// Sixty-four milliseconds matches roughly 4 frames at 60 Hz —
/// long enough that any legitimate dropped-frame slice still
/// advances correctly, short enough that a paused tab on resume
/// catches up at "normal" pace over the next several frames
/// rather than snapping.
pub const MAX_FRAME_DT: Duration = Duration::from_millis(64);

/// Build a concrete [`Animator`] given the current value and
/// velocity of the target.
///
/// Factories are what authors construct at the call site
/// (`TweenTo::new(target, duration).ease()`,
/// `SpringTo::new(target).stiffness(180).damping(20)`). The
/// framework converts them to an animator at the moment of
/// attachment, supplying the current sampled state of the value
/// — which is what gives us velocity-preserving handoff.
///
/// # Why this isn't just `From<…>`
///
/// `From` would force `current` and `velocity` to live on the
/// factory type. They don't logically belong there — the factory
/// describes the *intent* (a target + how to get there), not the
/// starting state. Splitting `intent` from `seed` lets a single
/// factory produce different animators at different attachment
/// times.
pub trait AnimatorFactory<T: Animatable> {
    /// Build the animator seeded with the value handle's current
    /// state.
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Constant<T: Animatable>(T);

    impl<T: Animatable> Animator<T> for Constant<T> {
        fn sample(&mut self, _dt: Duration) -> Sample<T> {
            Sample::settled(self.0.clone())
        }
    }

    #[test]
    fn settled_helper_zeroes_velocity() {
        let s = Sample::<f32>::settled(7.5);
        assert_eq!(s.value, 7.5);
        assert_eq!(s.velocity, 0.0);
        assert!(s.finished);
    }

    #[test]
    fn constant_animator_returns_settled() {
        let mut a = Constant(2.5_f32);
        let s = a.sample(Duration::from_millis(16));
        assert_eq!(s.value, 2.5);
        assert_eq!(s.velocity, 0.0);
        assert!(s.finished);
    }
}
