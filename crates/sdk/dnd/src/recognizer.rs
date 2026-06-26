//! [`DragRecognizer`] — the gesture FSM that drives a drag.
//!
//! It is the `pan`/`long_press` story fused into one recognizer: it watches
//! the raw [`TouchEvent`] stream and emits a [`DragPhase`] lifecycle to its
//! callback. The drag-and-drop policy (payload, hit-testing droppables, the
//! ghost transform) lives in [`Draggable`](crate::Draggable)'s callback, not
//! here — this recognizer only decides *when* a drag is active and reports
//! the finger's motion in both view-local and window coordinates.
//!
//! ## Why window coordinates matter
//!
//! A drag is the one gesture that routinely crosses the bounds of the view it
//! started in (drag a card from one column to another). Hit-testing drop
//! targets therefore happens in window space, so every [`DragSample`] carries
//! [`TouchEvent::window_position`] alongside the view-local position the
//! ghost transform uses.
//!
//! ## Activation
//!
//! [`Activation`] picks *when* the drag commits, the one axis where touch and
//! pointer platforms genuinely differ:
//!
//! - [`Activation::Immediate`] — commit as soon as the finger crosses `slop`
//!   pixels. Right for desktop / mouse reordering and for drag handles.
//! - [`Activation::LongPress`] — press and hold `ms` milliseconds (finger
//!   still within `slop`) before the drag commits. The touch-platform
//!   convention: it lets a list scroll normally until the user deliberately
//!   picks an item up, instead of fighting the scroll on every touch.
//!
//! [`Activation::platform_default`] reads [`runtime_core::platform`] and picks
//! long-press on phones/tablets, immediate elsewhere — legitimate
//! `Platform`-enum branching, not a per-backend hack.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::scheduling::{after_ms, ScheduledTask};
use runtime_core::{
    AsyncNotifier, GestureState, Platform, Recognizer, RecognizerCtx, RecognizerKind,
    RecognizerUpdate, TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse,
};

/// Default activation slop for [`Activation::Immediate`] — matches the pan
/// recognizer's 8 px so an immediate drag and a pan feel identical.
pub const DEFAULT_DRAG_SLOP_PX: f32 = 8.0;

/// Default hold duration for [`Activation::LongPress`] — the press-and-hold
/// delay before a drag picks up. Deliberately SHORTER than the OS context-menu
/// long-press (UIKit/Android ~500 ms): for drag-to-reorder the hold *is* the
/// interaction, so 500 ms reads as sluggish. 200 ms matches the snappy pickup
/// of Trello / native collection-view reordering while still being long enough
/// that a quick swipe (which crosses the slop first) escapes to scroll.
pub const DEFAULT_DRAG_LONG_PRESS_MS: u64 = 200;

/// Default movement tolerance during a long-press hold before the press is
/// abandoned (the user meant to scroll). Matches the long-press default.
pub const DEFAULT_DRAG_LONG_PRESS_SLOP_PX: f32 = 10.0;

/// EMA mixing factor for velocity smoothing — the same constant the pan /
/// pinch / swipe recognizers use, so release velocities are comparable.
const DRAG_VELOCITY_SMOOTHING: f32 = 0.6;

/// The axis a draggable's enclosing scroller scrolls along. Used by
/// [`Activation::scroll_aware`] / [`Activation::DirectionalLongPress`] to tell
/// a *drag* gesture apart from a *scroll* by direction: motion PERPENDICULAR to
/// this axis can't be a scroll (so it's a drag), motion ALONG it is a scroll
/// until a hold says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis {
    /// The scroller pans left-right (e.g. a horizontal board). Vertical motion
    /// is the unambiguous drag direction.
    Horizontal,
    /// The scroller pans up-down (e.g. a vertical list). Horizontal motion is
    /// the unambiguous drag direction.
    Vertical,
}

