//! Raw touch event pipeline — the lowest-level interaction surface.
//!
//! The framework receives platform touch events through this module and
//! delivers them to subscribers as `TouchEvent`s. All gesture recognition
//! (tap, long-press, pan, swipe, pinch, custom) runs in Rust on top of
//! this stream; the framework does **not** integrate with native gesture
//! recognizers (`UIGestureRecognizer`, Android `GestureDetector`, etc.).
//! See `docs/native-touch-plan.md` for the design rationale.
//!
//! Platform-specific delivery lives entirely behind the [`Backend`]
//! trait (`install_touch_handler` / `claim_touch`). Core knows nothing
//! about UIView subclassing, `MotionEvent` action codes, Pointer Events,
//! or winit — those are backend implementation details.
//!
//! [`Backend`]: crate::Backend

pub mod recognizers;

use std::rc::Rc;

/// A 2-D position in pixels. Used for both view-local and window-global
/// coordinates on [`TouchEvent`]. Origin is the top-left, y grows down,
/// matching the convention every supported platform happens to share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    pub x: f32,
    pub y: f32,
}

impl TouchPoint {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Stable identifier for a single finger or pointer through the lifetime
/// of one interaction — minted at [`TouchPhase::Began`] and reused for
/// every subsequent event for that finger until [`TouchPhase::Ended`] or
/// [`TouchPhase::Cancelled`].
///
/// Backends are responsible for assigning ids that don't collide across
/// concurrent fingers. Reusing an id after the corresponding finger has
/// lifted is permitted; handlers must not assume monotonicity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TouchId(pub u64);

/// Phase a [`TouchEvent`] reports on. Mirrors the four states every
/// supported platform exposes natively (UIKit `UITouch.Phase`, Android
/// `MotionEvent.ACTION_*`, web `pointer{down,move,up,cancel}`, winit
/// `TouchPhase`).
///
/// `Cancelled` is **first-class and distinct from `Ended`**. Recognizers
/// must reset their state on `Cancelled` exactly like `Ended` *but* must
/// not treat the gesture as completed — a Cancelled tap should not fire
/// the click action, a Cancelled long-press should not surface the
/// long-press callback. Causes include: system interrupts (incoming
/// call, alert), a parent claiming the touch, the subscribed node
/// detaching mid-touch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

/// One delivery of touch state to a subscribed handler. Multi-touch is
/// dispatched **per touch, not batched** — a two-finger interaction
/// produces two parallel streams of events sharing a handler, each
/// carrying its own [`TouchId`].
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    /// Stable identity for this finger / pointer.
    pub id: TouchId,
    /// Lifecycle phase. See [`TouchPhase`].
    pub phase: TouchPhase,
    /// Position relative to the subscribed view's top-left corner.
    pub position: TouchPoint,
    /// Position relative to the window's top-left corner. Used by
    /// recognizers that need to track motion that crosses view bounds
    /// (e.g. a pan that overshoots, drag-and-drop hand-off).
    pub window_position: TouchPoint,
    /// Platform monotonic timestamp in nanoseconds. Suitable for
    /// computing velocity / inter-event durations; **not** an absolute
    /// wall-clock time.
    pub timestamp_ns: u64,
    /// Normalized 0.0..=1.0 force / pressure if the input device
    /// reports it (3D Touch, Apple Pencil, stylus). `None` on devices
    /// that don't, on mouse, and on platforms that don't surface it.
    pub force: Option<f32>,
}

/// A handler's reply for one [`TouchEvent`]. The two flags are
/// independent — a handler can consume an event without claiming the
/// gesture, or claim without consuming.
#[derive(Clone, Copy, Debug, Default)]
pub struct TouchResponse {
    /// `true` → this view handles the event; do not bubble to the next
    /// subscribed ancestor in the responder chain. `false` → bubble.
    ///
    /// The bubble decision is committed at `Began`: whichever ancestor
    /// consumes the `Began` keeps every subsequent event for the same
    /// [`TouchId`]. An unconsumed `Began` re-tries one level up; this
    /// repeats until either a handler consumes or the chain runs out
    /// (and the event is dropped).
    pub consumed: bool,
    /// `true` → preempt any competing native consumers of this touch.
    /// Triggers the backend's claim protocol (e.g. cancelling parent
    /// scroll views, capturing the pointer on web). Idempotent: calling
    /// claim on every subsequent event is harmless.
    ///
    /// Typical use: a horizontal pan recognizer inside a vertical
    /// `ScrollView` returns `claim: true` once it has seen ≥ 8 px of
    /// horizontal movement, at which point the parent stops scrolling.
    pub claim: bool,
}

impl TouchResponse {
    /// Convenience: event consumed, no claim. Equivalent to
    /// `TouchResponse { consumed: true, claim: false }`.
    pub const CONSUMED: Self = Self { consumed: true, claim: false };

    /// Convenience: event ignored, will bubble. Equivalent to
    /// `TouchResponse::default()`.
    pub const IGNORED: Self = Self { consumed: false, claim: false };

    /// Convenience: event consumed AND gesture claimed.
    pub const CLAIMED: Self = Self { consumed: true, claim: true };
}

/// Boxed handler installed on a primitive. The framework holds one of
/// these per subscribed node and invokes it for every event delivered
/// to that node (after responder-chain resolution).
pub type TouchHandler = Rc<dyn Fn(&TouchEvent) -> TouchResponse>;
