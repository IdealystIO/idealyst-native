//! # `zoom` — a sensible cross-platform zoom gesture
//!
//! The scale peer of the [`pan`](https://docs.rs/pan) SDK. A [`Zoom`] handle
//! converges the two input families that mean "zoom" into one **reactive scale
//! factor** plus a **focal point**, and binds straight to a view's scale:
//!
//! - **Pinch** — two fingers on a touch screen. Rides the framework's
//!   [`pinch`](runtime_core::pinch) recognizer, which is built on the existing
//!   touch stream, so it works on every backend (iOS, Android, web-touch,
//!   macOS-touch) with no per-platform code.
//! - **Trackpad pinch + scroll-wheel** — desktop. Rides the framework's wheel
//!   channel (web `wheel`+`ctrlKey`, macOS `magnify:`), which each backend
//!   normalizes into a uniform incremental [`WheelEvent::scale`](runtime_core::WheelEvent).
//!
//! You install the pinch side with `view(..).on_touch(zoom.pinch_handler())`
//! and the wheel side with `view(..).on_wheel(zoom.wheel_handler())` — the same
//! `Zoom` drives both, so a pinch on a phone and a trackpad pinch on a laptop
//! move the identical value.
//!
//! ## What it deliberately leaves to you
//!
//! Min/max clamping, momentum, snap-to-fit, and focal-point "zoom about the
//! cursor" translation are **policy**, not built in — exactly the same stance
//! as `pan`. The scale is an `AnimatedValue`, so momentum is one line in
//! [`Zoom::on_end`]; clamping is a comparison in [`Zoom::on_change`]; and the
//! focal point is reported on every event so you can pair this with a `pan`
//! offset to keep the point under the fingers fixed.
//!
//! ```ignore
//! use zoom::Zoom;
//! use runtime_core::{view, Ref, ViewHandle};
//!
//! let view_ref: Ref<ViewHandle> = Ref::new();
//! let zoom = Zoom::new()
//!     .on_change(|info| {
//!         // info.scale, info.focus, info.velocity — clamp / translate here.
//!     });
//! zoom.bind(view_ref); // scale → AnimProp::Scale
//!
//! view(vec![/* ... */])
//!     .on_touch(zoom.pinch_handler())  // touch screens
//!     .on_wheel(zoom.wheel_handler())  // desktop trackpad / wheel
//!     .bind(view_ref)
//! ```

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{
    pinch as pinch_recognizer, PinchEvent, PinchRecognizer, Ref, TouchHandler, TouchPoint,
    TouchResponse, ViewHandle, WheelHandler, WheelKind,
};

/// Snapshot delivered to [`Zoom::on_start`] and [`Zoom::on_change`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomInfo {
    /// Focal point of the gesture in the view's local coordinates — the
    /// midpoint of the two fingers for a pinch, the cursor for a wheel zoom.
    /// The natural point to keep fixed while scaling.
    pub focus: TouchPoint,
    /// The managed scale factor that was just written to the bound value.
    /// `1.0` = no zoom; `> 1.0` = zoomed in.
    pub scale: f32,
    /// Scale-units-per-second velocity (pinch only; `0.0` for the discrete
    /// wheel). Hand to a decay animator in [`Zoom::on_end`] for momentum.
    pub velocity: f32,
}

/// Snapshot delivered to [`Zoom::on_end`] when a pinch's fingers lift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomEnd {
    /// Final scale-units-per-second velocity at release.
    pub velocity: f32,
    /// The scale the gesture came to rest at.
    pub scale: f32,
}

type StartCb = Rc<dyn Fn(ZoomInfo)>;
type ChangeCb = Rc<dyn Fn(ZoomInfo)>;
type EndCb = Rc<dyn Fn(ZoomEnd)>;
type CancelCb = Rc<dyn Fn()>;

/// A reactive zoom handle. See the [crate docs](crate). In short:
///
/// - It owns the scale as an [`AnimatedValue<f32>`] (starts at `1.0`).
/// - [`Zoom::pinch_handler`] / [`Zoom::wheel_handler`] produce the handlers you
///   install via `on_touch` / `on_wheel`.
/// - During a pinch the scale is `base * gesture_scale`, where `base` is
///   snapshotted at the gesture start — so successive pinches compound instead
///   of resetting to `1.0`. A wheel zoom multiplies the live scale directly.
/// - [`Zoom::bind`] wires the scale to a view's [`AnimProp::Scale`].
///
/// Clone is cheap and shares state.
#[derive(Clone)]
pub struct Zoom {
    config: PinchRecognizer,
    scale: AnimatedValue<f32>,
    /// Scale captured at the start of the in-flight pinch; the recognizer's
    /// cumulative `gesture_scale` multiplies onto this.
    base: Rc<Cell<f32>>,
    on_start: Option<StartCb>,
    on_change: Option<ChangeCb>,
    on_end: Option<EndCb>,
    on_cancel: Option<CancelCb>,
}