/// When a drag commits relative to the finger landing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Activation {
    /// Commit once the finger has travelled `slop_px` from where it landed.
    Immediate { slop_px: f32 },
    /// Commit after the finger is held `threshold_ms` within `slop_px` of
    /// where it landed. Moving past `slop_px` before the timer fires abandons
    /// the drag (and leaves the touch to any native scroller).
    LongPress { threshold_ms: u64, slop_px: f32 },
    /// Direction-aware hybrid for a draggable inside a scroller — the robust
    /// choice for reorderable lists/boards. Decisive motion PERPENDICULAR to
    /// `scroll_axis` past `slop_px` commits the drag IMMEDIATELY (it can't be a
    /// scroll, so there's nothing to wait for — instant pickup). Decisive motion
    /// ALONG `scroll_axis` past `slop_px` abandons the drag and leaves the touch
    /// to the native scroller. A still hold of `threshold_ms` commits regardless
    /// of direction, so an along-axis (e.g. cross-column) drag is still possible:
    /// hold to pick up, then move anywhere. The first move's dominant axis
    /// decides; a hold is the tie-breaker.
    DirectionalLongPress {
        scroll_axis: ScrollAxis,
        threshold_ms: u64,
        slop_px: f32,
    },
}

impl Activation {
    /// Immediate activation at the default slop.
    pub fn immediate() -> Self {
        Self::Immediate {
            slop_px: DEFAULT_DRAG_SLOP_PX,
        }
    }

    /// Long-press activation at the default threshold + slop.
    pub fn long_press() -> Self {
        Self::LongPress {
            threshold_ms: DEFAULT_DRAG_LONG_PRESS_MS,
            slop_px: DEFAULT_DRAG_LONG_PRESS_SLOP_PX,
        }
    }

    /// Long-press on touch-first platforms ([`Platform::is_mobile`]),
    /// immediate everywhere else. Reads [`runtime_core::platform`].
    pub fn platform_default() -> Self {
        if runtime_core::platform().is_mobile() {
            Self::long_press()
        } else {
            Self::immediate()
        }
    }

    /// Direction-aware activation for a draggable inside a scroller on
    /// `scroll_axis` — prefer this over [`platform_default`](Self::platform_default)
    /// for reorderable lists/boards. On touch platforms it's
    /// [`DirectionalLongPress`](Self::DirectionalLongPress): perpendicular motion
    /// picks up instantly, along-axis motion scrolls, a hold picks up either way.
    /// On desktop (mouse drag is unambiguous; scrolling is a separate wheel/
    /// trackpad input) it degrades to [`immediate`](Self::immediate). Resilient
    /// before a backend is installed (falls back to immediate).
    pub fn scroll_aware(scroll_axis: ScrollAxis) -> Self {
        match runtime_core::platform() {
            Platform::Ios | Platform::Android => Self::DirectionalLongPress {
                scroll_axis,
                threshold_ms: DEFAULT_DRAG_LONG_PRESS_MS,
                slop_px: DEFAULT_DRAG_LONG_PRESS_SLOP_PX,
            },
            _ => Self::immediate(),
        }
    }

    /// Slop in px, whichever variant.
    fn slop_px(self) -> f32 {
        match self {
            Self::Immediate { slop_px }
            | Self::LongPress { slop_px, .. }
            | Self::DirectionalLongPress { slop_px, .. } => slop_px,
        }
    }

    /// The hold threshold for variants that arm a long-press timer (`None` for
    /// [`Immediate`](Self::Immediate)). Drives whether the recognizer arms its
    /// timer on `Began`.
    fn long_press_ms(self) -> Option<u64> {
        match self {
            Self::Immediate { .. } => None,
            Self::LongPress { threshold_ms, .. }
            | Self::DirectionalLongPress { threshold_ms, .. } => Some(threshold_ms),
        }
    }
}

impl Default for Activation {
    fn default() -> Self {
        Self::platform_default_or_immediate()
    }
}

impl Activation {
    /// `platform_default` but resilient to being called before a backend is
    /// installed (e.g. unit tests): falls back to immediate.
    fn platform_default_or_immediate() -> Self {
        // `platform()` returns `Custom("")` when no backend is mounted; that
        // is not mobile, so this naturally yields immediate off-device.
        match runtime_core::platform() {
            Platform::Ios | Platform::Android => Self::long_press(),
            _ => Self::immediate(),
        }
    }
}

/// One motion sample delivered to the drag callback. Carries both coordinate
/// spaces: `view_position` for the ghost transform, `window_position` for
/// drop-target hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragSample {
    /// Touch position relative to the dragged view's top-left.
    pub view_position: TouchPoint,
    /// Touch position relative to the window's top-left — the coordinate
    /// space drop targets are hit-tested in.
    pub window_position: TouchPoint,
    /// Cumulative movement since the drag committed (`(0,0)` on `Began`).
    /// Used directly as the offset to apply to the ghost.
    pub delta: TouchPoint,
    /// Smoothed pixels-per-second velocity. Zero on `Began`.
    pub velocity: TouchPoint,
}

