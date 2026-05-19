//! Pluggable screen-transition animators for stack navigators.
//!
//! Each [`ScreenTransition`] decides how the two screens involved
//! in a push or pop animate: the *under* screen (the one already
//! mounted that the operation is moving away from / back toward)
//! and the *top* screen (the one being slid in / out). The
//! navigator owns one `Rc<dyn ScreenTransition>`; the dispatcher
//! seeds a [`crate::node::NavTransition`] with the kind +
//! start-time and the renderer samples this animator each frame
//! to compute the per-screen transform.
//!
//! Adding a new style — modal slide-up, fade, zoom-and-fade —
//! means writing one new impl. The renderer only depends on the
//! trait: it never reads a transition kind directly.

use std::cell::RefCell;
use std::rc::Rc;

/// Which direction a navigator transition is running.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransitionDirection {
    /// New screen entering (handle.push / replace / reset).
    Push,
    /// Top screen leaving (handle.pop).
    Pop,
}

/// Per-screen transform sampled by the renderer. Translates are
/// in logical pixels relative to the screen's natural resting
/// position inside the navigator's rect.
#[derive(Copy, Clone, Debug, Default)]
pub struct ScreenXform {
    pub translate_x: f32,
    pub translate_y: f32,
}

/// Sampled transition state for one frame.
#[derive(Copy, Clone, Debug, Default)]
pub struct TransitionFrame {
    pub under: ScreenXform,
    pub top: ScreenXform,
}

/// The animator contract.
///
/// Implementations supply a duration + a `sample` that maps
/// `(direction, progress, viewport_size)` to a `TransitionFrame`.
/// Progress is normalized `[0, 1]` — easing happens inside
/// `sample` so each animator can pick its own curve.
///
/// `Rc`'d on the navigator so multiple in-flight transitions
/// (e.g., a quick push-then-pop) share one allocation and the
/// renderer can clone cheaply for sampling.
pub trait ScreenTransition {
    fn duration_ms(&self) -> u32;
    fn sample(
        &self,
        direction: TransitionDirection,
        progress: f32,
        width: f32,
        height: f32,
    ) -> TransitionFrame;
}

/// Duration of the iOS-style horizontal slide. ~280ms feels
/// closer to Material 3 than UIKit's ~350ms but keeps stacked
/// pushes responsive.
pub const SLIDE_FROM_RIGHT_MS: u32 = 280;
/// Parallax fraction applied to the under-screen — at full
/// progress it has drifted left by this fraction of the
/// navigator's width. iOS uses ~30%; 0.0 disables the parallax.
pub const SLIDE_FROM_RIGHT_PARALLAX: f32 = 0.30;

/// iOS-style: top screen slides in from the right edge, the
/// under-screen drifts left a fraction of the way (parallax)
/// to sell the "layered above" illusion. Ease-out cubic.
pub struct SlideFromRight {
    pub duration_ms: u32,
    pub parallax: f32,
}

impl SlideFromRight {
    pub fn new() -> Self {
        Self {
            duration_ms: SLIDE_FROM_RIGHT_MS,
            parallax: SLIDE_FROM_RIGHT_PARALLAX,
        }
    }
}