impl Default for Zoom {
    fn default() -> Self {
        Self::new()
    }
}

impl Zoom {
    /// A zoom whose scale starts at `1.0` with the default pinch recognizer.
    pub fn new() -> Self {
        Self::with_config(PinchRecognizer::new())
    }

    /// A zoom whose scale starts at `initial`.
    pub fn at(initial: f32) -> Self {
        let z = Self::new();
        z.set_scale(initial);
        z
    }

    /// A zoom with a custom pinch recognizer (e.g. a tighter activation slop).
    pub fn with_config(config: PinchRecognizer) -> Self {
        Self {
            config,
            scale: AnimatedValue::new(1.0),
            base: Rc::new(Cell::new(1.0)),
            on_start: None,
            on_change: None,
            on_end: None,
            on_cancel: None,
        }
    }

    /// The scale value. Clone it to [`AnimatedValue::bind`] it to
    /// [`AnimProp::Scale`], or to [`AnimatedValue::animate`] it (momentum in
    /// [`Zoom::on_end`]).
    pub fn scale(&self) -> AnimatedValue<f32> {
        self.scale.clone()
    }

    /// The current scale factor, sampled now.
    pub fn value(&self) -> f32 {
        self.scale.get()
    }

    /// Programmatically set the scale (cancels any in-flight scale animation).
    pub fn set_scale(&self, scale: f32) {
        self.scale.set(scale);
    }

    /// Register a callback fired once when a pinch becomes active. Replaces any
    /// previous one. (Wheel zoom has no distinct start; it only fires
    /// `on_change`.)
    pub fn on_start(mut self, f: impl Fn(ZoomInfo) + 'static) -> Self {
        self.on_start = Some(Rc::new(f));
        self
    }

    /// Register a callback fired on every scale change — each pinch move and
    /// each wheel-zoom event — after the scale has been updated. Replaces any
    /// previous one.
    pub fn on_change(mut self, f: impl Fn(ZoomInfo) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// Register a callback fired when a pinch's fingers lift. This is where
    /// momentum / snap policy lives. Replaces any previous one.
    pub fn on_end(mut self, f: impl Fn(ZoomEnd) + 'static) -> Self {
        self.on_end = Some(Rc::new(f));
        self
    }

    /// Register a callback fired when the platform interrupts an active pinch.
    /// Replaces any previous one.
    pub fn on_cancel(mut self, f: impl Fn() + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }

    /// Bind the scale to `target`'s [`AnimProp::Scale`]. Call during render
    /// inside the active reactive scope (the subscription anchors there and
    /// unbinds when it drops), like any [`AnimatedValue::bind`].
    pub fn bind(&self, target: Ref<ViewHandle>) {
        self.scale.bind(target, AnimProp::Scale);
    }

    /// Produce the touch handler for the **pinch** input — install with
    /// `view(..).on_touch(zoom.pinch_handler())`.
    ///
    /// On [`PinchEvent::Began`] it snapshots `base = current scale` and cancels
    /// any in-flight scale animation (so grabbing pins the zoom where it is);
    /// each [`PinchEvent::Changed`] writes `base * gesture_scale`; the fingers
    /// lifting fires [`Zoom::on_end`].
    pub fn pinch_handler(&self) -> TouchHandler {
        pinch_recognizer(self.config, self.pinch_event_handler())
    }

    /// Build the underlying core [`runtime_core::Pinch`] recognizer wired
    /// to this handle's scale + callbacks, for composing in a
    /// `gesture::GestureGroup` alongside other recognizers (e.g. a pan, or
    /// a rotate). Use [`Zoom::pinch_handler`] for the standalone
    /// single-`on_touch` case; the two share no state, so use one per
    /// handle. (The wheel path is separate — see [`Zoom::wheel_handler`].)
    pub fn recognizer(&self) -> runtime_core::Pinch {
        runtime_core::Pinch::new(self.config, self.pinch_event_handler())
    }

    /// The `PinchEvent` → scale/callback closure shared by
    /// [`Zoom::pinch_handler`] and [`Zoom::pinch_recognizer`]. Each call
    /// captures fresh clones, so the two construction paths stay
    /// independent.
    fn pinch_event_handler(&self) -> impl Fn(&PinchEvent) + 'static {
        let scale = self.scale.clone();
        let base = self.base.clone();
        let on_start = self.on_start.clone();
        let on_change = self.on_change.clone();
        let on_end = self.on_end.clone();
        let on_cancel = self.on_cancel.clone();

        move |ev: &PinchEvent| match ev {
            PinchEvent::Began { focus } => {
                scale.cancel();
                let b = scale.get();
                base.set(b);
                if let Some(cb) = &on_start {
                    cb(ZoomInfo {
                        focus: *focus,
                        scale: b,
                        velocity: 0.0,
                    });
                }
            }
            PinchEvent::Changed {
                focus,
                scale: gesture_scale,
                velocity,
            } => {
                let b = base.get();
                let s = b * gesture_scale;
                scale.set(s);
                if let Some(cb) = &on_change {
                    cb(ZoomInfo {
                        focus: *focus,
                        scale: s,
                        // gesture_scale is relative to base, so the absolute
                        // scale velocity is base * d(gesture_scale)/dt.
                        velocity: velocity * b,
                    });
                }
            }
            PinchEvent::Ended { velocity } => {
                if let Some(cb) = &on_end {
                    cb(ZoomEnd {
                        velocity: velocity * base.get(),
                        scale: scale.get(),
                    });
                }
            }
            PinchEvent::Cancelled => {
                if let Some(cb) = &on_cancel {
                    cb();
                }
            }
        }
    }