/// Lifecycle a [`DragRecognizer`] reports to its callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragPhase {
    /// The drag committed (slop crossed, or long-press elapsed). `delta` is
    /// `(0,0)`.
    Began(DragSample),
    /// The finger moved while the drag is active.
    Moved(DragSample),
    /// The finger lifted after an active drag. `window_position` is the
    /// release point in window coordinates (where the drop is resolved);
    /// `velocity` is the final smoothed estimate (feed a fling/snap animator
    /// with it).
    Ended {
        window_position: TouchPoint,
        velocity: TouchPoint,
    },
    /// The platform interrupted an active drag (system gesture, view detach,
    /// a parent claim). Not a completed drop.
    Cancelled,
}

type DragCb = Rc<dyn Fn(DragPhase)>;

#[derive(Clone, Copy)]
enum State {
    /// No finger down, or the drag never committed.
    Idle,
    /// Finger down, drag not yet committed. For `Immediate` we watch slop;
    /// for `LongPress` the `timer` is armed and we watch slop to abandon.
    Tracking {
        id: TouchId,
        /// View-local landing position (slop reference + base for delta).
        start: TouchPoint,
        /// Most recent positions, so a long-press that commits off the touch
        /// stream begins at the finger's current location.
        last_view: TouchPoint,
        last_window: TouchPoint,
    },
    /// Long-press timer elapsed; waiting for the driver to `poll_async` (and,
    /// under the arbiter, for any require-to-fail gate to clear). Carries the
    /// finger's last known positions so the off-stream commit begins where the
    /// finger actually is.
    PendingCommit {
        id: TouchId,
        view: TouchPoint,
        window: TouchPoint,
    },
    /// Drag committed. `start` anchors the cumulative delta.
    Active {
        id: TouchId,
        start: TouchPoint,
        last_view: TouchPoint,
        last_ts_ns: u64,
        velocity: TouchPoint,
    },
    /// Long-press abandoned (moved past slop before the timer) — keep the
    /// finger to stay coherent, but never commit.
    Rejected { id: TouchId },
}

/// Drag gesture recognizer ([`Recognizer`] impl). Emits a [`DragPhase`]
/// lifecycle; see the module docs for the model and [`Activation`] for the
/// commit rule.
///
/// Returns `CONSUMED` while tracking (owns the touch in the framework's
/// responder chain, but a native scroller still runs alongside) and `CLAIMED`
/// once active (the backend's claim protocol cancels the native scroller and
/// hands the drag full ownership) — exactly like [`runtime_core::Pan`].
pub struct DragRecognizer {
    activation: Activation,
    on_drag: DragCb,
    state: Rc<RefCell<State>>,
    /// Armed long-press timer, kept alive until it fires or is cancelled.
    timer: Rc<RefCell<Option<ScheduledTask>>>,
    notifier: Rc<RefCell<Option<AsyncNotifier>>>,
    /// The backend's node-bound claim closure for the in-flight touch, captured
    /// synchronously on `Began` (see [`runtime_core::active_touch_claim`]). For a
    /// LongPress activation the commit happens off the touch stream (the timer),
    /// so there is no event to return `CLAIMED` on; we invoke this at commit
    /// instead, cancelling the ancestor scroller *before* the first move it would
    /// otherwise steal. `None` when no drag is tracking or the backend doesn't
    /// implement the claim protocol. Held only for the life of one gesture.
    claim: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
}

