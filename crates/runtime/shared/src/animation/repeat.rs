//! Looping: replay a factory N times (or forever) with optional
//! autoreverse-like behaviour.
//!
//! ```ignore
//! // Pulse the scale until cancelled.
//! value.animate(LoopFactory::new(
//!     SequenceFactory::new()
//!         .then(TweenTo::new(1.1_f32, Duration::from_millis(120)).ease_out())
//!         .then(TweenTo::new(1.0_f32, Duration::from_millis(120)).ease_in()),
//!     Repeat::Forever,
//! ));
//! ```
//!
//! # Autoreverse
//!
//! Most "ping-pong" requirements compose naturally as a
//! [`SequenceFactory`] with two segments. The framework doesn't
//! provide an in-loop autoreverse flag because a factory has no
//! canonical "reverse" — a spring with target X has no
//! algorithmic mirror, and a tween's reverse depends on knowing
//! the original starting value. Authors who want ping-pong write
//! it explicitly:
//!
//! ```ignore
//! LoopFactory::new(
//!     SequenceFactory::new()
//!         .then(TweenTo::new(1.0, dur))
//!         .then(TweenTo::new(0.0, dur)),
//!     Repeat::Forever,
//! )
//! ```
//!
//! For a strict-reset loop (every iteration starts at the same
//! value), pair with [`SnapTo`](crate::animation::SnapTo) inside
//! the sequence.

use std::time::Duration;

use super::animator::{Animator, AnimatorFactory, Sample};
use super::combinators::ErasedFactory;
use super::Animatable;

/// How many times to replay the inner factory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Repeat {
    /// Loop a fixed number of times. `Times(0)` is a no-op (the
    /// loop animator immediately reports finished); `Times(1)`
    /// behaves identically to running the inner factory directly.
    Times(u32),
    /// Loop until cancelled.
    Forever,
}

impl Repeat {
    /// Whether the loop has more iterations to run, given how
    /// many have already completed.
    fn has_more(self, completed: u32) -> bool {
        match self {
            Repeat::Forever => true,
            Repeat::Times(total) => completed < total,
        }
    }
}

/// Author-facing factory: replay `inner` per [`Repeat`].
///
/// `inner` is preserved across iterations via [`ErasedFactory`],
/// so it must be `Clone + 'static`. Built-in factories satisfy
/// this via `#[derive(Clone)]`; nested [`SequenceFactory`]s do
/// too.
#[derive(Clone)]
pub struct LoopFactory<T: Animatable> {
    inner: ErasedFactory<T>,
    repeat: Repeat,
}

impl<T: Animatable> LoopFactory<T> {
    pub fn new<F: AnimatorFactory<T> + Clone + 'static>(inner: F, repeat: Repeat) -> Self {
        Self {
            inner: ErasedFactory::new(inner),
            repeat,
        }
    }

    /// Construct from an already-erased factory.
    pub fn from_erased(inner: ErasedFactory<T>, repeat: Repeat) -> Self {
        Self { inner, repeat }
    }
}

impl<T: Animatable> AnimatorFactory<T> for LoopFactory<T> {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        Box::new(LoopAnimator {
            template: self.inner,
            repeat: self.repeat,
            completed: 0,
            active: None,
            last_value: current,
            last_velocity: velocity,
        })
    }
}

/// The animator produced by [`LoopFactory::build`].
pub struct LoopAnimator<T: Animatable> {
    template: ErasedFactory<T>,
    repeat: Repeat,
    completed: u32,
    active: Option<Box<dyn Animator<T>>>,
    last_value: T,
    last_velocity: T,
}

