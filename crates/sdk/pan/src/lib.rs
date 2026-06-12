//! # `pan` — a sensible cross-platform pan / drag gesture
//!
//! A [`Pan`] handle wraps the framework's single-finger pan recognizer
//! ([`runtime_core::pan`]) and the animation primitives
//! ([`runtime_core::animation::AnimatedValue`]) into the one thing almost
//! every drag interaction needs: a **reactive 2D offset** that tracks the
//! finger (or mouse, or pen), survives across successive grabs, and binds
//! straight to a view's translate.
//!
//! ## Why it works everywhere with no per-platform code
//!
//! The framework already converges pointer input *below* this crate. On web,
//! the backend listens on Pointer Events, which fold mouse, touch, and pen
//! into one stream; the native backends (iOS / Android / macOS) deliver their
//! touches through the same [`runtime_core::TouchEvent`] shape. So "Desktop /
//! Web and touch screens alike" is satisfied at the input layer — a
//! left-button mouse drag and a finger drag arrive here as the identical
//! `Began → Moved* → Ended` sequence. The output side is symmetric:
//! [`Pan::bind`] drives [`AnimProp::TranslateX`]/[`AnimProp::TranslateY`],
//! which each backend writes through its own native transform. There is
//! nothing to branch on, so this is one pure-Rust crate.
//!
//! ## What it deliberately does NOT do
//!
//! Momentum / fling, snap points, axis locking, bounds clamping, and
//! swipe-to-dismiss are **not** built in. They are policy, and policy belongs
//! to the app or a higher-level SDK. This crate hands you the offset and the
//! lifecycle hooks; you build the rest *with* them — see [`Pan::on_end`]:
//!
//! ```ignore
//! use pan::Pan;
//! use runtime_core::animation::DecayFrom;
//! use runtime_core::{Ref, ViewHandle};
//!
//! let view_ref: Ref<ViewHandle> = Ref::new();
//! let pan = Pan::new()
//!     // momentum is the app's call, expressed via the exposed AnimatedValue:
//!     .on_end(|end| {
//!         // fling horizontally; clamp / snap / dismiss would go here too.
//!     });
//!
//! // ... inside render, bind the offset to the view's translate:
//! pan.bind(view_ref);
//!
//! view(children)
//!     .on_touch(pan.handler())
//!     .bind(view_ref)
//! ```
//!
//! The `Pan` handle is cheap to clone (it is a bundle of `Rc`s and
//! `AnimatedValue`s, all of which share their backing state), so clone it
//! freely — the clone drives the same offset.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{
    pan as pan_recognizer, PanEvent, PanRecognizer, Ref, TouchHandler, TouchPoint, ViewHandle,
};

/// Snapshot delivered to the [`Pan::on_start`] and [`Pan::on_change`]
/// callbacks. Carries both the recognizer's raw figures and the managed
/// offset so a handler can react without re-reading the [`Pan`] handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanInfo {
    /// View-local touch position for this event.
    pub position: TouchPoint,
    /// Cumulative movement since this gesture's start (i.e. since the finger
    /// landed), **not** the per-event increment. On [`Pan::on_start`] this is
    /// `(0, 0)`.
    pub delta: TouchPoint,
    /// The managed offset that was just written to the bound values:
    /// `offset = base_at_gesture_start + delta`. This is what
    /// [`AnimProp::TranslateX`]/[`AnimProp::TranslateY`] receive.
    pub offset: TouchPoint,
    /// Smoothed pixels-per-second velocity. Zero on [`Pan::on_start`].
    pub velocity: TouchPoint,
}

/// Snapshot delivered to the [`Pan::on_end`] callback when the finger lifts
/// after an active pan. The `velocity` is the recognizer's final smoothed
/// estimate — feed it to a decay / spring animator if you want momentum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanEnd {
    /// Final smoothed pixels-per-second velocity at release.
    pub velocity: TouchPoint,
    /// The offset the element came to rest at when the finger lifted.
    pub offset: TouchPoint,
}

