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
//! ## Dispatch model (identical on every backend)
//!
//! Delivery is **deepest-view-first, then bubble to ancestors** — the
//! responder model, not a parent-intercepts-first model. For a `Began`
//! at a point, the *deepest* view under that point whose handler is
//! installed is asked first; if it returns [`IGNORED`] the event re-tries
//! the nearest ancestor handler, repeating up the chain until one
//! consumes (or the chain runs out and the event is dropped). The
//! ancestor that consumes the `Began` keeps every later event for that
//! [`TouchId`]. Every backend realizes this the same way despite
//! different mechanisms: hit-test → `nextResponder`/`super` on Apple and
//! winit, bubble-phase Pointer Events on web, and `OnTouchListener`
//! target dispatch on Android. A parent handler therefore does **not**
//! pre-empt its children — a tap on an interactive descendant reaches the
//! descendant first.
//!
//! ## Two footguns this model creates
//!
//! 1. **An ancestor that takes the press takes the whole subtree's
//!    gesture.** Consuming the `Began` already binds the pointer to that
//!    handler for the rest of the gesture (that is the promise above, and
//!    on web `setPointerCapture` is what keeps it — see
//!    `backend_web::primitives::touch`); [`CLAIMED`] additionally invokes
//!    the backend's preemption protocol, disallow-intercept / cancel the
//!    ancestor scroller on native. Either way, descendant controls that
//!    rely on their own touch stream — or on a synthesized `click`, which
//!    web only fires when `pointerdown`+`pointerup` both land on the same
//!    element — never see the gesture and go dead. **Put the handler on
//!    the leaf that should own the press, not on a container that wraps
//!    live controls.** An ancestor that only needs to stop a touch from
//!    falling through to a surface *beneath* it (e.g. a scrim over a
//!    canvas) should return [`CONSUMED`] (consume **without** `claim`) and
//!    must not enclose the controls it means to keep interactive.
//!
//! 2. **[`IGNORED`] bubbles to ancestors, never to siblings or layers
//!    beneath.** There is no `pointer-events: none` and no z-order
//!    fall-through: a handler cannot return `IGNORED` to "let the touch
//!    reach the view stacked under me." If two stacked surfaces must share
//!    a region, route the hit-testing through a single owning surface that
//!    inspects the point and dispatches itself, rather than relying on
//!    fall-through between siblings.
//!
//! [`Backend`]: crate::Backend
//! [`IGNORED`]: TouchResponse::IGNORED
//! [`CONSUMED`]: TouchResponse::CONSUMED
//! [`CLAIMED`]: TouchResponse::CLAIMED

pub mod recognizer;
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
    /// Pointer motion with **no button down** — mouse/trackpad hover. Only
    /// pointing devices produce it (touch never hovers): the web backend
    /// forwards unpressed `pointermove`s, macOS forwards `mouseMoved:` via its
    /// tracking area; iOS/Android never deliver it. It is NOT part of any
    /// gesture: recognizers ignore it entirely (a hover must never advance,
    /// reject, or cancel tap/pan/drag state), and handlers that only care
    /// about presses can treat it like a no-op. Exists for presence-style
    /// consumers — e.g. broadcasting a live cursor position while the user
    /// merely points at a canvas.
    Hovered,
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
    ///
    /// **Consuming is what guarantees delivery** — including motion that
    /// leaves the view's bounds, which native gives for free (UIKit
    /// `touchesMoved:`, AppKit `mouseDragged:`, Android's touch target)
    /// and web buys with `setPointerCapture` at this moment. A recognizer
    /// that has to measure travel before it can decide the gesture is its
    /// own therefore consumes from `Began`; it does NOT need to `claim`
    /// to keep hearing about the pointer.
    pub consumed: bool,
    /// `true` → preempt any competing native consumers of this touch.
    /// Triggers the backend's claim protocol (cancelling parent scroll
    /// views, disallow-intercept). Idempotent: calling claim on every
    /// subsequent event is harmless.
    ///
    /// Strictly about preemption, never about delivery — `consumed` is
    /// what routes the rest of the gesture here. Don't claim early to buy
    /// events; that steals the touch from any scroller the handler sits
    /// in, which is exactly wrong on mobile.
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