impl DragRecognizer {
    /// Construct from an activation policy + the lifecycle callback. Prefer
    /// [`Draggable`](crate::Draggable), which wires this to a payload, a
    /// drop-target hit-test, and a ghost offset; construct directly only when
    /// composing in a [`gesture::GestureGroup`].
    pub fn new<F: Fn(DragPhase) + 'static>(activation: Activation, on_drag: F) -> Self {
        Self {
            activation,
            on_drag: Rc::new(on_drag),
            state: Rc::new(RefCell::new(State::Idle)),
            timer: Rc::new(RefCell::new(None)),
            notifier: Rc::new(RefCell::new(None)),
            claim: Rc::new(RefCell::new(None)),
        }
    }

    fn map_state(s: &State) -> GestureState {
        match s {
            State::Idle | State::Tracking { .. } | State::PendingCommit { .. } => {
                GestureState::Possible
            }
            State::Active { .. } => GestureState::Changed,
            State::Rejected { .. } => GestureState::Failed,
        }
    }

    /// Arm the long-press timer for `id`. On elapse (if still tracking the
    /// same finger) it moves to `PendingCommit` and signals the driver, which
    /// re-polls via [`Recognizer::poll_async`] — never fires unilaterally, so
    /// the arbiter can still gate or cancel the commit.
    fn arm_timer(&self, id: TouchId, threshold_ms: u64) {
        let state = self.state.clone();
        let notifier = self.notifier.clone();
        let timer_slot = self.timer.clone();
        let task = after_ms(threshold_ms as i32, move || {
            let notify = {
                let mut s = state.borrow_mut();
                if let State::Tracking {
                    id: cur,
                    last_view,
                    last_window,
                    ..
                } = *s
                {
                    if cur == id {
                        *s = State::PendingCommit {
                            id,
                            view: last_view,
                            window: last_window,
                        };
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            // The timer fired; it's spent.
            *timer_slot.borrow_mut() = None;
            if notify {
                let n = notifier.borrow().clone();
                if let Some(n) = n {
                    n();
                }
            }
        });
        *self.timer.borrow_mut() = Some(task);
    }

    fn cancel_timer(&self) {
        if let Some(mut t) = self.timer.borrow_mut().take() {
            t.cancel();
        }
    }

    /// Transition into the active drag, anchoring the cumulative delta at
    /// `start` and reporting the finger now at `view`/`window`. Fires `Began`
    /// then an immediate `Moved`, both carrying `delta = view - start`
    /// (mirrors [`runtime_core::Pan`], which reports the slop distance already
    /// travelled). For an off-stream long-press commit, pass `start == view`
    /// so the drag begins with zero delta at the hold point.
    fn commit(
        &self,
        id: TouchId,
        start: TouchPoint,
        view: TouchPoint,
        window: TouchPoint,
        ts_ns: u64,
    ) {
        *self.state.borrow_mut() = State::Active {
            id,
            start,
            last_view: view,
            last_ts_ns: ts_ns,
            velocity: TouchPoint::ZERO,
        };
        // Claim the touch the instant the drag commits — critical on the
        // LongPress path, where this runs from the timer (off the touch stream),
        // so the ancestor native scroller is cancelled while the finger is still
        // held, before it can recognize the first move and steal the gesture.
        // (On the Immediate path the synchronous `CLAIMED` return already claims;
        // this is a harmless idempotent second call.)
        if let Some(claim) = self.claim.borrow().clone() {
            claim();
        }
        let sample = DragSample {
            view_position: view,
            window_position: window,
            delta: TouchPoint::new(view.x - start.x, view.y - start.y),
            velocity: TouchPoint::ZERO,
        };
        (self.on_drag)(DragPhase::Began(sample));
        (self.on_drag)(DragPhase::Moved(sample));
    }
}

impl Recognizer for DragRecognizer {
    fn name(&self) -> &'static str {
        "drag"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Continuous
    }
    fn state(&self) -> GestureState {
        Self::map_state(&self.state.borrow())
    }
    fn set_async_notifier(&mut self, notifier: AsyncNotifier) {
        *self.notifier.borrow_mut() = Some(notifier);
    }
    fn reset(&mut self, cancelled: bool) {
        self.cancel_timer();
        // Release the captured claim closure (it retains the native view on some
        // backends) — the gesture is over.
        *self.claim.borrow_mut() = None;
        let was_active = matches!(*self.state.borrow(), State::Active { .. });
        *self.state.borrow_mut() = State::Idle;
        if cancelled && was_active {
            (self.on_drag)(DragPhase::Cancelled);
        }
    }

    fn poll_async(&mut self, ctx: &RecognizerCtx) -> Option<RecognizerUpdate> {
        // Only the long-press path lands here, in `PendingCommit`.
        let (id, view, window) = match &*self.state.borrow() {
            State::PendingCommit { id, view, window } => (*id, *view, *window),
            _ => return None,
        };
        if !ctx.may_recognize {
            // Gated by an unresolved require-to-fail prerequisite: stay
            // pending; a later re-arbitration re-polls us.
            return Some(RecognizerUpdate::new(
                GestureState::Possible,
                TouchResponse::CONSUMED,
            ));
        }
        // No event timestamp off-stream; seed velocity timing from 0 and let
        // the first real `Moved` establish dt. Commit at the finger's last
        // known position (start == view → zero initial delta): a long-press
        // drag begins where you held, not where you first touched.
        self.commit(id, view, view, window, 0);
        Some(RecognizerUpdate::new(
            GestureState::Began,
            TouchResponse::CLAIMED,
        ))
    }

    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let activation = self.activation;
        let cur = *self.state.borrow();
        let (state, response): (GestureState, TouchResponse) = match (ev.phase, cur) {
            // ---- Began -------------------------------------------------
            (TouchPhase::Began, State::Idle) => {
                *self.state.borrow_mut() = State::Tracking {
                    id: ev.id,
                    start: ev.position,
                    last_view: ev.position,
                    last_window: ev.window_position,
                };
                // Grab the backend's claim closure for this touch NOW, while it's
                // valid (the backend publishes it only for the duration of this
                // synchronous dispatch). A LongPress commit later fires from the
                // timer, off-stream, where it's no longer available — so we hold
                // our own clone. `None` on backends without the claim protocol.
                *self.claim.borrow_mut() = runtime_core::active_touch_claim();
                if let Some(threshold_ms) = activation.long_press_ms() {
                    self.arm_timer(ev.id, threshold_ms);
                }
                (G::Possible, TouchResponse::CONSUMED)
            }
            // A second finger while we already track one — ignore extras so a
            // pinch sibling can pick them up.
            (TouchPhase::Began, _) => (self.state(), TouchResponse::IGNORED),

            // ---- Moved -------------------------------------------------
            (TouchPhase::Moved, State::Tracking { id, start, .. }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let dist2 = dx * dx + dy * dy;
                let slop = activation.slop_px();
                // Always keep the latest positions so an off-stream commit
                // (long-press) begins where the finger actually is.
                *self.state.borrow_mut() = State::Tracking {
                    id,
                    start,
                    last_view: ev.position,
                    last_window: ev.window_position,
                };
                match activation {
                    Activation::Immediate { .. } => {
                        if dist2 > slop * slop && ctx.may_recognize {
                            // Anchor delta at the touch-down position so the
                            // element follows from where it was grabbed.
                            self.commit(
                                ev.id,
                                start,
                                ev.position,
                                ev.window_position,
                                ev.timestamp_ns,
                            );
                            (G::Began, TouchResponse::CLAIMED)
                        } else {
                            (G::Possible, TouchResponse::CONSUMED)
                        }
                    }
                    Activation::LongPress { .. } => {
                        if dist2 > slop * slop {
                            // Moved too far before the hold elapsed → the user
                            // is scrolling, not dragging. Abandon; leave the
                            // touch to the native scroller (CONSUMED, not
                            // claimed, so the scroller already saw it).
                            self.cancel_timer();
                            *self.state.borrow_mut() = State::Rejected { id };
                            (G::Failed, TouchResponse::CONSUMED)
                        } else {
                            (G::Possible, TouchResponse::CONSUMED)
                        }
                    }
                    Activation::DirectionalLongPress { scroll_axis, .. } => {
                        if dist2 <= slop * slop {
                            // Sub-slop: undecided. The hold timer may still commit.
                            (G::Possible, TouchResponse::CONSUMED)
                        } else {
                            // First decisive motion classifies by dominant axis:
                            // `perp` is movement perpendicular to the scroll axis
                            // (the unambiguous drag direction), `along` is movement
                            // parallel to it (the scroll direction).
                            let (along, perp) = match scroll_axis {
                                ScrollAxis::Horizontal => (dx.abs(), dy.abs()),
                                ScrollAxis::Vertical => (dy.abs(), dx.abs()),
                            };
                            if perp >= along {
                                // Perpendicular to the scroll axis → can't be a
                                // scroll → pick up immediately (no hold). Honor a
                                // require-to-fail gate by waiting if not yet
                                // allowed to recognize.
                                if ctx.may_recognize {
                                    self.cancel_timer();
                                    self.commit(
                                        ev.id,
                                        start,
                                        ev.position,
                                        ev.window_position,
                                        ev.timestamp_ns,
                                    );
                                    (G::Began, TouchResponse::CLAIMED)
                                } else {
                                    (G::Possible, TouchResponse::CONSUMED)
                                }
                            } else {
                                // Along the scroll axis → it's a scroll. Abandon
                                // and leave the touch to the native scroller
                                // (a cross-axis drag must hold first). The hold
                                // timer is moot now.
                                self.cancel_timer();
                                *self.state.borrow_mut() = State::Rejected { id };
                                (G::Failed, TouchResponse::CONSUMED)
                            }
                        }
                    }
                }
            }
            (TouchPhase::Moved, State::Active {
                id,
                start,
                last_view,
                last_ts_ns,
                velocity: old_v,
            }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let frame_dx = ev.position.x - last_view.x;
                let frame_dy = ev.position.y - last_view.y;
                let dt_sec = if ev.timestamp_ns > last_ts_ns {
                    ((ev.timestamp_ns - last_ts_ns) as f32) / 1_000_000_000.0
                } else {
                    1.0 / 60.0
                }
                .max(0.001);
                let a = DRAG_VELOCITY_SMOOTHING;
                let new_v = TouchPoint::new(
                    old_v.x * (1.0 - a) + (frame_dx / dt_sec) * a,
                    old_v.y * (1.0 - a) + (frame_dy / dt_sec) * a,
                );
                *self.state.borrow_mut() = State::Active {
                    id,
                    start,
                    last_view: ev.position,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: new_v,
                };
                (self.on_drag)(DragPhase::Moved(DragSample {
                    view_position: ev.position,
                    window_position: ev.window_position,
                    delta: TouchPoint::new(dx, dy),
                    velocity: new_v,
                }));
                (G::Changed, TouchResponse::CLAIMED)
            }
            // Moved while PendingCommit: the commit happens on poll, but keep
            // ownership and refresh the stashed position.
            (TouchPhase::Moved, State::PendingCommit { id, .. }) if id == ev.id => {
                *self.state.borrow_mut() = State::PendingCommit {
                    id,
                    view: ev.position,
                    window: ev.window_position,
                };
                (G::Possible, TouchResponse::CONSUMED)
            }
            (TouchPhase::Moved, State::Rejected { id }) if id == ev.id => {
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Moved, _) => (self.state(), TouchResponse::IGNORED),

            // ---- Ended -------------------------------------------------
            (TouchPhase::Ended, State::Active { id, velocity, .. }) if id == ev.id => {
                self.cancel_timer();
                *self.state.borrow_mut() = State::Idle;
                (self.on_drag)(DragPhase::Ended {
                    window_position: ev.window_position,
                    velocity,
                });
                (G::Recognized, TouchResponse::CONSUMED)
            }
            (TouchPhase::Ended, State::Tracking { id, .. })
            | (TouchPhase::Ended, State::PendingCommit { id, .. })
            | (TouchPhase::Ended, State::Rejected { id })
                if id == ev.id =>
            {
                // Lifted before committing — never a drag.
                self.cancel_timer();
                *self.state.borrow_mut() = State::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Ended, _) => (self.state(), TouchResponse::IGNORED),

            // ---- Cancelled ---------------------------------------------
            (TouchPhase::Cancelled, State::Active { id, .. }) if id == ev.id => {
                self.cancel_timer();
                *self.state.borrow_mut() = State::Idle;
                (self.on_drag)(DragPhase::Cancelled);
                (G::Cancelled, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, State::Tracking { id, .. })
            | (TouchPhase::Cancelled, State::PendingCommit { id, .. })
            | (TouchPhase::Cancelled, State::Rejected { id })
                if id == ev.id =>
            {
                self.cancel_timer();
                *self.state.borrow_mut() = State::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, _) => (self.state(), TouchResponse::IGNORED),
        };
        // Any terminal transition lands in `Idle`; release the captured claim
        // closure there so its retained native view doesn't outlive the gesture.
        if matches!(*self.state.borrow(), State::Idle) {
            *self.claim.borrow_mut() = None;
        }
        RecognizerUpdate::new(state, response)
    }
}

impl DragRecognizer {
    /// Build the standalone [`TouchHandler`] for a view's `on_touch` slot.
    /// Installs an ungated notifier that polls immediately, so a long-press
    /// commit fires on its scheduler tick exactly like the arbiter path.
    pub fn into_handler(self) -> TouchHandler {
        let rec = Rc::new(RefCell::new(self));
        // Weak so the notifier → recognizer → notifier cycle can't leak
        // (see [[feedback_no_forget_in_library_code]]).
        let weak = Rc::downgrade(&rec);
        rec.borrow_mut().set_async_notifier(Rc::new(move || {
            if let Some(r) = weak.upgrade() {
                r.borrow_mut().poll_async(&RecognizerCtx::UNGATED);
            }
        }));
        Rc::new(move |ev: &TouchEvent| -> TouchResponse {
            rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
        })
    }
}