type StartCb = Rc<dyn Fn(PanInfo)>;
type ChangeCb = Rc<dyn Fn(PanInfo)>;
type EndCb = Rc<dyn Fn(PanEnd)>;
type CancelCb = Rc<dyn Fn()>;

/// A reactive pan / drag handle. See the [crate docs](crate) for the full
/// rationale; the short version:
///
/// - It owns an `(x, y)` offset as two [`AnimatedValue<f32>`]s.
/// - [`Pan::handler`] produces the [`TouchHandler`] you install via
///   `view(..).on_touch(pan.handler())`.
/// - While dragging, the offset is `base + delta`, where `base` is snapshotted
///   at each gesture start — so a second drag continues from where the first
///   left off instead of jumping back to the origin.
/// - [`Pan::bind`] wires the offset to a view's translate.
///
/// Clone is cheap and shares state.
#[derive(Clone)]
pub struct Pan {
    config: PanRecognizer,
    x: AnimatedValue<f32>,
    y: AnimatedValue<f32>,
    /// Offset captured at the start of the in-flight gesture. `Moved` events
    /// add the recognizer's cumulative delta to this. Kept in a `Cell`
    /// because every `Pan` clone must observe the same base.
    base: Rc<Cell<(f32, f32)>>,
    on_start: Option<StartCb>,
    on_change: Option<ChangeCb>,
    on_end: Option<EndCb>,
    on_cancel: Option<CancelCb>,
}

impl Default for Pan {
    fn default() -> Self {
        Self::new()
    }
}

impl Pan {
    /// A pan whose offset starts at `(0, 0)` with the default recognizer
    /// (8 px slop — matches `UIPanGestureRecognizer`).
    pub fn new() -> Self {
        Self::with_config(PanRecognizer::new())
    }

    /// A pan whose offset starts at `(x, y)`. Use this when the element is not
    /// laid out at the origin of its translate space and you want the offset
    /// to read as an absolute position.
    pub fn at(x: f32, y: f32) -> Self {
        let p = Self::new();
        p.set_offset(x, y);
        p
    }

    /// A pan with a custom recognizer (e.g. a tighter or looser activation
    /// slop via [`PanRecognizer::slop_px`]).
    pub fn with_config(config: PanRecognizer) -> Self {
        Self {
            config,
            x: AnimatedValue::new(0.0),
            y: AnimatedValue::new(0.0),
            base: Rc::new(Cell::new((0.0, 0.0))),
            on_start: None,
            on_change: None,
            on_end: None,
            on_cancel: None,
        }
    }

    /// The horizontal offset value. Clone it to [`AnimatedValue::bind`] it to
    /// [`AnimProp::TranslateX`], or to [`AnimatedValue::animate`] it (e.g.
    /// momentum in [`Pan::on_end`]).
    pub fn x(&self) -> AnimatedValue<f32> {
        self.x.clone()
    }

    /// The vertical offset value. See [`Pan::x`].
    pub fn y(&self) -> AnimatedValue<f32> {
        self.y.clone()
    }

    /// The current offset as a plain pair, sampled now.
    pub fn offset(&self) -> (f32, f32) {
        (self.x.get(), self.y.get())
    }

    /// Programmatically move the offset (cancels any in-flight offset
    /// animation, since you are setting an exact value). Safe to call from a
    /// lifecycle callback or anywhere else.
    pub fn set_offset(&self, x: f32, y: f32) {
        self.x.set(x);
        self.y.set(y);
    }

    /// Register a callback fired once when a pan becomes active (the finger
    /// has moved past the recognizer's slop). Replaces any previous one.
    pub fn on_start(mut self, f: impl Fn(PanInfo) + 'static) -> Self {
        self.on_start = Some(Rc::new(f));
        self
    }