/// Keyboard modifier state that was active when the current pointer/touch event
/// was delivered. [`TouchEvent`] is `Copy` and constructed in many backends, so
/// rather than widen it we expose the modifiers out-of-band: a backend calls
/// [`set_pointer_modifiers`] immediately before invoking the touch handler, and
/// the handler reads them via [`pointer_modifiers`]. Plain touch / pen input
/// reports all-`false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Cmd on macOS, Win/Super elsewhere.
    pub meta: bool,
}

impl PointerModifiers {
    /// Whether the conventional **add-to-selection** modifier is held — Shift or
    /// the platform command key (Cmd/Meta). Deliberately NOT Ctrl: on macOS
    /// Ctrl-click is a right-click, so it's reserved.
    pub fn extends_selection(self) -> bool {
        self.shift || self.meta
    }
}

thread_local! {
    static POINTER_MODIFIERS: std::cell::Cell<PointerModifiers> =
        const { std::cell::Cell::new(PointerModifiers { shift: false, ctrl: false, alt: false, meta: false }) };
}

/// Record the modifier state for the touch/pointer event about to be dispatched.
/// **Backend-facing** — call right before invoking the [`TouchHandler`] so a
/// handler reading [`pointer_modifiers`] sees the state for THIS event.
pub fn set_pointer_modifiers(m: PointerModifiers) {
    POINTER_MODIFIERS.with(|c| c.set(m));
}

/// The modifier state recorded for the in-flight pointer/touch event. Valid only
/// while a touch handler is running (read it synchronously inside `on_touch`).
pub fn pointer_modifiers() -> PointerModifiers {
    POINTER_MODIFIERS.with(|c| c.get())
}

/// Which pointer button produced the current event. Passed out-of-band for the
/// same reason as [`PointerModifiers`]: [`TouchEvent`] is `Copy` and constructed
/// by every backend, so widening it would be a breaking change across all of
/// them.
///
/// Touch and pen contact report [`Primary`](PointerButton::Primary), so a
/// handler that only acts on `Primary` behaves identically on every input
/// device.
///
/// **A secondary press delivers only a [`Began`](TouchPhase::Began)** — no
/// `Moved`, no `Ended`. Non-primary presses deliberately never enter the
/// drag/capture path: a browser context menu can swallow the matching
/// `pointerup`, which would strand a claimed gesture (a dragged element
/// following the cursor forever). Treat the `Began` as the whole click.
///
/// Because of that, **every gesture recognizer ignores a non-primary
/// `Began`** — the gate lives in
/// [`Recognizer::drive`](crate::touch::recognizer::Recognizer::drive), so a
/// recognizer never consumes a press it cannot finish and the event bubbles
/// to whatever is listening for a context menu. A hand-rolled `on_touch`
/// handler that claims the touch owes the same check:
///
/// ```ignore
/// if ev.phase == TouchPhase::Began && !pointer_button().is_primary() {
///     return TouchResponse::IGNORED;
/// }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerButton {
    /// Left mouse button, or any touch / pen contact.
    #[default]
    Primary,
    /// Right mouse button — and, on macOS, a Ctrl-held left click, which the
    /// OS reports as a secondary press.
    Secondary,
    /// Middle button / wheel click.
    Middle,
    /// Anything else the platform reports (back / forward / pen barrel).
    Other(u16),
}

impl PointerButton {
    /// Whether this press should open a context menu — the platform-correct
    /// test, rather than each call site re-deriving "right click OR macOS
    /// Ctrl-click" (the OS has already folded the latter into `Secondary`).
    pub fn opens_context_menu(self) -> bool {
        matches!(self, PointerButton::Secondary)
    }

    /// Whether this press is the ordinary "activate" press — the one a tap,
    /// a drag, or any other gesture is made of. Touch and pen contact are
    /// always `Primary`, so a gate written against this is a no-op on a
    /// touch-only backend.
    pub fn is_primary(self) -> bool {
        matches!(self, PointerButton::Primary)
    }
}

