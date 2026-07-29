//! Small composable factories — [`Wait`] holds the value still for
//! a duration, [`SnapTo`] resets to a target instantly, plus the
//! [`ErasedFactory`] type-erasure wrapper that lets heterogeneous
//! factories sit in the same collection inside [`SequenceFactory`]
//! and [`LoopFactory`].
//!
//! These are the connective tissue of the composition layer: a
//! `Loop` over a `Sequence` of `Wait + TweenTo + Wait + TweenTo`
//! is a "pulse twice, pause, repeat" without any new primitives.

use std::rc::Rc;
use std::time::Duration;

use super::animator::{Animator, AnimatorFactory, Sample};
use super::sequence::SequenceFactory;
use super::value::AnimatedValue;
use super::Animatable;

// ---------------------------------------------------------------------------
// ErasedFactory — type-erased wrapper
// ---------------------------------------------------------------------------

/// Type-erased [`AnimatorFactory<T>`]. Stores the build logic
/// behind an `Rc<dyn Fn(T, T) -> Box<dyn Animator<T>>>` so:
///
/// 1. Heterogeneous factory types collapse to a single concrete
///    type — needed for `Vec<ErasedFactory<T>>` inside
///    [`SequenceFactory`].
/// 2. The wrapper is `Clone` (clones share the inner `Rc`), which
///    lets [`LoopFactory`] re-build the same logical sequence
///    every iteration without consuming it.
///
/// The wrapped factory must be `Clone + 'static` because
/// `ErasedFactory::build` is callable any number of times and
/// each call needs to construct a fresh underlying animator.
pub struct ErasedFactory<T: Animatable> {
    build_fn: Rc<dyn Fn(T, T) -> Box<dyn Animator<T>>>,
}

impl<T: Animatable> Clone for ErasedFactory<T> {
    fn clone(&self) -> Self {
        Self {
            build_fn: Rc::clone(&self.build_fn),
        }
    }
}

impl<T: Animatable> ErasedFactory<T> {
    /// Erase a concrete factory.
    pub fn new<F: AnimatorFactory<T> + Clone + 'static>(factory: F) -> Self {
        Self {
            build_fn: Rc::new(move |current, velocity| {
                factory.clone().build(current, velocity)
            }),
        }
    }

    /// Build an animator from the erased factory. Repeatable —
    /// `ErasedFactory` keeps the wrapped factory behind an `Rc`
    /// so each call clones it and produces a fresh animator.
    ///
    /// Distinct name from the trait's [`AnimatorFactory::build`]
    /// (which consumes `self`) so call sites disambiguate
    /// statically. Internal users (Sequence/Loop) reach for
    /// `instantiate`; the trait impl below routes consuming
    /// callers to the same logic.
    pub fn instantiate(&self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        (self.build_fn)(current, velocity)
    }
}

impl<T: Animatable> AnimatorFactory<T> for ErasedFactory<T> {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        self.instantiate(current, velocity)
    }
}

// ---------------------------------------------------------------------------
// Wait — hold value still for a duration
// ---------------------------------------------------------------------------

/// Factory: hold the current value still for `duration`, then
/// report `finished`. Composes inside [`SequenceFactory`] to
/// produce pauses, and inside `stagger` to produce per-item
/// delays.
#[derive(Clone, Copy, Debug)]
pub struct Wait {
    pub duration: Duration,
}

impl Wait {
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl<T: Animatable> AnimatorFactory<T> for Wait {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        Box::new(WaitAnimator {
            current,
            velocity,
            elapsed: Duration::ZERO,
            total: self.duration,
        })
    }
}

struct WaitAnimator<T: Animatable> {
    current: T,
    velocity: T,
    elapsed: Duration,
    total: Duration,
}

impl<T: Animatable> Animator<T> for WaitAnimator<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        // No clamping: Wait doesn't integrate anything; on long
        // slices we just report finished sooner. Velocity is
        // passed through unchanged so a Wait between two animated
        // segments doesn't kill momentum that the next animator
        // would inherit.
        self.elapsed = self.elapsed.saturating_add(dt);
        let finished = self.elapsed >= self.total;
        Sample {
            value: self.current.clone(),
            velocity: if finished {
                T::zero()
            } else {
                self.velocity.clone()
            },
            finished,
        }
    }
}

// ---------------------------------------------------------------------------
// SnapTo — instantly set value, finish
// ---------------------------------------------------------------------------

/// Factory: snap the value to `target` and finish immediately.
/// Useful inside [`SequenceFactory`] to reset state between
/// segments (e.g. for a non-autoreverse loop) and inside
/// [`LoopFactory`] when paired with a `then(SnapTo(start))` to
/// rewind position before the next iteration.
#[derive(Clone, Copy, Debug)]
pub struct SnapTo<T: Animatable> {
    pub target: T,
}

impl<T: Animatable> SnapTo<T> {
    pub fn new(target: T) -> Self {
        Self { target }
    }
}

impl<T: Animatable> AnimatorFactory<T> for SnapTo<T> {
    fn build(self, _current: T, _velocity: T) -> Box<dyn Animator<T>> {
        Box::new(SnapAnimator {
            target: Some(self.target),
        })
    }
}

struct SnapAnimator<T: Animatable> {
    /// `Some` on first sample, `None` after. We hold the target
    /// across calls so idempotent post-finish sampling returns
    /// the same value rather than `T::zero()`.
    target: Option<T>,
}