impl<T: Animatable> Animator<T> for LoopAnimator<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        // Same per-frame advance pattern as `SequenceAnimator`:
        // if an iteration finishes mid-slice we can install the
        // next iteration and re-sample with the same `dt`. Cap to
        // prevent zero-duration inner factories from locking up.
        for _ in 0..super::sequence::MAX_SEGMENTS_PER_FRAME {
            if self.active.is_none() {
                if !self.repeat.has_more(self.completed) {
                    return Sample {
                        value: self.last_value.clone(),
                        velocity: T::zero(),
                        finished: true,
                    };
                }
                self.active = Some(self.template.instantiate(
                    self.last_value.clone(),
                    self.last_velocity.clone(),
                ));
            }

            let animator = self
                .active
                .as_mut()
                .expect("active is Some — just installed");
            let sample = animator.sample(dt);
            self.last_value = sample.value.clone();
            self.last_velocity = sample.velocity.clone();
            if !sample.finished {
                return sample;
            }
            // Iteration finished.
            self.active = None;
            self.completed = self.completed.saturating_add(1);
        }
        Sample {
            value: self.last_value.clone(),
            velocity: self.last_velocity.clone(),
            finished: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{SequenceFactory, SnapTo, TweenTo};

    const STEP: Duration = Duration::from_millis(16);
    const MAX_FRAMES: usize = 2_000;

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn times_zero_finishes_immediately() {
        let factory =
            LoopFactory::<f32>::new(TweenTo::new(1.0_f32, Duration::from_millis(50)), Repeat::Times(0));
        let mut a = factory.build(5.0, 0.0);
        let s = a.sample(STEP);
        assert!(s.finished);
        // Value is whatever the seed was — no iteration ran.
        assert_eq!(s.value, 5.0);
    }

    #[test]
    fn times_one_is_equivalent_to_running_inner() {
        let factory = LoopFactory::<f32>::new(
            TweenTo::new(1.0_f32, Duration::from_millis(50)).linear(),
            Repeat::Times(1),
        );
        let mut a = factory.build(0.0, 0.0);
        for _ in 0..MAX_FRAMES {
            if a.sample(STEP).finished {
                break;
            }
        }
        let s = a.sample(STEP);
        assert!(s.finished);
        assert!(approx_eq(s.value, 1.0, 1e-4));
    }

    #[test]
    fn times_n_runs_n_iterations() {
        // Each iteration: snap to 0, tween to 1. After 3
        // iterations the final value should be 1.0.
        let inner = SequenceFactory::<f32>::new()
            .then(SnapTo::new(0.0_f32))
            .then(TweenTo::new(1.0_f32, Duration::from_millis(30)).linear());
        let factory = LoopFactory::new(inner, Repeat::Times(3));
        let mut a = factory.build(0.0, 0.0);
        for _ in 0..MAX_FRAMES {
            if a.sample(STEP).finished {
                break;
            }
        }
        let s = a.sample(STEP);
        assert!(s.finished);
        assert!(approx_eq(s.value, 1.0, 1e-4));
    }

    #[test]
    fn forever_never_finishes_within_test_window() {
        let factory = LoopFactory::<f32>::new(
            TweenTo::new(1.0_f32, Duration::from_millis(30)).linear(),
            Repeat::Forever,
        );
        let mut a = factory.build(0.0, 0.0);
        let mut saw_unfinished = false;
        for _ in 0..200 {
            let s = a.sample(STEP);
            if !s.finished {
                saw_unfinished = true;
            }
        }
        assert!(saw_unfinished);
    }

    #[test]
    fn ping_pong_via_sequence_loop() {
        // Two-segment sequence inside a Repeat::Times(2) loop —
        // a→b then b→a, twice — ends at the starting value.
        let inner = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(30)).linear())
            .then(TweenTo::new(0.0_f32, Duration::from_millis(30)).linear());
        let factory = LoopFactory::new(inner, Repeat::Times(2));
        let mut a = factory.build(0.0, 0.0);
        for _ in 0..MAX_FRAMES {
            if a.sample(STEP).finished {
                break;
            }
        }
        let s = a.sample(STEP);
        assert!(s.finished);
        assert!(approx_eq(s.value, 0.0, 1e-4));
    }

    #[test]
    fn zero_duration_inner_does_not_lock_up() {
        // Inner produces SnapTo which finishes in 0 dt. Capped
        // at MAX_SEGMENTS_PER_FRAME so this frame returns
        // gracefully; the loop is finite (Times(1000)) so
        // ultimately finishes — but the test's real value is
        // that a single sample call returns and doesn't hang.
        let factory =
            LoopFactory::<f32>::new(SnapTo::new(3.0_f32), Repeat::Times(1000));
        let mut a = factory.build(0.0, 0.0);
        let _ = a.sample(STEP);
        // No assertion on completeness — just that we got here.
    }
}