thread_local! {
    static POINTER_BUTTON: std::cell::Cell<PointerButton> =
        const { std::cell::Cell::new(PointerButton::Primary) };
}

/// Record which button produced the touch/pointer event about to be dispatched.
/// **Backend-facing** — call right before invoking the [`TouchHandler`], next to
/// [`set_pointer_modifiers`]. A backend with no button concept leaves it alone;
/// the default is [`Primary`](PointerButton::Primary).
pub fn set_pointer_button(b: PointerButton) {
    POINTER_BUTTON.with(|c| c.set(b));
}

/// The button recorded for the in-flight pointer/touch event. Valid only while a
/// touch handler is running (read it synchronously inside `on_touch`).
pub fn pointer_button() -> PointerButton {
    POINTER_BUTTON.with(|c| c.get())
}

thread_local! {
    static ACTIVE_TOUCH_CLAIM: std::cell::RefCell<Option<Rc<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Publish a **node-bound claim closure** for the touch about to be dispatched.
/// **Backend-facing** — a backend that implements the claim protocol calls this
/// right before invoking the [`TouchHandler`] (and clears it with `None` right
/// after), passing a closure that performs its native claim for *this* node
/// (cancel ancestor scrollers, etc. — the same thing [`Backend::claim_touch`]
/// would do).
///
/// Why this exists: the synchronous claim path ([`TouchResponse::claim`] read off
/// the handler's return) only fires when a handler *returns* `claim: true` during
/// an event. A recognizer that commits **off the touch stream** — e.g. a
/// long-press whose timer fires while the finger is held still — has no event to
/// return on at the instant it decides "this is mine". By then the next move is
/// the one a native scroll container sees *first*, so it wins and our handler is
/// cancelled. Capturing this closure synchronously (on `Began`) and invoking it
/// at the async commit lets the recognizer claim at exactly the right moment,
/// before any movement. Claiming is idempotent, so an extra call is harmless.
///
/// [`Backend::claim_touch`]: crate::Backend::claim_touch
pub fn set_active_touch_claim(claim: Option<Rc<dyn Fn()>>) {
    ACTIVE_TOUCH_CLAIM.with(|c| *c.borrow_mut() = claim);
}

/// The claim closure published by the backend for the touch currently being
/// dispatched, if any. A recognizer captures this synchronously (typically on
/// `Began`), holds its own clone, and invokes it if/when it commits off-stream.
/// Returns `None` on backends that don't implement the claim protocol — callers
/// must treat the absence as "claiming unavailable", not an error.
pub fn active_touch_claim() -> Option<Rc<dyn Fn()>> {
    ACTIVE_TOUCH_CLAIM.with(|c| c.borrow().clone())
}

#[cfg(test)]
mod pointer_button_tests {
    use super::*;

    /// The default must be `Primary`: backends with no button concept
    /// (touch, pen, and every native platform that doesn't call
    /// `set_pointer_button`) never set it, and a handler that gates on
    /// `Primary` has to keep working there.
    #[test]
    fn defaults_to_primary_so_buttonless_backends_behave() {
        assert_eq!(PointerButton::default(), PointerButton::Primary);
        assert_eq!(pointer_button(), PointerButton::Primary);
    }

    /// Only a secondary press opens a context menu. macOS Ctrl-click is
    /// folded into `Secondary` by the OS before it reaches us, which is
    /// why call sites must NOT re-derive "right click or Ctrl held" —
    /// on Windows/Linux a Ctrl-held LEFT click is an additive click, not
    /// a menu.
    #[test]
    fn only_secondary_opens_a_context_menu() {
        assert!(PointerButton::Secondary.opens_context_menu());
        assert!(!PointerButton::Primary.opens_context_menu());
        assert!(!PointerButton::Middle.opens_context_menu());
        assert!(!PointerButton::Other(3).opens_context_menu());
    }

    #[test]
    fn round_trips_through_the_thread_local() {
        for b in [
            PointerButton::Secondary,
            PointerButton::Middle,
            PointerButton::Other(4),
            PointerButton::Primary,
        ] {
            set_pointer_button(b);
            assert_eq!(pointer_button(), b);
        }
    }
}