impl Default for SlideFromRight {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenTransition for SlideFromRight {
    fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    fn sample(
        &self,
        direction: TransitionDirection,
        progress: f32,
        width: f32,
        _height: f32,
    ) -> TransitionFrame {
        // Ease-out cubic — settles smoothly toward the resting
        // position, matches the feel of UIKit's default push.
        let eased = 1.0 - (1.0 - progress).powi(3);
        let (under_x, top_x) = match direction {
            TransitionDirection::Push => {
                // Under drifts left, top slides in from the right.
                (-eased * self.parallax * width, (1.0 - eased) * width)
            }
            TransitionDirection::Pop => {
                // Under returns from -parallax→0, top exits 0→+width.
                (-(1.0 - eased) * self.parallax * width, eased * width)
            }
        };
        TransitionFrame {
            under: ScreenXform { translate_x: under_x, translate_y: 0.0 },
            top: ScreenXform { translate_x: top_x, translate_y: 0.0 },
        }
    }
}

/// Material-style: top screen slides up from the bottom edge,
/// the under-screen stays put. Useful for "present modally"
/// navigators where the new screen is conceptually a sheet.
pub struct SlideFromBottom {
    pub duration_ms: u32,
}

impl SlideFromBottom {
    pub fn new() -> Self {
        Self { duration_ms: SLIDE_FROM_RIGHT_MS }
    }
}

impl Default for SlideFromBottom {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenTransition for SlideFromBottom {
    fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    fn sample(
        &self,
        direction: TransitionDirection,
        progress: f32,
        _width: f32,
        height: f32,
    ) -> TransitionFrame {
        let eased = 1.0 - (1.0 - progress).powi(3);
        let top_y = match direction {
            TransitionDirection::Push => (1.0 - eased) * height,
            TransitionDirection::Pop => eased * height,
        };
        TransitionFrame {
            under: ScreenXform::default(),
            top: ScreenXform { translate_x: 0.0, translate_y: top_y },
        }
    }
}

/// No animation — the new screen snaps in / out instantly. The
/// dispatcher still observes the transition so the deferred Pop
/// cleanup runs, but the render samples land at progress=1 from
/// frame 1.
pub struct InstantTransition;

impl ScreenTransition for InstantTransition {
    fn duration_ms(&self) -> u32 {
        0
    }

    fn sample(
        &self,
        _direction: TransitionDirection,
        _progress: f32,
        _width: f32,
        _height: f32,
    ) -> TransitionFrame {
        TransitionFrame::default()
    }
}

/// Default animator for new navigators. Used when no
/// [`with_transition`] override is in scope.
pub fn default_transition() -> Rc<dyn ScreenTransition> {
    Rc::new(SlideFromRight::new())
}

// ---------------------------------------------------------------
// Per-navigator override
//
// Users select a non-default animator by wrapping the
// `Navigator::new(...).bind(handle).into()` chain in
// [`with_transition`]. The wgpu backend's `create_navigator`
// consumes whatever's installed at call time.
//
// Lifecycle note: the override stays installed until the next
// `create_navigator` call consumes it — it is *not* cleared on
// `with_transition`'s exit. This is intentional: the framework's
// walker processes the returned `Primitive::Navigator` *after*
// the screen-builder closure (and `with_transition`) has
// already returned, so an auto-cleared override would always
// be gone by the time `create_navigator` actually runs. Leaving
// it set lets the deferred walker pick it up. If a
// `with_transition` block ends without producing a Navigator
// the override stays staged for the next nav built on this
// thread — call [`clear_transition_override`] manually if you
// need to abandon a staged value.
// ---------------------------------------------------------------

thread_local! {
    static TRANSITION_OVERRIDE: RefCell<Option<Rc<dyn ScreenTransition>>> =
        const { RefCell::new(None) };
}

/// Stage an animator for the next `Navigator` constructed on
/// this thread. The framework's walker only fires
/// `create_navigator` *after* the user closure that built the
/// `Primitive::Navigator` has returned, so this thread-local
/// stays set across the closure boundary and is consumed
/// when the walker eventually reaches the navigator node.
///
/// ```ignore
/// nav_anim::with_transition(Rc::new(SlideFromBottom::new()), || {
///     Navigator::new(&MODAL_ROUTE)
///         .screen(MODAL_ROUTE, |_| Screen::new(modal_root()))
///         .bind(nav)
///         .into()
/// })
/// ```
///
/// Place `with_transition` immediately around the `Navigator`
/// construction. If multiple Navigators are built before any
/// of them mount, the last `with_transition` wins.
pub fn with_transition<R>(anim: Rc<dyn ScreenTransition>, build: impl FnOnce() -> R) -> R {
    TRANSITION_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(anim);
    });
    build()
}

/// Drop a staged transition override without binding it to a
/// navigator. Rarely needed in normal flow — the next
/// `create_navigator` consumes the stage automatically.
pub fn clear_transition_override() {
    TRANSITION_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Take and clear the current transition override. The wgpu
/// backend's `create_navigator` calls this once per navigator;
/// `None` means use [`default_transition`].
pub(crate) fn take_transition_override() -> Option<Rc<dyn ScreenTransition>> {
    TRANSITION_OVERRIDE.with(|cell| cell.borrow_mut().take())
}