    /// Produce the wheel handler for the **desktop** input (trackpad pinch /
    /// scroll-wheel) — install with `view(..).on_wheel(zoom.wheel_handler())`.
    ///
    /// Only [`WheelKind::Zoom`] events affect the scale: each multiplies the
    /// live scale by the backend-normalized incremental factor, cancels any
    /// running animation, and fires [`Zoom::on_change`] (with the cursor as the
    /// focus). The event is consumed so the page doesn't also zoom/scroll.
    /// [`WheelKind::Scroll`] events are ignored (returned unconsumed) so normal
    /// scrolling still works.
    pub fn wheel_handler(&self) -> WheelHandler {
        let scale = self.scale.clone();
        let on_change = self.on_change.clone();

        Rc::new(move |ev| {
            if ev.kind != WheelKind::Zoom {
                return TouchResponse::IGNORED;
            }
            scale.cancel();
            let s = scale.get() * ev.scale;
            scale.set(s);
            if let Some(cb) = &on_change {
                cb(ZoomInfo {
                    focus: ev.position,
                    scale: s,
                    velocity: 0.0,
                });
            }
            TouchResponse::CONSUMED
        })
    }
}

#[cfg(test)]
mod tests {
    //! Drive the handlers with synthetic events and assert on the managed
    //! scale + callback payloads. Covers the SDK's value-add: base
    //! snapshotting across pinches, wheel multiplication, and the
    //! pinch↔wheel convergence onto one value. The recognizer's own
    //! two-finger logic is tested in `runtime_core::touch::recognizers`.

    use super::*;
    use runtime_core::{TouchEvent, TouchId, TouchPhase, WheelEvent, WheelKind};
    use std::cell::RefCell;

    fn touch(phase: TouchPhase, id: u64, x: f32, y: f32, ts_ns: u64) -> TouchEvent {
        TouchEvent {
            id: TouchId(id),
            phase,
            position: TouchPoint::new(x, y),
            window_position: TouchPoint::new(x, y),
            timestamp_ns: ts_ns,
            force: None,
        }
    }

    fn wheel_zoom(scale: f32, x: f32, y: f32) -> WheelEvent {
        WheelEvent {
            kind: WheelKind::Zoom,
            delta_x: 0.0,
            delta_y: 0.0,
            scale,
            position: TouchPoint::new(x, y),
            window_position: TouchPoint::new(x, y),
            timestamp_ns: 0,
        }
    }

    /// Two fingers from `start_gap` to `end_gap` px apart, centered on x=200.
    /// Both fingers move out symmetrically so the midpoint stays at the center
    /// and the final distance is exactly `end_gap` → scale `end_gap/start_gap`.
    fn pinch_spread(h: &TouchHandler, start_gap: f32, end_gap: f32) {
        let c = 200.0;
        h(&touch(TouchPhase::Began, 1, c - start_gap / 2.0, 0.0, 0));
        h(&touch(TouchPhase::Began, 2, c + start_gap / 2.0, 0.0, 0));
        h(&touch(TouchPhase::Moved, 1, c - end_gap / 2.0, 0.0, 16_000_000));
        h(&touch(TouchPhase::Moved, 2, c + end_gap / 2.0, 0.0, 16_000_000));
    }

    /// Lift both fingers, fully resetting the recognizer to idle so a
    /// subsequent `pinch_spread` starts a clean gesture.
    fn end_both(h: &TouchHandler) {
        h(&touch(TouchPhase::Ended, 2, 300.0, 0.0, 32_000_000));
        h(&touch(TouchPhase::Ended, 1, 100.0, 0.0, 33_000_000));
    }

