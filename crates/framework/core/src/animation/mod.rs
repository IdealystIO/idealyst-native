//! Animation system: per-frame motion sources, value handles, and
//! the shared clock that ties them together.
//!
//! See the module-level docs on [`animatable`], [`animator`],
//! [`tween`], [`spring`], [`decay`], [`clock`], [`value`] for the
//! pieces. The end-to-end author flow is:
//!
//! ```ignore
//! use framework_core::animation::*;
//! use std::time::Duration;
//!
//! let scale = AnimatedValue::new(1.0_f32);
//!
//! // Tween to 1.1 over 150ms, ease-out.
//! scale.animate(TweenTo::new(1.1, Duration::from_millis(150)).ease_out());
//!
//! // Mid-flight, hand off to a spring (inherits the tween's
//! // current velocity).
//! scale.animate(SpringTo::new(1.0).stiffness(280).damping(22));
//!
//! // Subscribe to per-frame updates and propagate to a backend
//! // property.
//! let _sub = scale.subscribe(|value, _velocity| {
//!     // backend.set_animated_f32(node, AnimProp::ScaleX, *value);
//! });
//! ```

pub mod animatable;
pub mod animator;
pub mod binding;
pub mod clock;
pub mod combinators;
pub mod curve;
pub mod decay;
pub mod keyframes;
pub mod prop;
pub mod repeat;
pub mod sequence;
pub mod spring;
pub mod tween;
pub mod value;

pub use animatable::Animatable;
pub use animator::{Animator, AnimatorFactory, Sample, MAX_FRAME_DT};
pub use clock::{
    register, register_guarded, tick_for_test, unregister, TickFn, TickId, TickRegistration,
};
pub use combinators::{stagger, ErasedFactory, SnapTo, Wait};
pub use curve::{apply_easing, cubic_bezier_y, BEZIER_NEWTON_ITERATIONS};
pub use decay::{
    Decay, DecayFrom, DEFAULT_DECAY_FRICTION, DEFAULT_DECAY_REST_VELOCITY,
};
pub use keyframes::{KeyframesAnimator, KeyframesTo};
pub use prop::AnimProp;
pub use repeat::{LoopAnimator, LoopFactory, Repeat};
pub use sequence::{SequenceAnimator, SequenceFactory, MAX_SEGMENTS_PER_FRAME};
pub use spring::{
    Spring, SpringTo, DEFAULT_SPRING_DAMPING, DEFAULT_SPRING_MASS,
    DEFAULT_SPRING_REST_DISPLACEMENT, DEFAULT_SPRING_REST_VELOCITY, DEFAULT_SPRING_STIFFNESS,
};
pub use tween::{Tween, TweenTo};
pub use value::{AnimatedValue, Subscription};