impl<T: Animatable> Animator<T> for SnapAnimator<T> {
    fn sample(&mut self, _dt: Duration) -> Sample<T> {
        // First call moves out of the option; subsequent calls
        // re-read the last-known target (we keep it cached for
        // idempotency, see below). The clock unregisters after
        // `finished: true` so in practice this only fires once,
        // but the animator contract demands idempotency.
        let value = self
            .target
            .clone()
            .expect("SnapTo target was cleared — impossible");
        // Don't clear `target`: stays cached so a post-finish
        // re-sample returns the same value rather than glitching.
        Sample {
            value,
            velocity: T::zero(),
            finished: true,
        }
    }
}

// ---------------------------------------------------------------------------
// stagger — apply per-index delay to a collection
// ---------------------------------------------------------------------------

/// Animate a collection of [`AnimatedValue`]s with a per-index
/// delay. Each value `i` receives a [`Wait`] of `step_delay * i`
/// followed by the factory produced by `factory_for(i)`. The
/// resulting animation is installed on each value via
/// [`AnimatedValue::animate`].
///
/// ```ignore
/// let scales: Vec<AnimatedValue<f32>> = items
///     .iter()
///     .map(|_| AnimatedValue::new(0.0))
///     .collect();
/// stagger(&scales, Duration::from_millis(40), |_i| {
///     SpringTo::new(1.0_f32).stiffness(220).damping(20)
/// });
/// ```
///
/// `factory_for` runs once per value and produces a fresh factory
/// — it can capture per-index state (different targets, different
/// curves) closed over by the caller.
pub fn stagger<T, FB, F>(
    values: &[AnimatedValue<T>],
    step_delay: Duration,
    mut factory_for: FB,
) where
    T: Animatable,
    FB: FnMut(usize) -> F,
    F: AnimatorFactory<T> + Clone + 'static,
{
    for (i, value) in values.iter().enumerate() {
        let factory = factory_for(i);
        let delay = step_delay
            .checked_mul(i as u32)
            .unwrap_or(Duration::ZERO);
        if delay.is_zero() {
            value.animate(factory);
        } else {
            value.animate(
                SequenceFactory::new()
                    .then(Wait::new(delay))
                    .then(factory),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::TweenTo;

    const STEP: Duration = Duration::from_millis(16);

    #[test]
    fn wait_holds_value_until_duration() {
        let factory: Wait = Wait::new(Duration::from_millis(50));
        let mut a = <Wait as AnimatorFactory<f32>>::build(factory, 3.0_f32, 0.0);
        let s = a.sample(STEP);
        assert_eq!(s.value, 3.0);
        assert!(!s.finished);
        let s = a.sample(Duration::from_millis(50));
        assert_eq!(s.value, 3.0);
        assert!(s.finished);
    }

    #[test]
    fn wait_passes_through_velocity() {
        let factory = Wait::new(Duration::from_millis(50));
        let mut a = <Wait as AnimatorFactory<f32>>::build(factory, 0.0, 42.0);
        let s = a.sample(STEP);
        assert_eq!(s.velocity, 42.0);
    }

    #[test]
    fn snap_to_returns_target_immediately() {
        let factory: SnapTo<f32> = SnapTo::new(7.5_f32);
        let mut a = <SnapTo<f32> as AnimatorFactory<f32>>::build(factory, 0.0, 99.0);
        let s = a.sample(STEP);
        assert_eq!(s.value, 7.5);
        assert_eq!(s.velocity, 0.0);
        assert!(s.finished);
    }

    #[test]
    fn snap_to_idempotent_after_first_sample() {
        let factory: SnapTo<f32> = SnapTo::new(2.0_f32);
        let mut a = <SnapTo<f32> as AnimatorFactory<f32>>::build(factory, 0.0, 0.0);
        let s1 = a.sample(STEP);
        let s2 = a.sample(STEP);
        assert_eq!(s1.value, s2.value);
        assert!(s1.finished && s2.finished);
    }

    #[test]
    fn erased_factory_rebuilds_independent_animators() {
        let ef = ErasedFactory::new(TweenTo::new(1.0_f32, Duration::from_millis(50)).linear());
        let mut a1 = ef.instantiate(0.0, 0.0);
        let mut a2 = ef.instantiate(5.0, 0.0);
        // Different starting points → different first samples.
        let s1 = a1.sample(STEP);
        let s2 = a2.sample(STEP);
        assert!(s1.value < s2.value, "{} vs {}", s1.value, s2.value);
    }

    #[test]
    fn stagger_offsets_per_index() {
        use crate::animation::clock::tick_for_test;

        let values: Vec<AnimatedValue<f32>> =
            (0..4).map(|_| AnimatedValue::new(0.0_f32)).collect();
        stagger(&values, Duration::from_millis(48), |_i| {
            TweenTo::new(1.0_f32, Duration::from_millis(48)).linear()
        });

        // After one frame: only value[0]'s Wait was zero-length,
        // so it started tweening immediately. value[1..] are
        // still waiting.
        tick_for_test(STEP);
        assert!(values[0].get() > 0.0, "value[0] should be progressing");
        for v in &values[1..] {
            assert_eq!(v.get(), 0.0, "later staggered values not yet started");
        }
    }
}
