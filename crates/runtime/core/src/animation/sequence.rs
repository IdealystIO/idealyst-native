//! Sequencing: run a list of factories back-to-back.
//!
//! ```ignore
//! value.animate(
//!     SequenceFactory::new()
//!         .then(TweenTo::new(1.0, Duration::from_millis(150)).ease_out())
//!         .then(Wait::new(Duration::from_millis(200)))
//!         .then(SpringTo::new(0.5).stiffness(200))
//! );
//! ```
//!
//! Velocity flows across segment boundaries — when segment 1
//! finishes with some velocity, segment 2 is built with that
//! velocity, so a tween into a spring continues smoothly.
//!
//! # Boundary timing
//!
//! When a segment finishes mid-frame the [`SequenceAnimator`]
//! advances directly to the next segment and samples it with the
//! *same* `dt` slice. The "first frame of the next segment" gets
//! the full slice rather than the leftover portion — a small
//! over-advance, invisible at 60 Hz, and far simpler than
//! splitting the slice on a per-animator basis (which would
//! require each `Animator` to report leftover time, which springs
//! and decays can't precisely measure since they finish on a soft
//! settle threshold).
//!
//! The advance loop is capped at [`MAX_SEGMENTS_PER_FRAME`] so a
//! sequence of zero-duration segments can't lock up.

use std::collections::VecDeque;
use std::time::Duration;

use super::animator::{Animator, AnimatorFactory, Sample};
use super::combinators::ErasedFactory;
use super::Animatable;

/// Upper bound on segments advanced in a single `sample` call.
/// A sequence of N zero-duration `SnapTo` segments would
/// otherwise loop N times in one frame; capping at 64 keeps that
/// finite (and 64 is well above any realistic UI sequence depth).
pub const MAX_SEGMENTS_PER_FRAME: usize = 64;

/// Author-facing builder for a sequenced animation.
///
/// `Clone`-able so it can be nested inside [`LoopFactory`] (each
/// loop iteration clones the sequence's factory list, then drains
/// it).
pub struct SequenceFactory<T: Animatable> {
    factories: VecDeque<ErasedFactory<T>>,
}

impl<T: Animatable> Clone for SequenceFactory<T> {
    fn clone(&self) -> Self {
        Self {
            factories: self.factories.clone(),
        }
    }
}

impl<T: Animatable> Default for SequenceFactory<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Animatable> SequenceFactory<T> {
    pub fn new() -> Self {
        Self {
            factories: VecDeque::new(),
        }
    }

    /// Append a factory to the sequence. The factory must be
    /// `Clone + 'static` (see [`ErasedFactory`] for why) — most
    /// built-in factories satisfy this by `#[derive(Clone)]`.
    pub fn then<F: AnimatorFactory<T> + Clone + 'static>(mut self, factory: F) -> Self {
        self.factories.push_back(ErasedFactory::new(factory));
        self
    }

    /// Append an already-erased factory. Used by helpers that
    /// produce `ErasedFactory<T>` directly.
    pub fn then_erased(mut self, factory: ErasedFactory<T>) -> Self {
        self.factories.push_back(factory);
        self
    }
}

impl<T: Animatable> AnimatorFactory<T> for SequenceFactory<T> {
    fn build(self, current: T, velocity: T) -> Box<dyn Animator<T>> {
        Box::new(SequenceAnimator {
            pending: self.factories,
            active: None,
            last_value: current,
            last_velocity: velocity,
        })
    }
}

/// The animator produced by [`SequenceFactory::build`].
pub struct SequenceAnimator<T: Animatable> {
    pending: VecDeque<ErasedFactory<T>>,
    active: Option<Box<dyn Animator<T>>>,
    last_value: T,
    last_velocity: T,
}