    /// Register a callback fired on every move while the pan is active, after
    /// the offset has been updated. Replaces any previous one.
    pub fn on_change(mut self, f: impl Fn(PanInfo) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// Register a callback fired when the finger lifts after an active pan.
    /// This is where momentum / snap / dismiss policy lives — act on
    /// [`PanEnd::velocity`] and [`PanEnd::offset`]. Replaces any previous one.
    pub fn on_end(mut self, f: impl Fn(PanEnd) + 'static) -> Self {
        self.on_end = Some(Rc::new(f));
        self
    }

    /// Register a callback fired when the platform interrupts an active pan
    /// (incoming call, system gesture, view detach). A common response is to
    /// animate the offset back to rest. Replaces any previous one.
    pub fn on_cancel(mut self, f: impl Fn() + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }

    /// Bind the offset to `target`'s translate. Wires `x → TranslateX` and
    /// `y → TranslateY` in one call. Like any [`AnimatedValue::bind`], call
    /// this during render inside the active reactive scope (the subscriptions
    /// anchor to that scope and unbind when it drops).
    pub fn bind(&self, target: Ref<ViewHandle>) {
        // `Ref` is `Copy`, so each axis gets its own handle to bind to.
        self.x.bind(target, AnimProp::TranslateX);
        self.y.bind(target, AnimProp::TranslateY);
    }

    /// Produce the installable [`TouchHandler`]. Install it with
    /// `view(..).on_touch(pan.handler())`.
    ///
    /// State machine (on top of [`runtime_core::pan`]):
    /// - **Began** (slop crossed): snapshot `base = current offset` and
    ///   [`AnimatedValue::cancel`] any in-flight offset animation, so grabbing
    ///   the element pins it to the finger at its current position instead of
    ///   letting a running fling fight the drag. Then fire [`Pan::on_start`].
    /// - **Moved**: write `offset = base + cumulative_delta`, then fire
    ///   [`Pan::on_change`].
    /// - **Ended**: fire [`Pan::on_end`] with the final velocity and offset.
    ///   The offset is left exactly where the finger released — momentum is
    ///   the callback's job.
    /// - **Cancelled**: fire [`Pan::on_cancel`].
    pub fn handler(&self) -> TouchHandler {
        pan_recognizer(self.config, self.event_handler())
    }

    /// Build the underlying core [`runtime_core::Pan`] recognizer wired to
    /// this handle's offset + callbacks, for composing in a
    /// `gesture::GestureGroup` alongside other recognizers (e.g. a pinch
    /// zoom). Use [`Pan::handler`] for the standalone single-`on_touch`
    /// case; the recognizer it returns and this one share no state, so use
    /// one or the other per handle.
    pub fn recognizer(&self) -> runtime_core::Pan {
        runtime_core::Pan::new(self.config, self.event_handler())
    }

    /// The `PanEvent` → offset/callback closure shared by [`Pan::handler`]
    /// and [`Pan::recognizer`]. Each call captures fresh clones of the
    /// reactive handles, so the two construction paths stay independent.
    fn event_handler(&self) -> impl Fn(&PanEvent) + 'static {
        let x = self.x.clone();
        let y = self.y.clone();
        let base = self.base.clone();
        let on_start = self.on_start.clone();
        let on_change = self.on_change.clone();
        let on_end = self.on_end.clone();
        let on_cancel = self.on_cancel.clone();

        move |ev: &PanEvent| match ev {
            PanEvent::Began { position } => {
                // Take over from any running animation and snapshot where we
                // are right now, so successive grabs accumulate.
                x.cancel();
                y.cancel();
                let b = (x.get(), y.get());
                base.set(b);
                if let Some(cb) = &on_start {
                    cb(PanInfo {
                        position: *position,
                        delta: TouchPoint::ZERO,
                        offset: TouchPoint::new(b.0, b.1),
                        velocity: TouchPoint::ZERO,
                    });
                }
            }
            PanEvent::Moved {
                position,
                delta,
                velocity,
            } => {
                let (bx, by) = base.get();
                let ox = bx + delta.x;
                let oy = by + delta.y;
                x.set(ox);
                y.set(oy);
                if let Some(cb) = &on_change {
                    cb(PanInfo {
                        position: *position,
                        delta: *delta,
                        offset: TouchPoint::new(ox, oy),
                        velocity: *velocity,
                    });
                }
            }
            PanEvent::Ended { velocity } => {
                if let Some(cb) = &on_end {
                    cb(PanEnd {
                        velocity: *velocity,
                        offset: TouchPoint::new(x.get(), y.get()),
                    });
                }
            }
            PanEvent::Cancelled => {
                if let Some(cb) = &on_cancel {
                    cb();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! These drive a `Pan::handler()` with synthetic `TouchEvent`s — the same
    //! technique the core recognizer tests use — and assert on the managed
    //! offset and the callback payloads. They cover the SDK's whole value-add:
    //! offset accumulation, base snapshotting across successive gestures, and
    //! callback wiring. The underlying recognizer's own slop / phase logic is
    //! covered in `runtime_core::touch::recognizers`.

    use super::*;
    use runtime_core::{TouchEvent, TouchId, TouchPhase};
    use std::cell::RefCell;

    fn ev(phase: TouchPhase, id: u64, x: f32, y: f32, ts_ns: u64) -> TouchEvent {
        TouchEvent {
            id: TouchId(id),
            phase,
            position: TouchPoint::new(x, y),
            window_position: TouchPoint::new(x, y),
            timestamp_ns: ts_ns,
            force: None,
        }
    }

    /// Drive a single complete drag: finger down at `from`, move to `to`
    /// (past the 8 px default slop so the pan activates), then lift. Returns
    /// nothing — assert on the `Pan`'s offset afterwards.
    fn drag(h: &TouchHandler, id: u64, from: (f32, f32), to: (f32, f32), t0: u64) {
        h(&ev(TouchPhase::Began, id, from.0, from.1, t0));
        h(&ev(TouchPhase::Moved, id, to.0, to.1, t0 + 16_000_000));
        h(&ev(TouchPhase::Ended, id, to.0, to.1, t0 + 32_000_000));
    }

    #[test]
    fn drag_writes_cumulative_offset() {
        let pan = Pan::new();
        let h = pan.handler();
        // Finger down at origin, drag 40 px right / 10 px down.
        drag(&h, 1, (0.0, 0.0), (40.0, 10.0), 0);
        let (ox, oy) = pan.offset();
        assert_eq!(ox, 40.0, "x offset should equal the cumulative delta");
        assert_eq!(oy, 10.0, "y offset should equal the cumulative delta");
    }

    #[test]
    fn recognizer_drives_the_same_managed_offset() {
        // The `recognizer()` composition path (for GestureGroup) must wire
        // the same offset as `handler()`. Drive it through the Recognizer
        // trait directly.
        use runtime_core::{Recognizer, RecognizerCtx};
        let pan = Pan::new();
        let mut rec = pan.recognizer();
        let ctx = RecognizerCtx::UNGATED;
        rec.update(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0), &ctx);
        rec.update(&ev(TouchPhase::Moved, 1, 40.0, 10.0, 16_000_000), &ctx);
        rec.update(&ev(TouchPhase::Ended, 1, 40.0, 10.0, 32_000_000), &ctx);
        assert_eq!(pan.offset(), (40.0, 10.0));
    }

    #[test]
    fn successive_gestures_accumulate_from_last_offset() {
        let pan = Pan::new();
        let h = pan.handler();
        // First drag leaves the offset at (40, 0).
        drag(&h, 1, (0.0, 0.0), (40.0, 0.0), 0);
        assert_eq!(pan.offset(), (40.0, 0.0));
        // A second, independent drag of +15 px must continue from 40, not
        // snap back to delta-from-zero. base is snapshotted on Began.
        drag(&h, 2, (100.0, 0.0), (115.0, 0.0), 1_000_000_000);
        assert_eq!(
            pan.offset(),
            (55.0, 0.0),
            "second gesture should build on the first's resting offset"
        );
    }

    #[test]
    fn on_change_reports_offset_delta_and_position() {
        let seen: Rc<RefCell<Vec<PanInfo>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let pan = Pan::new().on_change(move |info| sink.borrow_mut().push(info));
        let h = pan.handler();
        drag(&h, 1, (0.0, 0.0), (40.0, 10.0), 0);

        let events = seen.borrow();
        // The recognizer fires an immediate Moved(delta=0) on activation, then
        // the real one — so the last on_change carries the full drag.
        let last = events.last().expect("on_change should have fired");
        assert_eq!(last.delta, TouchPoint::new(40.0, 10.0));
        assert_eq!(last.offset, TouchPoint::new(40.0, 10.0));
        assert_eq!(last.position, TouchPoint::new(40.0, 10.0));
    }

    #[test]
    fn on_start_fires_once_with_base_offset() {
        let starts: Rc<RefCell<Vec<PanInfo>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = starts.clone();
        // Pre-seed the offset so the start snapshot is non-zero.
        let pan = Pan::at(7.0, 3.0).on_start(move |info| sink.borrow_mut().push(info));
        let h = pan.handler();
        drag(&h, 1, (0.0, 0.0), (40.0, 0.0), 0);

        let events = starts.borrow();
        assert_eq!(events.len(), 1, "on_start fires exactly once per gesture");
        assert_eq!(events[0].delta, TouchPoint::ZERO);
        assert_eq!(
            events[0].offset,
            TouchPoint::new(7.0, 3.0),
            "start offset is the pre-gesture base"
        );
    }

    #[test]
    fn on_end_reports_resting_offset() {
        let ends: Rc<RefCell<Vec<PanEnd>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = ends.clone();
        let pan = Pan::new().on_end(move |end| sink.borrow_mut().push(end));
        let h = pan.handler();
        drag(&h, 1, (0.0, 0.0), (25.0, 0.0), 0);

        let events = ends.borrow();
        assert_eq!(events.len(), 1, "on_end fires once when the finger lifts");
        assert_eq!(events[0].offset, TouchPoint::new(25.0, 0.0));
    }

    #[test]
    fn cancel_after_active_fires_on_cancel() {
        let cancels = Rc::new(Cell::new(0u32));
        let sink = cancels.clone();
        let pan = Pan::new().on_cancel(move || sink.set(sink.get() + 1));
        let h = pan.handler();
        // Activate the pan (cross slop), then cancel.
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 1, 40.0, 0.0, 16_000_000));
        h(&ev(TouchPhase::Cancelled, 1, 40.0, 0.0, 32_000_000));
        assert_eq!(cancels.get(), 1);
    }

    #[test]
    fn below_slop_tap_does_not_move_or_fire() {
        let changes = Rc::new(Cell::new(0u32));
        let sink = changes.clone();
        let pan = Pan::new().on_change(move |_| sink.set(sink.get() + 1));
        let h = pan.handler();
        // 3 px wobble — under the 8 px slop, so the pan never activates.
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 1, 3.0, 0.0, 16_000_000));
        h(&ev(TouchPhase::Ended, 1, 3.0, 0.0, 32_000_000));
        assert_eq!(pan.offset(), (0.0, 0.0), "sub-slop wobble must not move");
        assert_eq!(changes.get(), 0, "no on_change for a non-pan");
    }

    #[test]
    fn set_offset_moves_programmatically() {
        let pan = Pan::new();
        pan.set_offset(12.0, -5.0);
        assert_eq!(pan.offset(), (12.0, -5.0));
        // A subsequent drag accumulates from the programmatic position.
        let h = pan.handler();
        drag(&h, 1, (0.0, 0.0), (10.0, 0.0), 0);
        assert_eq!(pan.offset(), (22.0, -5.0));
    }

    #[test]
    fn clone_shares_the_same_offset() {
        let pan = Pan::new();
        let clone = pan.clone();
        // Driving the original's handler moves the clone's offset too.
        let h = pan.handler();
        drag(&h, 1, (0.0, 0.0), (40.0, 0.0), 0);
        assert_eq!(clone.offset(), (40.0, 0.0));
    }
}