    #[test]
    fn pinch_spread_scales_up() {
        let zoom = Zoom::new();
        let h = zoom.pinch_handler();
        // Start 100 px apart, spread to 200 → 2x.
        pinch_spread(&h, 100.0, 200.0);
        assert!((zoom.value() - 2.0).abs() < 1e-3, "got {}", zoom.value());
    }

    #[test]
    fn recognizer_drives_the_same_managed_scale() {
        // The `recognizer()` composition path (for GestureGroup) must wire
        // the same scale as `pinch_handler()`. Drive it through the
        // Recognizer trait directly.
        use runtime_core::{Recognizer, RecognizerCtx};
        let zoom = Zoom::new();
        let mut rec = zoom.recognizer();
        let ctx = RecognizerCtx::UNGATED;
        let c = 200.0;
        rec.update(&touch(TouchPhase::Began, 1, c - 50.0, 0.0, 0), &ctx);
        rec.update(&touch(TouchPhase::Began, 2, c + 50.0, 0.0, 0), &ctx);
        rec.update(&touch(TouchPhase::Moved, 1, c - 100.0, 0.0, 16_000_000), &ctx);
        rec.update(&touch(TouchPhase::Moved, 2, c + 100.0, 0.0, 16_000_000), &ctx);
        assert!((zoom.value() - 2.0).abs() < 1e-3, "got {}", zoom.value());
    }

    #[test]
    fn successive_pinches_compound_from_last_scale() {
        let zoom = Zoom::new();
        let h = zoom.pinch_handler();
        pinch_spread(&h, 100.0, 200.0); // → 2.0
        end_both(&h);
        assert!((zoom.value() - 2.0).abs() < 1e-3);
        // A second pinch that doubles again must land on 4.0, not 2.0 — base is
        // snapshotted from the resting scale.
        pinch_spread(&h, 100.0, 200.0); // ×2 again
        assert!((zoom.value() - 4.0).abs() < 1e-3, "got {}", zoom.value());
    }

    #[test]
    fn on_change_reports_scale_and_focus() {
        let seen: Rc<RefCell<Vec<ZoomInfo>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let zoom = Zoom::new().on_change(move |i| sink.borrow_mut().push(i));
        let h = zoom.pinch_handler();
        pinch_spread(&h, 100.0, 200.0);
        let events = seen.borrow();
        let last = events.last().expect("on_change fired");
        assert!((last.scale - 2.0).abs() < 1e-3);
        // Midpoint of the two fingers sits at x=200 (the spread center).
        assert!((last.focus.x - 200.0).abs() < 1e-3);
    }

    #[test]
    fn on_end_reports_resting_scale() {
        let ends: Rc<RefCell<Vec<ZoomEnd>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = ends.clone();
        let zoom = Zoom::new().on_end(move |e| sink.borrow_mut().push(e));
        let h = zoom.pinch_handler();
        pinch_spread(&h, 100.0, 200.0);
        h(&touch(TouchPhase::Ended, 2, 300.0, 0.0, 32_000_000));
        let events = ends.borrow();
        assert_eq!(events.len(), 1);
        assert!((events[0].scale - 2.0).abs() < 1e-3);
    }

    #[test]
    fn wheel_zoom_multiplies_scale_and_consumes() {
        let zoom = Zoom::new();
        let wh = zoom.wheel_handler();
        let r = wh(&wheel_zoom(1.25, 10.0, 10.0));
        assert!(r.consumed, "zoom wheel events are consumed (preventDefault)");
        assert!((zoom.value() - 1.25).abs() < 1e-3);
        // Compounds.
        wh(&wheel_zoom(2.0, 10.0, 10.0));
        assert!((zoom.value() - 2.5).abs() < 1e-3, "got {}", zoom.value());
    }

    #[test]
    fn wheel_scroll_is_ignored() {
        let zoom = Zoom::new();
        let wh = zoom.wheel_handler();
        let scroll = WheelEvent {
            kind: WheelKind::Scroll,
            delta_x: 0.0,
            delta_y: 40.0,
            scale: 1.0,
            position: TouchPoint::new(0.0, 0.0),
            window_position: TouchPoint::new(0.0, 0.0),
            timestamp_ns: 0,
        };
        let r = wh(&scroll);
        assert!(!r.consumed, "scroll must pass through, not be consumed as zoom");
        assert_eq!(zoom.value(), 1.0, "scroll must not change scale");
    }

    #[test]
    fn pinch_and_wheel_drive_the_same_value() {
        let zoom = Zoom::new();
        let ph = zoom.pinch_handler();
        let wh = zoom.wheel_handler();
        pinch_spread(&ph, 100.0, 200.0); // → 2.0
        end_both(&ph);
        wh(&wheel_zoom(1.5, 0.0, 0.0)); // 2.0 × 1.5
        assert!((zoom.value() - 3.0).abs() < 1e-3, "got {}", zoom.value());
    }
}