impl<T: Animatable> Animator<T> for SequenceAnimator<T> {
    fn sample(&mut self, dt: Duration) -> Sample<T> {
        for _ in 0..MAX_SEGMENTS_PER_FRAME {
            if self.active.is_none() {
                // Need to start the next segment.
                let Some(factory) = self.pending.pop_front() else {
                    // Sequence done.
                    return Sample {
                        value: self.last_value.clone(),
                        velocity: T::zero(),
                        finished: true,
                    };
                };
                self.active = Some(factory.instantiate(
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
            // Segment finished — drop it and try the next.
            self.active = None;
        }
        // Cap hit. Return current state without `finished: true`
        // so the clock keeps ticking — we'll resume advancing on
        // the next frame.
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
    use crate::animation::{SnapTo, SpringTo, TweenTo, Wait};

    const STEP: Duration = Duration::from_millis(16);
    const MAX_FRAMES: usize = 2_000;

    fn approx_eq(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    fn run_to_finish(mut a: Box<dyn Animator<f32>>) -> (f32, usize) {
        for n in 1..=MAX_FRAMES {
            let s = a.sample(STEP);
            if s.finished {
                return (s.value, n);
            }
        }
        panic!("sequence did not finish within {} frames", MAX_FRAMES);
    }

    #[test]
    fn empty_sequence_finishes_immediately() {
        let seq: SequenceFactory<f32> = SequenceFactory::new();
        let mut a = seq.build(7.0, 0.0);
        let s = a.sample(STEP);
        assert_eq!(s.value, 7.0);
        assert!(s.finished);
    }

    #[test]
    fn single_segment_sequence_runs_to_completion() {
        let seq = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(100)).linear());
        let (value, _) = run_to_finish(seq.build(0.0, 0.0));
        assert!(approx_eq(value, 1.0, 1e-4));
    }

    #[test]
    fn multi_segment_sequence_chains_values() {
        let seq = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(50)).linear())
            .then(TweenTo::new(0.5_f32, Duration::from_millis(50)).linear());
        let (value, _) = run_to_finish(seq.build(0.0, 0.0));
        assert!(approx_eq(value, 0.5, 1e-4));
    }

    #[test]
    fn wait_segment_holds_then_continues() {
        let seq = SequenceFactory::<f32>::new()
            .then(Wait::new(Duration::from_millis(40)))
            .then(TweenTo::new(2.0_f32, Duration::from_millis(50)).linear());
        let mut a = seq.build(0.0, 0.0);
        // During the wait segment the value stays at 0.
        let s = a.sample(STEP);
        assert_eq!(s.value, 0.0);
        let (final_value, _) = run_to_finish(a);
        assert!(approx_eq(final_value, 2.0, 1e-4));
    }

    #[test]
    fn snap_to_resets_position_between_segments() {
        // A common loop primitive: tween somewhere, snap back to
        // start, repeat.
        let seq = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(50)).linear())
            .then(SnapTo::new(0.0_f32))
            .then(TweenTo::new(1.0_f32, Duration::from_millis(50)).linear());
        let (final_value, _) = run_to_finish(seq.build(0.0, 0.0));
        assert!(approx_eq(final_value, 1.0, 1e-4));
    }

    #[test]
    fn velocity_carries_across_boundaries() {
        // A tween-into-spring should land *past* the tween target
        // momentarily because the spring inherits the tween's
        // finite-difference velocity at the boundary. Verify by
        // sampling at the boundary moment.
        let seq = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(48)).linear())
            .then(SpringTo::new(1.0_f32).stiffness(100.0).damping(10.0));
        let mut a = seq.build(0.0, 0.0);
        // Tween completes after ~48ms = 3 STEPs. At boundary the
        // spring should still see a positive velocity.
        for _ in 0..4 {
            a.sample(STEP);
        }
        let s = a.sample(STEP);
        // Now in spring territory with momentum; value should be
        // at or past 1.0.
        assert!(s.value >= 1.0 - 1e-3, "value {} not near/past target", s.value);
    }

    #[test]
    fn sequence_is_cloneable_for_loop_use() {
        // Compile-time check that SequenceFactory<T>: Clone — a
        // requirement for `LoopFactory::new(SequenceFactory…)`.
        let seq = SequenceFactory::<f32>::new()
            .then(TweenTo::new(1.0_f32, Duration::from_millis(50)));
        let cloned = seq.clone();
        let _ = cloned.build(0.0, 0.0);
    }
}
