//! Stock gesture recognizers built on the raw [`TouchEvent`] stream.
//!
//! Each recognizer is a finite-state machine implementing the
//! [`Recognizer`](crate::touch::recognizer::Recognizer) trait: `Tap`,
//! `LongPress`, `Pan`, `Pinch`. Two ways to use one:
//!
//! - **Standalone** — the `tap` / `long_press` / `pan` / `pinch` factory
//!   functions wrap a recognizer in a `TouchHandler` for a single view's
//!   `on_touch` slot. State lives behind `Rc<RefCell<…>>`, so multiple
//!   primitives can share a factory without entangling state. A standalone
//!   recognizer is [`RecognizerCtx::UNGATED`] — it recognizes
//!   optimistically: once a finger lands, the touch is owned through
//!   `Ended` / `Cancelled`.
//! - **Composed** — hand the `Recognizer` to the `gesture` SDK's
//!   `GestureGroup`, which drives several against one slot and resolves
//!   priority, require-to-fail, and simultaneous recognition. The gating
//!   contract that makes that work lives on the trait; see its docs and
//!   `docs/gesture-arbiter-plan.md`.
//!
//! `Tap` / `LongPress` / `Pan` match a single concurrent finger; `Pinch`
//! tracks a pair. Further multi-finger recognizers (rotate, two-finger
//! pan) implement the same trait.

use std::cell::RefCell;
use std::rc::Rc;

use crate::scheduling::{after_ms, ScheduledTask};
use crate::touch::recognizer::{
    AsyncNotifier, GestureState, Recognizer, RecognizerCtx, RecognizerKind, RecognizerUpdate,
};
use crate::touch::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint, TouchResponse};

// ---------------------------------------------------------------------------
// Tap
// ---------------------------------------------------------------------------

/// Maximum movement (in CSS pixels) between `Began` and `Ended` for
/// a single-finger interaction to count as a tap. Matches the slop
/// real native frameworks use — UIKit's tap recognizers reject
/// around 10pt, Material's `ViewConfiguration.getScaledTouchSlop`
/// returns roughly the same on mdpi.
pub const DEFAULT_TAP_SLOP_PX: f32 = 10.0;

/// Maximum elapsed time (ms) between `Began` and `Ended` for a
/// release to count as a tap. Longer holds are not taps; if you
/// want them to mean something, install a [`LongPressRecognizer`]
/// alongside.
pub const DEFAULT_TAP_MAX_DURATION_MS: u64 = 750;

/// Configuration for [`tap`]. Construct via [`TapRecognizer::new`]
/// and customize with the builder setters; pass to [`tap`] together
/// with the callback to produce an installable [`TouchHandler`].
#[derive(Clone, Copy, Debug)]
pub struct TapRecognizer {
    pub slop_px: f32,
    pub max_duration_ms: u64,
}

impl Default for TapRecognizer {
    fn default() -> Self {
        Self {
            slop_px: DEFAULT_TAP_SLOP_PX,
            max_duration_ms: DEFAULT_TAP_MAX_DURATION_MS,
        }
    }
}

impl TapRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn slop_px(mut self, v: f32) -> Self {
        self.slop_px = v;
        self
    }
    pub fn max_duration_ms(mut self, v: u64) -> Self {
        self.max_duration_ms = v;
        self
    }
}

#[derive(Clone, Copy)]
enum TapState {
    Idle,
    /// One finger is down and still a tap candidate.
    Tracking {
        id: TouchId,
        start: TouchPoint,
        start_ts_ns: u64,
    },
    /// The candidate failed (slop or timeout exceeded) but the
    /// finger is still down. We keep ownership through `Ended` so
    /// the touch doesn't leak back to the bubble dispatcher
    /// mid-interaction.
    Rejected { id: TouchId },
}

/// Single-finger tap recognizer ([`Recognizer`] impl).
///
/// Recognizes once per qualifying touch: `Began` → `Ended` with total
/// movement ≤ `slop_px` and duration ≤ `max_duration_ms`. Movement past
/// slop or duration past timeout marks the interaction failed; subsequent
/// events still consume (the recognizer keeps ownership of the touch) but
/// it never recognizes.
pub struct Tap {
    config: TapRecognizer,
    on_tap: Box<dyn Fn()>,
    state: TapState,
}

impl Tap {
    /// Construct from config + callback. Prefer [`tap`] for the
    /// standalone `on_touch` case; construct directly when handing the
    /// recognizer to a `GestureGroup`.
    pub fn new<F: Fn() + 'static>(config: TapRecognizer, on_tap: F) -> Self {
        Self {
            config,
            on_tap: Box::new(on_tap),
            state: TapState::Idle,
        }
    }
}

impl Recognizer for Tap {
    fn name(&self) -> &'static str {
        "tap"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Discrete
    }
    fn state(&self) -> GestureState {
        match self.state {
            TapState::Idle | TapState::Tracking { .. } => GestureState::Possible,
            TapState::Rejected { .. } => GestureState::Failed,
        }
    }
    fn reset(&mut self, _cancelled: bool) {
        // Tap is discrete and never `is_active`, so a cancel has no
        // callback to surface — just return to resting.
        self.state = TapState::Idle;
    }
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let config = self.config;
        let (state, response): (GestureState, TouchResponse) = match (ev.phase, self.state) {
            // New touch begins while idle: start tracking.
            (TouchPhase::Began, TapState::Idle) => {
                self.state = TapState::Tracking {
                    id: ev.id,
                    start: ev.position,
                    start_ts_ns: ev.timestamp_ns,
                };
                (G::Possible, TouchResponse::CONSUMED)
            }
            // A second finger lands while we're tracking the first.
            // Single-finger recognizer ignores extras — return
            // unconsumed so an outer handler (or another recognizer
            // sibling) can pick them up.
            (TouchPhase::Began, _) => (self.state(), TouchResponse::IGNORED),

            // Movement on the tracked finger: re-check slop / timeout.
            (TouchPhase::Moved, TapState::Tracking { id, start, start_ts_ns }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let exceeded_slop = (dx * dx + dy * dy) > config.slop_px * config.slop_px;
                let exceeded_time = ev.timestamp_ns.saturating_sub(start_ts_ns)
                    > config.max_duration_ms * 1_000_000;
                if exceeded_slop || exceeded_time {
                    self.state = TapState::Rejected { id };
                    (G::Failed, TouchResponse::CONSUMED)
                } else {
                    (G::Possible, TouchResponse::CONSUMED)
                }
            }
            // Movement on the tracked finger after rejection: keep
            // ownership but no further state change.
            (TouchPhase::Moved, TapState::Rejected { id }) if id == ev.id => {
                (G::Failed, TouchResponse::CONSUMED)
            }
            // Movement we don't care about (foreign id while tracking,
            // or events with no active state). Don't consume.
            (TouchPhase::Moved, _) => (self.state(), TouchResponse::IGNORED),

            // Release on the tracked finger: recognize if still tracking
            // AND the final event also passes slop / timeout AND the
            // arbiter permits it (require-to-fail gate).
            (TouchPhase::Ended, TapState::Tracking { id, start, start_ts_ns }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let within_slop = (dx * dx + dy * dy) <= config.slop_px * config.slop_px;
                let within_time = ev.timestamp_ns.saturating_sub(start_ts_ns)
                    <= config.max_duration_ms * 1_000_000;
                self.state = TapState::Idle;
                if within_slop && within_time && ctx.may_recognize {
                    (self.on_tap)();
                    (G::Recognized, TouchResponse::CONSUMED)
                } else {
                    // Out of bounds, or gated by a prerequisite that
                    // recognized — this interaction never taps.
                    (G::Failed, TouchResponse::CONSUMED)
                }
            }
            (TouchPhase::Ended, TapState::Rejected { id }) if id == ev.id => {
                self.state = TapState::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Ended, _) => (self.state(), TouchResponse::IGNORED),

            // Cancellation: reset, never recognize.
            (TouchPhase::Cancelled, TapState::Tracking { id, .. })
            | (TouchPhase::Cancelled, TapState::Rejected { id })
                if id == ev.id =>
            {
                self.state = TapState::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, _) => (self.state(), TouchResponse::IGNORED),
        };
        RecognizerUpdate::new(state, response)
    }
}

/// Build a single-finger tap [`TouchHandler`] for a view's `on_touch`
/// slot. Wraps a [`Tap`] recognizer, ungated.
///
/// Fires `on_tap` once per qualifying touch: `Began` → `Ended` with
/// total movement ≤ `slop_px` and duration ≤ `max_duration_ms`.
///
/// ```ignore
/// view(children!(...))
///     .on_touch(tap(TapRecognizer::new(), || println!("tapped")))
/// ```
pub fn tap<F: Fn() + 'static>(config: TapRecognizer, on_tap: F) -> TouchHandler {
    let rec = Rc::new(RefCell::new(Tap::new(config, on_tap)));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

// ---------------------------------------------------------------------------
// Pan
// ---------------------------------------------------------------------------

/// Minimum movement (CSS pixels) the finger must travel before the
/// pan becomes "active" (fires `PanEvent::Began`). Below this,
/// `Moved` is silently absorbed so a small finger wobble during a
/// tap doesn't unintentionally start a pan. Matches the iOS
/// `UIPanGestureRecognizer` default slop.
pub const DEFAULT_PAN_SLOP_PX: f32 = 8.0;

/// EMA mixing factor for velocity smoothing. Higher = more weight
/// on the latest sample. 0.6 is the same constant the wgpu
/// scroll-pan code uses.
const PAN_VELOCITY_SMOOTHING: f32 = 0.6;

/// Configuration for [`pan`]. Construct via [`PanRecognizer::new`]
/// and customize with the builder setters.
#[derive(Clone, Copy, Debug)]
pub struct PanRecognizer {
    pub slop_px: f32,
}

impl Default for PanRecognizer {
    fn default() -> Self {
        Self {
            slop_px: DEFAULT_PAN_SLOP_PX,
        }
    }
}

impl PanRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn slop_px(mut self, v: f32) -> Self {
        self.slop_px = v;
        self
    }
}

/// Lifecycle events fired by [`pan`]. The recognizer's callback
/// receives one of these per significant phase change.
#[derive(Clone, Copy, Debug)]
pub enum PanEvent {
    /// Pan started — slop has been exceeded. `position` is the
    /// touch's view-local position when the threshold was crossed.
    /// The handler typically stashes the current value being
    /// dragged so subsequent `Moved` events can offset from it.
    Began { position: TouchPoint },
    /// Pan in progress. `delta` is total movement from the
    /// `Began`-time position (NOT incremental from the previous
    /// `Moved` — handlers use it directly as the offset to apply).
    /// `velocity` is in pixels-per-second, smoothed via an
    /// exponential moving average so single-frame jitter doesn't
    /// produce a wildly different value.
    Moved {
        position: TouchPoint,
        delta: TouchPoint,
        velocity: TouchPoint,
    },
    /// User released the finger after the pan was active. The
    /// final `velocity` lets handlers kick off momentum / fling
    /// animations.
    Ended { velocity: TouchPoint },
    /// Pan was interrupted by the platform (incoming call, system
    /// gesture, view detach). Handlers should reset / animate back
    /// to the resting state.
    Cancelled,
}

/// `Copy` so the state machine can be wrapped in a `Cell` instead
/// of a `RefCell` — keeping the match arms borrow-free avoids the
/// classic "immutable borrow active in match scrutinee, mutable
/// borrow inside arm" RefCell panic. All fields are scalar / Copy
/// (`TouchId`, `TouchPoint`, `u64`).
#[derive(Clone, Copy)]
enum PanState {
    /// No finger down (or pan never went active).
    Idle,
    /// Finger down but slop not yet exceeded. No `Began` has
    /// fired; if the finger lifts here, nothing fires.
    Tracking {
        id: TouchId,
        start: TouchPoint,
    },
    /// Pan active — `Began` already fired. Each subsequent `Moved`
    /// fires `PanEvent::Moved` and updates the velocity estimate.
    Active {
        id: TouchId,
        start: TouchPoint,
        last_position: TouchPoint,
        last_ts_ns: u64,
        velocity: TouchPoint,
    },
}

/// Single-finger pan recognizer ([`Recognizer`] impl).
///
/// State machine:
/// - `Began` → start tracking (no callback yet).
/// - First `Moved` past `slop_px` (and `ctx.may_recognize`) → fire
///   `PanEvent::Began` then an immediate `Moved` with `delta = (0,0)`;
///   transition to active.
/// - Subsequent `Moved` → `PanEvent::Moved` with cumulative delta +
///   smoothed velocity.
/// - `Ended` while active → `PanEvent::Ended { velocity }`.
/// - `Cancelled` / arbiter cancel while active → `PanEvent::Cancelled`.
///
/// Returns `CONSUMED` while tracking (owns the touch, hasn't preempted
/// anything) and `CLAIMED` once active (suppress parent scroll containers
/// via the backend claim protocol). Single-finger only; secondary touches
/// return `IGNORED` so a pinch recognizer can pick them up.
pub struct Pan {
    config: PanRecognizer,
    on_pan: Box<dyn Fn(&PanEvent)>,
    state: PanState,
}

impl Pan {
    pub fn new<F: Fn(&PanEvent) + 'static>(config: PanRecognizer, on_pan: F) -> Self {
        Self {
            config,
            on_pan: Box::new(on_pan),
            state: PanState::Idle,
        }
    }
}

impl Recognizer for Pan {
    fn name(&self) -> &'static str {
        "pan"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Continuous
    }
    fn state(&self) -> GestureState {
        match self.state {
            PanState::Idle | PanState::Tracking { .. } => GestureState::Possible,
            PanState::Active { .. } => GestureState::Changed,
        }
    }
    fn reset(&mut self, cancelled: bool) {
        if cancelled && matches!(self.state, PanState::Active { .. }) {
            (self.on_pan)(&PanEvent::Cancelled);
        }
        self.state = PanState::Idle;
    }
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let config = self.config;
        let (state, response): (GestureState, TouchResponse) = match (ev.phase, self.state) {
            (TouchPhase::Began, PanState::Idle) => {
                self.state = PanState::Tracking {
                    id: ev.id,
                    start: ev.position,
                };
                (G::Possible, TouchResponse::CONSUMED)
            }
            (TouchPhase::Began, _) => (self.state(), TouchResponse::IGNORED),

            (TouchPhase::Moved, PanState::Tracking { id, start }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let dist2 = dx * dx + dy * dy;
                // Past slop AND permitted to begin: go active. If gated
                // (a prerequisite hasn't failed yet), stay tracking and
                // re-check next Moved — slop is cumulative, so we re-detect.
                if dist2 > config.slop_px * config.slop_px && ctx.may_recognize {
                    self.state = PanState::Active {
                        id: ev.id,
                        start,
                        last_position: ev.position,
                        last_ts_ns: ev.timestamp_ns,
                        velocity: TouchPoint::ZERO,
                    };
                    (self.on_pan)(&PanEvent::Began { position: ev.position });
                    (self.on_pan)(&PanEvent::Moved {
                        position: ev.position,
                        delta: TouchPoint::new(dx, dy),
                        velocity: TouchPoint::ZERO,
                    });
                    (G::Began, TouchResponse::CLAIMED)
                } else {
                    (G::Possible, TouchResponse::CONSUMED)
                }
            }
            (TouchPhase::Moved, PanState::Active {
                id,
                start,
                last_position,
                last_ts_ns,
                velocity: old_velocity,
            }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let frame_dx = ev.position.x - last_position.x;
                let frame_dy = ev.position.y - last_position.y;
                let dt_sec = if ev.timestamp_ns > last_ts_ns {
                    ((ev.timestamp_ns - last_ts_ns) as f32) / 1_000_000_000.0
                } else {
                    1.0 / 60.0
                };
                // Floor dt to avoid wild velocities on same-frame
                // events. 1ms floor matches the wgpu scroll-pan
                // velocity math.
                let dt_sec = dt_sec.max(0.001);
                let raw_vx = frame_dx / dt_sec;
                let raw_vy = frame_dy / dt_sec;
                let a = PAN_VELOCITY_SMOOTHING;
                let new_velocity = TouchPoint::new(
                    old_velocity.x * (1.0 - a) + raw_vx * a,
                    old_velocity.y * (1.0 - a) + raw_vy * a,
                );
                self.state = PanState::Active {
                    id: ev.id,
                    start,
                    last_position: ev.position,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: new_velocity,
                };
                (self.on_pan)(&PanEvent::Moved {
                    position: ev.position,
                    delta: TouchPoint::new(dx, dy),
                    velocity: new_velocity,
                });
                (G::Changed, TouchResponse::CLAIMED)
            }
            (TouchPhase::Moved, _) => (self.state(), TouchResponse::IGNORED),

            (TouchPhase::Ended, PanState::Tracking { id, .. }) if id == ev.id => {
                // Never crossed slop — no Began fired, never recognized.
                self.state = PanState::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Ended, PanState::Active { id, velocity, .. }) if id == ev.id => {
                self.state = PanState::Idle;
                (self.on_pan)(&PanEvent::Ended { velocity });
                (G::Recognized, TouchResponse::CONSUMED)
            }
            (TouchPhase::Ended, _) => (self.state(), TouchResponse::IGNORED),

            (TouchPhase::Cancelled, PanState::Tracking { id, .. }) if id == ev.id => {
                self.state = PanState::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, PanState::Active { id, .. }) if id == ev.id => {
                self.state = PanState::Idle;
                (self.on_pan)(&PanEvent::Cancelled);
                (G::Cancelled, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, _) => (self.state(), TouchResponse::IGNORED),
        };
        RecognizerUpdate::new(state, response)
    }
}

/// Build a single-finger pan [`TouchHandler`] for a view's `on_touch`
/// slot. Wraps a [`Pan`] recognizer, ungated. See [`Pan`] for the state
/// machine and the `PanEvent` lifecycle.
pub fn pan<F: Fn(&PanEvent) + 'static>(config: PanRecognizer, on_pan: F) -> TouchHandler {
    let rec = Rc::new(RefCell::new(Pan::new(config, on_pan)));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

// ---------------------------------------------------------------------------
// Long press
// ---------------------------------------------------------------------------

/// How long (ms) the finger must stay still inside the slop radius
/// before the long-press fires. Matches UIKit
/// `UILongPressGestureRecognizer.minimumPressDuration` (0.5s) and
/// Android `ViewConfiguration.getLongPressTimeout` (~500ms).
pub const DEFAULT_LONG_PRESS_THRESHOLD_MS: u64 = 500;

/// Maximum movement (CSS pixels) during the wait — past this and
/// the press is rejected. Same value as the tap default; the user's
/// finger drifting more than this counts as "they meant to drag."
pub const DEFAULT_LONG_PRESS_SLOP_PX: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
pub struct LongPressRecognizer {
    pub threshold_ms: u64,
    pub slop_px: f32,
}

impl Default for LongPressRecognizer {
    fn default() -> Self {
        Self {
            threshold_ms: DEFAULT_LONG_PRESS_THRESHOLD_MS,
            slop_px: DEFAULT_LONG_PRESS_SLOP_PX,
        }
    }
}

impl LongPressRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn threshold_ms(mut self, v: u64) -> Self {
        self.threshold_ms = v;
        self
    }
    pub fn slop_px(mut self, v: f32) -> Self {
        self.slop_px = v;
        self
    }
}

enum LongPressState {
    Idle,
    /// Finger down, timer armed. `timer` lives until either it
    /// fires (which transitions us to `PendingFire`) or we cancel it on
    /// release / slop-exceed / cancellation.
    Tracking {
        id: TouchId,
        start: TouchPoint,
        timer: ScheduledTask,
    },
    /// Timer elapsed off the touch stream. The recognizer wants to
    /// recognize but has not yet — it waits for the driver to call
    /// [`Recognizer::poll_async`], which fires the callback subject to
    /// the require-to-fail gate. Standalone use polls immediately, so
    /// this state is transient; under the arbiter it can persist while a
    /// prerequisite is still resolving.
    PendingFire { id: TouchId },
    /// Recognized — `on_long_press` delivered. Keep ownership through
    /// `Ended` so the touch doesn't reflow into the bubble dispatcher.
    Fired { id: TouchId },
    /// Slop exceeded before the timer fired — recognizer dropped the
    /// gesture but keeps the finger to stay coherent.
    Rejected { id: TouchId },
}

/// Single-finger long-press recognizer ([`Recognizer`] impl).
///
/// Recognizes once per qualifying touch: the finger stays within
/// `slop_px` of its `Began` position for at least `threshold_ms`.
/// Movement past slop before the timer fires marks the gesture rejected;
/// release before the timer fires cancels silently.
///
/// Unlike the other stock recognizers this one recognizes **off the touch
/// stream** (on a scheduler tick via [`crate::scheduling::after_ms`]).
/// When the timer elapses it does not fire unilaterally — it enters
/// [`LongPressState::PendingFire`] and calls its [`AsyncNotifier`], and
/// the driver re-polls via [`Recognizer::poll_async`]. Standalone use
/// ([`long_press`]) installs a notifier that polls immediately and
/// ungated, so behaviour matches a direct fire; the arbiter installs one
/// that re-runs arbitration so a long-press can be gated / cancelled by a
/// competitor.
pub struct LongPress {
    config: LongPressRecognizer,
    on_long_press: Rc<dyn Fn()>,
    state: Rc<RefCell<LongPressState>>,
    notifier: Rc<RefCell<Option<AsyncNotifier>>>,
}

impl LongPress {
    pub fn new<F: Fn() + 'static>(config: LongPressRecognizer, on_long_press: F) -> Self {
        Self {
            config,
            on_long_press: Rc::new(on_long_press),
            state: Rc::new(RefCell::new(LongPressState::Idle)),
            notifier: Rc::new(RefCell::new(None)),
        }
    }

    fn map_state(s: &LongPressState) -> GestureState {
        match s {
            LongPressState::Idle
            | LongPressState::Tracking { .. }
            | LongPressState::PendingFire { .. } => GestureState::Possible,
            LongPressState::Fired { .. } => GestureState::Recognized,
            LongPressState::Rejected { .. } => GestureState::Failed,
        }
    }
}

impl Recognizer for LongPress {
    fn name(&self) -> &'static str {
        "long_press"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Discrete
    }
    fn state(&self) -> GestureState {
        Self::map_state(&self.state.borrow())
    }
    fn set_async_notifier(&mut self, notifier: AsyncNotifier) {
        *self.notifier.borrow_mut() = Some(notifier);
    }
    fn reset(&mut self, _cancelled: bool) {
        // Discrete and has no `Cancelled` callback; dropping a `Tracking`
        // state drops (and thus cancels) any armed timer.
        *self.state.borrow_mut() = LongPressState::Idle;
    }
    fn poll_async(&mut self, ctx: &RecognizerCtx) -> Option<RecognizerUpdate> {
        let pending_id = match &*self.state.borrow() {
            LongPressState::PendingFire { id } => Some(*id),
            _ => None,
        };
        let id = pending_id?;
        if ctx.may_recognize {
            *self.state.borrow_mut() = LongPressState::Fired { id };
            (self.on_long_press)();
            Some(RecognizerUpdate::new(
                GestureState::Recognized,
                TouchResponse::CONSUMED,
            ))
        } else {
            // Gated by an unresolved prerequisite: stay pending. A later
            // re-arbitration (when the prerequisite fails) re-polls us.
            Some(RecognizerUpdate::new(
                GestureState::Possible,
                TouchResponse::CONSUMED,
            ))
        }
    }
    fn update(&mut self, ev: &TouchEvent, _ctx: &RecognizerCtx) -> RecognizerUpdate {
        let ev_id = ev.id;
        let config = self.config;
        let response = match ev.phase {
            TouchPhase::Began => {
                // Single-finger: ignore extras while we're armed.
                if !matches!(*self.state.borrow(), LongPressState::Idle) {
                    TouchResponse::IGNORED
                } else {
                    let timer = {
                        let state = self.state.clone();
                        let notifier = self.notifier.clone();
                        let id = ev_id;
                        after_ms(config.threshold_ms as i32, move || {
                            // Still tracking the same id → arm a pending
                            // recognition and notify the driver. Release
                            // the borrow before calling out.
                            let notify = {
                                let mut s = state.borrow_mut();
                                if let LongPressState::Tracking { id: cur, .. } = *s {
                                    if cur == id {
                                        *s = LongPressState::PendingFire { id };
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            };
                            if notify {
                                let n = notifier.borrow().clone();
                                if let Some(n) = n {
                                    n();
                                }
                            }
                        })
                    };
                    *self.state.borrow_mut() = LongPressState::Tracking {
                        id: ev_id,
                        start: ev.position,
                        timer,
                    };
                    TouchResponse::CONSUMED
                }
            }
            TouchPhase::Moved => {
                let mut s = self.state.borrow_mut();
                match &mut *s {
                    LongPressState::Tracking { id, start, timer } if *id == ev_id => {
                        let dx = ev.position.x - start.x;
                        let dy = ev.position.y - start.y;
                        if (dx * dx + dy * dy) > config.slop_px * config.slop_px {
                            timer.cancel();
                            let id = *id;
                            *s = LongPressState::Rejected { id };
                        }
                        TouchResponse::CONSUMED
                    }
                    LongPressState::PendingFire { id }
                    | LongPressState::Fired { id }
                    | LongPressState::Rejected { id }
                        if *id == ev_id =>
                    {
                        TouchResponse::CONSUMED
                    }
                    _ => TouchResponse::IGNORED,
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let mut s = self.state.borrow_mut();
                match &mut *s {
                    LongPressState::Tracking { id, timer, .. } if *id == ev_id => {
                        timer.cancel();
                        *s = LongPressState::Idle;
                        // Released before the timer — never recognized.
                        return RecognizerUpdate::new(
                            GestureState::Failed,
                            TouchResponse::CONSUMED,
                        );
                    }
                    LongPressState::PendingFire { id } if *id == ev_id => {
                        // Timer elapsed but recognition was never granted
                        // (gated, or no poll yet) and the finger lifted.
                        *s = LongPressState::Idle;
                        return RecognizerUpdate::new(
                            GestureState::Failed,
                            TouchResponse::CONSUMED,
                        );
                    }
                    LongPressState::Fired { id } if *id == ev_id => {
                        *s = LongPressState::Idle;
                        return RecognizerUpdate::new(
                            GestureState::Recognized,
                            TouchResponse::CONSUMED,
                        );
                    }
                    LongPressState::Rejected { id } if *id == ev_id => {
                        *s = LongPressState::Idle;
                        return RecognizerUpdate::new(
                            GestureState::Failed,
                            TouchResponse::CONSUMED,
                        );
                    }
                    _ => TouchResponse::IGNORED,
                }
            }
        };
        RecognizerUpdate::new(self.state(), response)
    }
}

/// Build a single-finger long-press [`TouchHandler`] for a view's
/// `on_touch` slot. Wraps a [`LongPress`] recognizer with a notifier that
/// polls immediately and ungated, so the timer fires `on_long_press`
/// directly on its scheduler tick — identical to the pre-trait behaviour.
pub fn long_press<F: Fn() + 'static>(
    config: LongPressRecognizer,
    on_long_press: F,
) -> TouchHandler {
    let rec = Rc::new(RefCell::new(LongPress::new(config, on_long_press)));
    // Weak so the notifier closure → recognizer → notifier cycle can't
    // leak (see [[feedback_no_forget_in_library_code]]).
    let weak = Rc::downgrade(&rec);
    rec.borrow_mut()
        .set_async_notifier(Rc::new(move || {
            if let Some(r) = weak.upgrade() {
                r.borrow_mut().poll_async(&RecognizerCtx::UNGATED);
            }
        }));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

// ---------------------------------------------------------------------------
// Pinch (two-finger zoom)
// ---------------------------------------------------------------------------

/// Minimum change (CSS pixels) in the distance between the two fingers
/// before the pinch becomes active and fires [`PinchEvent::Began`]. Below
/// this, finger jitter or the opening moments of a two-finger *pan* (both
/// fingers translating together, distance roughly constant) don't spuriously
/// start a zoom.
pub const DEFAULT_PINCH_SLOP_PX: f32 = 6.0;

/// EMA mixing factor for scale-velocity smoothing — same constant the pan
/// recognizer uses for positional velocity.
const PINCH_VELOCITY_SMOOTHING: f32 = 0.6;

/// Configuration for [`pinch`]. Construct via [`PinchRecognizer::new`] and
/// customize with the builder setters.
#[derive(Clone, Copy, Debug)]
pub struct PinchRecognizer {
    pub slop_px: f32,
}

impl Default for PinchRecognizer {
    fn default() -> Self {
        Self {
            slop_px: DEFAULT_PINCH_SLOP_PX,
        }
    }
}

impl PinchRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn slop_px(mut self, v: f32) -> Self {
        self.slop_px = v;
        self
    }
}

/// Lifecycle events fired by [`pinch`]. Scale is **cumulative relative to
/// the two-finger-down distance**, mirroring how [`PanEvent`]'s delta is
/// cumulative from the finger-down position — handlers multiply it onto a
/// value snapshotted at [`PinchEvent::Began`].
#[derive(Clone, Copy, Debug)]
pub enum PinchEvent {
    /// Two fingers are down and the distance between them has moved past
    /// slop. `focus` is the midpoint of the two fingers in view-local
    /// coordinates — the natural point to zoom about.
    Began { focus: TouchPoint },
    /// Pinch in progress. `scale` is the cumulative factor relative to the
    /// two-finger-down distance (`1.0` = unchanged, `2.0` = fingers twice as
    /// far apart, `0.5` = half). `focus` is the current midpoint.
    /// `velocity` is in scale-units per second, EMA-smoothed so a single
    /// jittery frame doesn't spike it.
    Changed {
        focus: TouchPoint,
        scale: f32,
        velocity: f32,
    },
    /// One of the two fingers lifted after the pinch was active. The final
    /// `velocity` lets handlers fling the zoom to a momentum settle.
    Ended { velocity: f32 },
    /// Pinch interrupted by the platform (incoming call, system gesture,
    /// view detach). Handlers should reset / animate back to rest.
    Cancelled,
}

/// One tracked finger. `Copy` so [`PinchState`] stays `Copy` and can live in
/// a `Cell` (see the note on [`PanState`]).
#[derive(Clone, Copy)]
struct PinchFinger {
    id: TouchId,
    pos: TouchPoint,
}

#[derive(Clone, Copy)]
enum PinchState {
    /// Fewer than two fingers down.
    Idle,
    /// Exactly one finger tracked; waiting for a second to form the pair.
    One { a: PinchFinger },
    /// Two fingers tracked. `active` flips true once `|cur_dist - start_dist|`
    /// crosses slop; before that no callback has fired. `scale` is always
    /// computed as `cur_dist / start_dist`.
    Two {
        a: PinchFinger,
        b: PinchFinger,
        start_dist: f32,
        active: bool,
        last_scale: f32,
        last_ts_ns: u64,
        velocity: f32,
    },
}

fn pinch_dist(a: TouchPoint, b: TouchPoint) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

fn pinch_midpoint(a: TouchPoint, b: TouchPoint) -> TouchPoint {
    TouchPoint::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

/// Two-finger pinch recognizer ([`Recognizer`] impl).
///
/// State machine:
/// - First finger down → track it but **return [`TouchResponse::IGNORED`]**,
///   so a single-finger tap / pan recognizer on the same responder chain
///   still sees it. (Unlike [`Pan`], pinch only cares about two fingers, so
///   it doesn't claim ownership of a lone touch.)
/// - Second finger down → record the start distance; still inactive.
/// - First `Moved` whose `|cur_dist - start_dist|` exceeds `slop_px` (and
///   `ctx.may_recognize`) → fire [`PinchEvent::Began`] then an immediate
///   [`PinchEvent::Changed`], and switch to returning
///   [`TouchResponse::CLAIMED`].
/// - Subsequent `Moved` → [`PinchEvent::Changed`] with cumulative scale +
///   smoothed velocity.
/// - Either finger lifts while active → [`PinchEvent::Ended`]; the other
///   finger stays tracked so a fresh second touch can start a new pinch.
/// - `Cancelled` / arbiter cancel while active → [`PinchEvent::Cancelled`].
///
/// Because pinch lets a lone finger bubble (`IGNORED`), it composes
/// naturally with a single-finger [`Pan`] in a `GestureGroup` set
/// `allow_simultaneous` — the photo-viewer pan+zoom case.
pub struct Pinch {
    config: PinchRecognizer,
    on_pinch: Box<dyn Fn(&PinchEvent)>,
    state: PinchState,
}

impl Pinch {
    pub fn new<F: Fn(&PinchEvent) + 'static>(config: PinchRecognizer, on_pinch: F) -> Self {
        Self {
            config,
            on_pinch: Box::new(on_pinch),
            state: PinchState::Idle,
        }
    }
}

impl Recognizer for Pinch {
    fn name(&self) -> &'static str {
        "pinch"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Continuous
    }
    fn state(&self) -> GestureState {
        match self.state {
            PinchState::Two { active: true, .. } => GestureState::Changed,
            _ => GestureState::Possible,
        }
    }
    fn reset(&mut self, cancelled: bool) {
        if cancelled && matches!(self.state, PinchState::Two { active: true, .. }) {
            (self.on_pinch)(&PinchEvent::Cancelled);
        }
        self.state = PinchState::Idle;
    }
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let config = self.config;
        let cur = self.state;
        let (state, response): (GestureState, TouchResponse) = match ev.phase {
            TouchPhase::Began => match cur {
                PinchState::Idle => {
                    self.state = PinchState::One {
                        a: PinchFinger {
                            id: ev.id,
                            pos: ev.position,
                        },
                    };
                    (G::Possible, TouchResponse::IGNORED)
                }
                PinchState::One { a } if a.id != ev.id => {
                    let b = PinchFinger {
                        id: ev.id,
                        pos: ev.position,
                    };
                    // Floor start_dist so the scale division can never blow up
                    // if two pointers report the same coordinate on landing.
                    let start_dist = pinch_dist(a.pos, b.pos).max(0.0001);
                    self.state = PinchState::Two {
                        a,
                        b,
                        start_dist,
                        active: false,
                        last_scale: 1.0,
                        last_ts_ns: ev.timestamp_ns,
                        velocity: 0.0,
                    };
                    (G::Possible, TouchResponse::IGNORED)
                }
                // A third finger, or a duplicate id — single-pair recognizer
                // ignores extras.
                _ => (self.state(), TouchResponse::IGNORED),
            },

            TouchPhase::Moved => match cur {
                PinchState::One { mut a } if a.id == ev.id => {
                    a.pos = ev.position;
                    self.state = PinchState::One { a };
                    (G::Possible, TouchResponse::IGNORED)
                }
                PinchState::Two {
                    mut a,
                    mut b,
                    start_dist,
                    active,
                    last_scale,
                    last_ts_ns,
                    velocity,
                } => {
                    if a.id == ev.id {
                        a.pos = ev.position;
                    } else if b.id == ev.id {
                        b.pos = ev.position;
                    } else {
                        // Movement from a third finger we're not tracking.
                        return RecognizerUpdate::new(self.state(), TouchResponse::IGNORED);
                    }
                    let cur_dist = pinch_dist(a.pos, b.pos);
                    let scale = cur_dist / start_dist;
                    let focus = pinch_midpoint(a.pos, b.pos);
                    if !active {
                        // Begin only when slop is crossed AND the arbiter
                        // permits it. If gated, stay inactive and re-check
                        // next Moved (the |dist - start| test is cumulative).
                        if (cur_dist - start_dist).abs() > config.slop_px && ctx.may_recognize {
                            self.state = PinchState::Two {
                                a,
                                b,
                                start_dist,
                                active: true,
                                last_scale: scale,
                                last_ts_ns: ev.timestamp_ns,
                                velocity: 0.0,
                            };
                            (self.on_pinch)(&PinchEvent::Began { focus });
                            (self.on_pinch)(&PinchEvent::Changed {
                                focus,
                                scale,
                                velocity: 0.0,
                            });
                            (G::Began, TouchResponse::CLAIMED)
                        } else {
                            self.state = PinchState::Two {
                                a,
                                b,
                                start_dist,
                                active: false,
                                last_scale: scale,
                                last_ts_ns: ev.timestamp_ns,
                                velocity,
                            };
                            (G::Possible, TouchResponse::IGNORED)
                        }
                    } else {
                        let dt_sec = if ev.timestamp_ns > last_ts_ns {
                            ((ev.timestamp_ns - last_ts_ns) as f32) / 1_000_000_000.0
                        } else {
                            1.0 / 60.0
                        };
                        let dt_sec = dt_sec.max(0.001);
                        let raw_v = (scale - last_scale) / dt_sec;
                        let mix = PINCH_VELOCITY_SMOOTHING;
                        let new_velocity = velocity * (1.0 - mix) + raw_v * mix;
                        self.state = PinchState::Two {
                            a,
                            b,
                            start_dist,
                            active: true,
                            last_scale: scale,
                            last_ts_ns: ev.timestamp_ns,
                            velocity: new_velocity,
                        };
                        (self.on_pinch)(&PinchEvent::Changed {
                            focus,
                            scale,
                            velocity: new_velocity,
                        });
                        (G::Changed, TouchResponse::CLAIMED)
                    }
                }
                _ => (self.state(), TouchResponse::IGNORED),
            },

            TouchPhase::Ended | TouchPhase::Cancelled => {
                let is_cancel = matches!(ev.phase, TouchPhase::Cancelled);
                match cur {
                    PinchState::One { a } if a.id == ev.id => {
                        self.state = PinchState::Idle;
                        (G::Failed, TouchResponse::IGNORED)
                    }
                    PinchState::Two { a, b, active, velocity, .. }
                        if a.id == ev.id || b.id == ev.id =>
                    {
                        let remaining = if a.id == ev.id { b } else { a };
                        if active {
                            if is_cancel {
                                (self.on_pinch)(&PinchEvent::Cancelled);
                            } else {
                                (self.on_pinch)(&PinchEvent::Ended { velocity });
                            }
                        }
                        // A real lift (Ended) leaves the other finger down →
                        // demote to One so a new second touch can re-pinch.
                        // A Cancelled tears the whole interaction down.
                        if is_cancel {
                            self.state = PinchState::Idle;
                        } else {
                            self.state = PinchState::One { a: remaining };
                        }
                        match (active, is_cancel) {
                            (true, false) => (G::Recognized, TouchResponse::CONSUMED),
                            (true, true) => (G::Cancelled, TouchResponse::CONSUMED),
                            (false, _) => (G::Failed, TouchResponse::IGNORED),
                        }
                    }
                    _ => (self.state(), TouchResponse::IGNORED),
                }
            }
        };
        RecognizerUpdate::new(state, response)
    }
}

/// Build a two-finger pinch [`TouchHandler`] for a view's `on_touch`
/// slot. Wraps a [`Pinch`] recognizer, ungated.
///
/// Single-handler-per-view note: a view's `on_touch` slot holds one handler,
/// so install *either* a pan/tap recognizer *or* this — composing pan + pinch
/// on the same node is a `GestureGroup` (gesture SDK) concern.
pub fn pinch<F: Fn(&PinchEvent) + 'static>(config: PinchRecognizer, on_pinch: F) -> TouchHandler {
    let rec = Rc::new(RefCell::new(Pinch::new(config, on_pinch)));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

// ---------------------------------------------------------------------------
// Swipe (single-finger directional fling)
// ---------------------------------------------------------------------------

/// Minimum travel (CSS pixels) along the dominant axis between `Began` and
/// `Ended` for a flick to count as a swipe. Filters out a fast tap that
/// barely moves.
pub const DEFAULT_SWIPE_MIN_DISTANCE_PX: f32 = 24.0;

/// Minimum release speed (CSS pixels / second) along the dominant axis for
/// a swipe — this is what makes it a *flick* rather than a slow drag. A
/// slow, deliberate drag that ends below this speed is not a swipe (use a
/// [`Pan`] for that).
pub const DEFAULT_SWIPE_MIN_VELOCITY_PX_S: f32 = 300.0;

/// EMA mixing factor for swipe velocity — same constant the pan / pinch
/// recognizers use.
const SWIPE_VELOCITY_SMOOTHING: f32 = 0.6;

/// The four cardinal swipe directions, reported to the [`swipe`] callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Which directions a [`SwipeRecognizer`] will recognize. A swipe whose
/// dominant direction is not allowed *fails* (so, in a [`GestureGroup`], a
/// horizontal-only swipe lets a vertical scroll through). Defaults to all
/// four via [`SwipeDirs::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipeDirs {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
}

impl SwipeDirs {
    pub const ALL: Self = Self {
        left: true,
        right: true,
        up: true,
        down: true,
    };
    pub const HORIZONTAL: Self = Self {
        left: true,
        right: true,
        up: false,
        down: false,
    };
    pub const VERTICAL: Self = Self {
        left: false,
        right: false,
        up: true,
        down: true,
    };
    pub fn allows(self, dir: SwipeDirection) -> bool {
        match dir {
            SwipeDirection::Left => self.left,
            SwipeDirection::Right => self.right,
            SwipeDirection::Up => self.up,
            SwipeDirection::Down => self.down,
        }
    }
}

/// Configuration for [`swipe`] / [`Swipe`].
#[derive(Clone, Copy, Debug)]
pub struct SwipeRecognizer {
    pub min_distance_px: f32,
    pub min_velocity_px_s: f32,
    pub directions: SwipeDirs,
}

impl Default for SwipeRecognizer {
    fn default() -> Self {
        Self {
            min_distance_px: DEFAULT_SWIPE_MIN_DISTANCE_PX,
            min_velocity_px_s: DEFAULT_SWIPE_MIN_VELOCITY_PX_S,
            directions: SwipeDirs::ALL,
        }
    }
}

impl SwipeRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn min_distance_px(mut self, v: f32) -> Self {
        self.min_distance_px = v;
        self
    }
    pub fn min_velocity_px_s(mut self, v: f32) -> Self {
        self.min_velocity_px_s = v;
        self
    }
    pub fn directions(mut self, v: SwipeDirs) -> Self {
        self.directions = v;
        self
    }
}

#[derive(Clone, Copy)]
enum SwipeState {
    Idle,
    /// One finger down. We accumulate a smoothed velocity so the decision
    /// at `Ended` reflects the *release* speed, not the whole-gesture
    /// average.
    Tracking {
        id: TouchId,
        start: TouchPoint,
        last_position: TouchPoint,
        last_ts_ns: u64,
        velocity: TouchPoint,
    },
}

/// Single-finger directional swipe (flick) recognizer ([`Recognizer`]
/// impl). Discrete: recognizes once, on `Ended`, if the release speed and
/// travel along the dominant axis clear the thresholds and the direction
/// is allowed. Reports the [`SwipeDirection`].
///
/// Owns the touch while tracking (`CONSUMED`) but never claims — a swipe
/// observes the gesture without preempting a native scroller, so "swipe to
/// act" and scrolling coexist. Compose it with a scroll/pan in a
/// [`GestureGroup`] when they must arbitrate.
pub struct Swipe {
    config: SwipeRecognizer,
    on_swipe: Box<dyn Fn(SwipeDirection)>,
    state: SwipeState,
}

impl Swipe {
    pub fn new<F: Fn(SwipeDirection) + 'static>(config: SwipeRecognizer, on_swipe: F) -> Self {
        Self {
            config,
            on_swipe: Box::new(on_swipe),
            state: SwipeState::Idle,
        }
    }

    /// Decide the swipe outcome from the release kinematics. `None` =
    /// thresholds not met (the recognizer fails).
    fn classify(&self, start: TouchPoint, end: TouchPoint, velocity: TouchPoint) -> Option<SwipeDirection> {
        let horizontal = velocity.x.abs() >= velocity.y.abs();
        let (dir, dist, speed) = if horizontal {
            let dir = if velocity.x >= 0.0 {
                SwipeDirection::Right
            } else {
                SwipeDirection::Left
            };
            (dir, (end.x - start.x).abs(), velocity.x.abs())
        } else {
            let dir = if velocity.y >= 0.0 {
                SwipeDirection::Down
            } else {
                SwipeDirection::Up
            };
            (dir, (end.y - start.y).abs(), velocity.y.abs())
        };
        if dist >= self.config.min_distance_px
            && speed >= self.config.min_velocity_px_s
            && self.config.directions.allows(dir)
        {
            Some(dir)
        } else {
            None
        }
    }
}

impl Recognizer for Swipe {
    fn name(&self) -> &'static str {
        "swipe"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Discrete
    }
    fn state(&self) -> GestureState {
        GestureState::Possible
    }
    fn reset(&mut self, _cancelled: bool) {
        self.state = SwipeState::Idle;
    }
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let (state, response): (GestureState, TouchResponse) = match (ev.phase, self.state) {
            (TouchPhase::Began, SwipeState::Idle) => {
                self.state = SwipeState::Tracking {
                    id: ev.id,
                    start: ev.position,
                    last_position: ev.position,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: TouchPoint::ZERO,
                };
                (G::Possible, TouchResponse::CONSUMED)
            }
            (TouchPhase::Began, _) => (G::Possible, TouchResponse::IGNORED),

            (TouchPhase::Moved, SwipeState::Tracking {
                id,
                start,
                last_position,
                last_ts_ns,
                velocity,
            }) if id == ev.id => {
                let frame_dx = ev.position.x - last_position.x;
                let frame_dy = ev.position.y - last_position.y;
                let dt_sec = if ev.timestamp_ns > last_ts_ns {
                    ((ev.timestamp_ns - last_ts_ns) as f32) / 1_000_000_000.0
                } else {
                    1.0 / 60.0
                }
                .max(0.001);
                let a = SWIPE_VELOCITY_SMOOTHING;
                let new_velocity = TouchPoint::new(
                    velocity.x * (1.0 - a) + (frame_dx / dt_sec) * a,
                    velocity.y * (1.0 - a) + (frame_dy / dt_sec) * a,
                );
                self.state = SwipeState::Tracking {
                    id,
                    start,
                    last_position: ev.position,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: new_velocity,
                };
                (G::Possible, TouchResponse::CONSUMED)
            }
            (TouchPhase::Moved, _) => (G::Possible, TouchResponse::IGNORED),

            (TouchPhase::Ended, SwipeState::Tracking { id, start, velocity, .. }) if id == ev.id => {
                self.state = SwipeState::Idle;
                match self.classify(start, ev.position, velocity) {
                    Some(dir) if ctx.may_recognize => {
                        (self.on_swipe)(dir);
                        (G::Recognized, TouchResponse::CONSUMED)
                    }
                    _ => (G::Failed, TouchResponse::CONSUMED),
                }
            }
            (TouchPhase::Ended, _) => (G::Possible, TouchResponse::IGNORED),

            (TouchPhase::Cancelled, SwipeState::Tracking { id, .. }) if id == ev.id => {
                self.state = SwipeState::Idle;
                (G::Failed, TouchResponse::CONSUMED)
            }
            (TouchPhase::Cancelled, _) => (G::Possible, TouchResponse::IGNORED),
        };
        RecognizerUpdate::new(state, response)
    }
}

/// Build a single-finger swipe [`TouchHandler`] for a view's `on_touch`
/// slot. Wraps a [`Swipe`] recognizer, ungated.
pub fn swipe<F: Fn(SwipeDirection) + 'static>(
    config: SwipeRecognizer,
    on_swipe: F,
) -> TouchHandler {
    let rec = Rc::new(RefCell::new(Swipe::new(config, on_swipe)));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

// ---------------------------------------------------------------------------
// Rotate (two-finger continuous rotation)
// ---------------------------------------------------------------------------

/// Minimum absolute rotation (radians) of the two-finger line past the
/// two-finger-down angle before the rotate becomes active and fires
/// [`RotateEvent::Began`]. ~5°, enough to reject the incidental twist of a
/// two-finger pan or pinch.
pub const DEFAULT_ROTATE_SLOP_RAD: f32 = 0.087;

/// EMA mixing factor for angular-velocity smoothing — same constant the
/// pan / pinch recognizers use.
const ROTATE_VELOCITY_SMOOTHING: f32 = 0.6;

/// Configuration for [`rotate`] / [`Rotate`].
#[derive(Clone, Copy, Debug)]
pub struct RotateRecognizer {
    pub slop_rad: f32,
}

impl Default for RotateRecognizer {
    fn default() -> Self {
        Self {
            slop_rad: DEFAULT_ROTATE_SLOP_RAD,
        }
    }
}

impl RotateRecognizer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn slop_rad(mut self, v: f32) -> Self {
        self.slop_rad = v;
        self
    }
}

/// Lifecycle events fired by [`rotate`]. `rotation` is **cumulative
/// radians relative to the two-finger-down angle** (positive = clockwise
/// in screen coordinates, since y grows down), mirroring how [`PinchEvent`]
/// reports cumulative scale — handlers add it onto an angle snapshotted at
/// [`RotateEvent::Began`].
#[derive(Clone, Copy, Debug)]
pub enum RotateEvent {
    /// Two fingers down and the line between them has turned past slop.
    /// `focus` is the midpoint — the natural point to rotate about.
    Began { focus: TouchPoint },
    /// Rotation in progress. `rotation` is cumulative radians; `velocity`
    /// is radians/second, EMA-smoothed.
    Changed {
        focus: TouchPoint,
        rotation: f32,
        velocity: f32,
    },
    /// A finger lifted after the rotate was active. Final `velocity` lets
    /// handlers fling the rotation to a momentum settle.
    Ended { velocity: f32 },
    /// Interrupted by the platform. Handlers reset / animate back to rest.
    Cancelled,
}

#[derive(Clone, Copy)]
enum RotateState {
    Idle,
    One {
        a: PinchFinger,
    },
    /// Two fingers tracked. `rotation` accumulates the wrapped per-frame
    /// angle delta since the pair formed; `active` flips once
    /// `|rotation|` crosses slop.
    Two {
        a: PinchFinger,
        b: PinchFinger,
        last_angle: f32,
        rotation: f32,
        active: bool,
        last_ts_ns: u64,
        velocity: f32,
    },
}

/// Angle (radians) of the directed line a→b. y grows down, so a positive
/// increase is a clockwise turn on screen.
fn two_finger_angle(a: TouchPoint, b: TouchPoint) -> f32 {
    (b.y - a.y).atan2(b.x - a.x)
}

/// Wrap an angle delta into `[-π, π]` so a rotation that crosses the
/// ±π seam accumulates continuously instead of jumping a full turn.
fn wrap_angle(mut a: f32) -> f32 {
    use std::f32::consts::PI;
    while a > PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

/// Two-finger rotation recognizer ([`Recognizer`] impl). The angular peer
/// of [`Pinch`]: same two-finger lifecycle, but measures the turn of the
/// finger line rather than the change in their distance.
///
/// Like pinch, it lets a lone finger bubble (`IGNORED`), so it composes
/// with a single-finger [`Pan`] — and with [`Pinch`] itself — in a
/// [`GestureGroup`] via `allow_simultaneous` (the standard rotate+zoom+pan
/// map/photo manipulation set).
pub struct Rotate {
    config: RotateRecognizer,
    on_rotate: Box<dyn Fn(&RotateEvent)>,
    state: RotateState,
}

impl Rotate {
    pub fn new<F: Fn(&RotateEvent) + 'static>(config: RotateRecognizer, on_rotate: F) -> Self {
        Self {
            config,
            on_rotate: Box::new(on_rotate),
            state: RotateState::Idle,
        }
    }
}

impl Recognizer for Rotate {
    fn name(&self) -> &'static str {
        "rotate"
    }
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Continuous
    }
    fn state(&self) -> GestureState {
        match self.state {
            RotateState::Two { active: true, .. } => GestureState::Changed,
            _ => GestureState::Possible,
        }
    }
    fn reset(&mut self, cancelled: bool) {
        if cancelled && matches!(self.state, RotateState::Two { active: true, .. }) {
            (self.on_rotate)(&RotateEvent::Cancelled);
        }
        self.state = RotateState::Idle;
    }
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate {
        use GestureState as G;
        let config = self.config;
        let cur = self.state;
        let (state, response): (GestureState, TouchResponse) = match ev.phase {
            TouchPhase::Began => match cur {
                RotateState::Idle => {
                    self.state = RotateState::One {
                        a: PinchFinger {
                            id: ev.id,
                            pos: ev.position,
                        },
                    };
                    (G::Possible, TouchResponse::IGNORED)
                }
                RotateState::One { a } if a.id != ev.id => {
                    let b = PinchFinger {
                        id: ev.id,
                        pos: ev.position,
                    };
                    self.state = RotateState::Two {
                        a,
                        b,
                        last_angle: two_finger_angle(a.pos, b.pos),
                        rotation: 0.0,
                        active: false,
                        last_ts_ns: ev.timestamp_ns,
                        velocity: 0.0,
                    };
                    (G::Possible, TouchResponse::IGNORED)
                }
                _ => (self.state(), TouchResponse::IGNORED),
            },

            TouchPhase::Moved => match cur {
                RotateState::One { mut a } if a.id == ev.id => {
                    a.pos = ev.position;
                    self.state = RotateState::One { a };
                    (G::Possible, TouchResponse::IGNORED)
                }
                RotateState::Two {
                    mut a,
                    mut b,
                    last_angle,
                    rotation,
                    active,
                    last_ts_ns,
                    velocity,
                } => {
                    if a.id == ev.id {
                        a.pos = ev.position;
                    } else if b.id == ev.id {
                        b.pos = ev.position;
                    } else {
                        return RecognizerUpdate::new(self.state(), TouchResponse::IGNORED);
                    }
                    let cur_angle = two_finger_angle(a.pos, b.pos);
                    let delta = wrap_angle(cur_angle - last_angle);
                    let rotation = rotation + delta;
                    let focus = pinch_midpoint(a.pos, b.pos);
                    if !active {
                        if rotation.abs() > config.slop_rad && ctx.may_recognize {
                            self.state = RotateState::Two {
                                a,
                                b,
                                last_angle: cur_angle,
                                rotation,
                                active: true,
                                last_ts_ns: ev.timestamp_ns,
                                velocity: 0.0,
                            };
                            (self.on_rotate)(&RotateEvent::Began { focus });
                            (self.on_rotate)(&RotateEvent::Changed {
                                focus,
                                rotation,
                                velocity: 0.0,
                            });
                            (G::Began, TouchResponse::CLAIMED)
                        } else {
                            self.state = RotateState::Two {
                                a,
                                b,
                                last_angle: cur_angle,
                                rotation,
                                active: false,
                                last_ts_ns: ev.timestamp_ns,
                                velocity,
                            };
                            (G::Possible, TouchResponse::IGNORED)
                        }
                    } else {
                        let dt_sec = if ev.timestamp_ns > last_ts_ns {
                            ((ev.timestamp_ns - last_ts_ns) as f32) / 1_000_000_000.0
                        } else {
                            1.0 / 60.0
                        }
                        .max(0.001);
                        let raw_v = delta / dt_sec;
                        let mix = ROTATE_VELOCITY_SMOOTHING;
                        let new_velocity = velocity * (1.0 - mix) + raw_v * mix;
                        self.state = RotateState::Two {
                            a,
                            b,
                            last_angle: cur_angle,
                            rotation,
                            active: true,
                            last_ts_ns: ev.timestamp_ns,
                            velocity: new_velocity,
                        };
                        (self.on_rotate)(&RotateEvent::Changed {
                            focus,
                            rotation,
                            velocity: new_velocity,
                        });
                        (G::Changed, TouchResponse::CLAIMED)
                    }
                }
                _ => (self.state(), TouchResponse::IGNORED),
            },

            TouchPhase::Ended | TouchPhase::Cancelled => {
                let is_cancel = matches!(ev.phase, TouchPhase::Cancelled);
                match cur {
                    RotateState::One { a } if a.id == ev.id => {
                        self.state = RotateState::Idle;
                        (G::Failed, TouchResponse::IGNORED)
                    }
                    RotateState::Two { a, b, active, velocity, .. }
                        if a.id == ev.id || b.id == ev.id =>
                    {
                        let remaining = if a.id == ev.id { b } else { a };
                        if active {
                            if is_cancel {
                                (self.on_rotate)(&RotateEvent::Cancelled);
                            } else {
                                (self.on_rotate)(&RotateEvent::Ended { velocity });
                            }
                        }
                        if is_cancel {
                            self.state = RotateState::Idle;
                        } else {
                            self.state = RotateState::One { a: remaining };
                        }
                        match (active, is_cancel) {
                            (true, false) => (G::Recognized, TouchResponse::CONSUMED),
                            (true, true) => (G::Cancelled, TouchResponse::CONSUMED),
                            (false, _) => (G::Failed, TouchResponse::IGNORED),
                        }
                    }
                    _ => (self.state(), TouchResponse::IGNORED),
                }
            }
        };
        RecognizerUpdate::new(state, response)
    }
}

/// Build a two-finger rotate [`TouchHandler`] for a view's `on_touch`
/// slot. Wraps a [`Rotate`] recognizer, ungated. Like [`pinch`], a single
/// `on_touch` slot holds one handler — compose rotate with pinch/pan in a
/// `GestureGroup`.
pub fn rotate<F: Fn(&RotateEvent) + 'static>(
    config: RotateRecognizer,
    on_rotate: F,
) -> TouchHandler {
    let rec = Rc::new(RefCell::new(Rotate::new(config, on_rotate)));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        rec.borrow_mut().update(ev, &RecognizerCtx::UNGATED).response
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    // -----------------------------------------------------------------
    // Test scheduler — a deterministic, manually-advanced clock for
    // exercising any recognizer that schedules a future callback (the
    // long-press recognizer being the current one).
    //
    // Why custom: runtime-core's default no-scheduler behavior on
    // native is to invoke `after_ms` callbacks *synchronously at call
    // time*, which fires the long-press timer before the recognizer
    // has even finished setting up its `Tracking` state. We need to
    // hold the callback pending and release it on an explicit clock
    // advance.
    //
    // State lives in `thread_local!` so concurrent `cargo test`
    // threads don't trample each other; the `install_scheduler` call
    // is process-global (OnceLock) but the unit-struct `TestScheduler`
    // is trivially Send+Sync and reads its state per-thread. Each
    // test calls `reset_test_clock()` at the start to clear leftover
    // state in case cargo reuses a thread.
    // -----------------------------------------------------------------

    struct TestScheduler;

    thread_local! {
        static QUEUE: RefCell<Vec<TimerEntry>> = RefCell::new(Vec::new());
        static NOW_MS: Cell<u64> = const { Cell::new(0) };
        static NEXT_ID: Cell<u64> = const { Cell::new(0) };
        static CANCELLED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    }

    struct TimerEntry {
        id: u64,
        fire_at_ms: u64,
        cb: Box<dyn FnOnce()>,
    }

    struct TestHandle {
        id: u64,
    }

    impl ScheduleHandle for TestHandle {
        fn cancel(&mut self) {
            CANCELLED.with(|c| {
                c.borrow_mut().insert(self.id);
            });
        }
    }

    impl Scheduler for TestScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            // Microtasks run synchronously in tests — there's no JS
            // event loop to defer them onto.
            f();
        }
        fn after_animation_frame(
            &self,
            _f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            // Unused by these tests.
            Box::new(TestHandle { id: u64::MAX })
        }
        fn after_ms(
            &self,
            delay_ms: i32,
            f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            let id = NEXT_ID.with(|n| {
                let v = n.get();
                n.set(v + 1);
                v
            });
            let fire_at = NOW_MS.with(|n| n.get()) + delay_ms.max(0) as u64;
            QUEUE.with(|q| {
                q.borrow_mut().push(TimerEntry {
                    id,
                    fire_at_ms: fire_at,
                    cb: f,
                });
            });
            Box::new(TestHandle { id })
        }
        fn raf_loop(
            &self,
            _f: Box<dyn FnMut() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            // Unused by these tests.
            Box::new(TestHandle { id: u64::MAX })
        }
    }

    /// Drain every timer whose `fire_at_ms <= clock + ms`, advance the
    /// clock to that value, and invoke each timer's callback in fire
    /// order (cancelled timers are dropped without firing). Safe to
    /// call multiple times in one test.
    fn advance_ms(ms: u64) {
        NOW_MS.with(|n| n.set(n.get() + ms));
        let now = NOW_MS.with(|n| n.get());
        let ready: Vec<TimerEntry> = QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            // Partition with retain-equivalent so callbacks can
            // schedule more timers from inside without nested borrow.
            let mut ready = Vec::new();
            let mut keep = Vec::new();
            for entry in q.drain(..) {
                if entry.fire_at_ms <= now {
                    ready.push(entry);
                } else {
                    keep.push(entry);
                }
            }
            *q = keep;
            ready
        });
        let cancelled_snapshot: HashSet<u64> = CANCELLED.with(|c| c.borrow().clone());
        for entry in ready {
            if !cancelled_snapshot.contains(&entry.id) {
                (entry.cb)();
            }
        }
    }

    /// Reset thread-local timer state at the start of each test.
    /// Cargo can reuse threads between tests in the same binary;
    /// without this, a previous test's residual timers could fire
    /// inside the next test's advance.
    fn reset_test_clock() {
        QUEUE.with(|q| q.borrow_mut().clear());
        NOW_MS.with(|n| n.set(0));
        NEXT_ID.with(|n| n.set(0));
        CANCELLED.with(|c| c.borrow_mut().clear());
    }

    fn install_test_scheduler_once() {
        // OnceLock: first install wins; subsequent calls are silently
        // ignored. Every test calls this — only the first does any
        // work.
        install_scheduler(Box::new(TestScheduler));
    }

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

    #[test]
    fn tap_fires_on_clean_press_release() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(TapRecognizer::new(), move || fires.set(fires.get() + 1))
        };
        let r1 = h(&ev(TouchPhase::Began, 1, 10.0, 10.0, 0));
        assert!(r1.consumed);
        let r2 = h(&ev(TouchPhase::Ended, 1, 11.0, 11.0, 50_000_000));
        assert!(r2.consumed);
        assert_eq!(fires.get(), 1);
    }

    #[test]
    fn tap_rejects_on_slop_exceeded() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(TapRecognizer::new(), move || fires.set(fires.get() + 1))
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // 50px move — far past default 10px slop.
        h(&ev(TouchPhase::Moved, 1, 50.0, 0.0, 10_000_000));
        h(&ev(TouchPhase::Ended, 1, 50.0, 0.0, 20_000_000));
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn tap_rejects_on_timeout_exceeded() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(
                TapRecognizer::new().max_duration_ms(100),
                move || fires.set(fires.get() + 1),
            )
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // 200ms hold — past the 100ms max.
        h(&ev(TouchPhase::Ended, 1, 1.0, 1.0, 200_000_000));
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn tap_does_not_fire_on_cancel() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(TapRecognizer::new(), move || fires.set(fires.get() + 1))
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Cancelled, 1, 0.0, 0.0, 50_000_000));
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn tap_recovers_after_one_interaction() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(TapRecognizer::new(), move || fires.set(fires.get() + 1))
        };
        // First tap.
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Ended, 1, 0.0, 0.0, 30_000_000));
        // Second tap with a different id (mouse re-press uses same
        // id; touch fingers get fresh ids — both must work).
        h(&ev(TouchPhase::Began, 2, 0.0, 0.0, 100_000_000));
        h(&ev(TouchPhase::Ended, 2, 0.0, 0.0, 130_000_000));
        assert_eq!(fires.get(), 2);
    }

    #[test]
    fn tap_ignores_extra_concurrent_finger() {
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            tap(TapRecognizer::new(), move || fires.set(fires.get() + 1))
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // Second finger lands — must not consume; the original
        // touch is still alive.
        let r2 = h(&ev(TouchPhase::Began, 2, 5.0, 5.0, 5_000_000));
        assert!(!r2.consumed, "second finger should bubble, not consume");
        // First finger releases cleanly.
        h(&ev(TouchPhase::Ended, 1, 0.0, 0.0, 30_000_000));
        assert_eq!(fires.get(), 1);
    }

    // -----------------------------------------------------------------
    // Long-press tests
    // -----------------------------------------------------------------

    #[test]
    fn long_press_fires_after_threshold() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 10.0, 10.0, 0));
        // Just before the default 500 ms threshold — must not fire.
        advance_ms(499);
        assert_eq!(fires.get(), 0, "fired too early");
        // Tick past the threshold — must fire exactly once.
        advance_ms(2);
        assert_eq!(fires.get(), 1, "did not fire after threshold");
    }

    #[test]
    fn long_press_cancels_on_early_release() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // Release before threshold.
        h(&ev(TouchPhase::Ended, 1, 0.0, 0.0, 100_000_000));
        // Advance well past the original threshold — must not fire,
        // the cancel-on-release path should have dropped the timer.
        advance_ms(2_000);
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn long_press_rejects_on_slop_exceeded() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // Move past default 10 px slop while the timer is still
        // armed — recognizer should cancel the timer and reject.
        h(&ev(TouchPhase::Moved, 1, 50.0, 0.0, 100_000_000));
        advance_ms(2_000);
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn long_press_does_not_fire_on_cancel() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Cancelled, 1, 0.0, 0.0, 100_000_000));
        advance_ms(2_000);
        assert_eq!(fires.get(), 0);
    }

    #[test]
    fn long_press_ignores_extra_concurrent_finger() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // Second finger lands mid-press; the recognizer should
        // ignore it (return unconsumed) without disturbing the
        // pending timer.
        let r2 = h(&ev(TouchPhase::Began, 2, 5.0, 5.0, 100_000_000));
        assert!(!r2.consumed);
        // Original timer still fires when its threshold is crossed.
        advance_ms(500);
        assert_eq!(fires.get(), 1);
    }

    #[test]
    fn long_press_custom_threshold_and_slop() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let cfg = LongPressRecognizer::new().threshold_ms(200).slop_px(30.0);
        let h = {
            let fires = fires.clone();
            long_press(cfg, move || fires.set(fires.get() + 1))
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        // 25 px move — past tap's 10 px default but inside the
        // overridden 30 px slop. Timer must survive.
        h(&ev(TouchPhase::Moved, 1, 25.0, 0.0, 50_000_000));
        advance_ms(199);
        assert_eq!(fires.get(), 0);
        advance_ms(2);
        assert_eq!(fires.get(), 1);
    }

    #[test]
    fn long_press_recovers_after_one_interaction() {
        install_test_scheduler_once();
        reset_test_clock();
        let fires = Rc::new(Cell::new(0u32));
        let h = {
            let fires = fires.clone();
            long_press(LongPressRecognizer::new(), move || {
                fires.set(fires.get() + 1)
            })
        };
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        advance_ms(501);
        h(&ev(TouchPhase::Ended, 1, 0.0, 0.0, 600_000_000));
        // Second press, fresh touch id.
        h(&ev(TouchPhase::Began, 2, 0.0, 0.0, 700_000_000));
        advance_ms(501);
        assert_eq!(fires.get(), 2);
    }

    // -----------------------------------------------------------------
    // Pinch tests
    // -----------------------------------------------------------------

    fn pinch_collect() -> (TouchHandler, Rc<RefCell<Vec<PinchEvent>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let h = pinch(PinchRecognizer::new(), move |e| sink.borrow_mut().push(*e));
        (h, log)
    }

    #[test]
    fn pinch_activates_and_reports_cumulative_scale() {
        let (h, log) = pinch_collect();
        // Two fingers land 100 px apart on the x axis.
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 100.0, 0.0, 0));
        // Spread to 200 px apart → scale 2.0, focus at midpoint (100, 0).
        h(&ev(TouchPhase::Moved, 2, 200.0, 0.0, 16_000_000));
        let events = log.borrow();
        assert!(
            matches!(events.first(), Some(PinchEvent::Began { .. })),
            "first event is Began"
        );
        match events.last() {
            Some(PinchEvent::Changed { scale, focus, .. }) => {
                assert!((*scale - 2.0).abs() < 1e-3, "scale should be 2.0, got {scale}");
                assert!((focus.x - 100.0).abs() < 1e-3 && focus.y.abs() < 1e-3);
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn pinch_does_not_activate_below_slop() {
        let (h, log) = pinch_collect();
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 100.0, 0.0, 0));
        // Move only 3 px — under the 6 px default slop.
        h(&ev(TouchPhase::Moved, 2, 103.0, 0.0, 16_000_000));
        assert!(log.borrow().is_empty(), "no pinch below slop");
    }

    #[test]
    fn pinch_single_finger_emits_nothing_and_does_not_consume() {
        let (h, log) = pinch_collect();
        // A lone finger must bubble so a tap/pan on the same chain still
        // sees it — pinch only owns the touch once two fingers are active.
        let r = h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        assert!(!r.consumed, "pinch must not consume a single finger");
        h(&ev(TouchPhase::Moved, 1, 80.0, 0.0, 16_000_000));
        h(&ev(TouchPhase::Ended, 1, 80.0, 0.0, 32_000_000));
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn pinch_ends_and_claims_once_active() {
        let (h, log) = pinch_collect();
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 100.0, 0.0, 0));
        let r = h(&ev(TouchPhase::Moved, 2, 200.0, 0.0, 16_000_000));
        assert!(r.claim, "active pinch claims to preempt parent scroll");
        h(&ev(TouchPhase::Ended, 2, 200.0, 0.0, 32_000_000));
        assert!(matches!(log.borrow().last(), Some(PinchEvent::Ended { .. })));
    }

    #[test]
    fn pinch_scale_relative_to_start_not_absolute() {
        // Fingers start 50 px apart, spread to 150 → scale 3.0, regardless
        // of absolute screen position.
        let (h, log) = pinch_collect();
        h(&ev(TouchPhase::Began, 1, 300.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 350.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 2, 450.0, 0.0, 16_000_000));
        let events = log.borrow();
        match events.last() {
            Some(PinchEvent::Changed { scale, .. }) => {
                assert!((*scale - 3.0).abs() < 1e-3, "got {scale}")
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Swipe
    // -----------------------------------------------------------------

    fn swipe_collect(cfg: SwipeRecognizer) -> (TouchHandler, Rc<RefCell<Vec<SwipeDirection>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let h = swipe(cfg, move |d| sink.borrow_mut().push(d));
        (h, log)
    }

    /// Drive a fast horizontal flick: four 40 px/16 ms frames → ~2300 px/s.
    fn fast_flick_right(h: &TouchHandler) {
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 1, 40.0, 0.0, 16_000_000));
        h(&ev(TouchPhase::Moved, 1, 80.0, 0.0, 32_000_000));
        h(&ev(TouchPhase::Moved, 1, 120.0, 0.0, 48_000_000));
        h(&ev(TouchPhase::Ended, 1, 120.0, 0.0, 48_000_000));
    }

    #[test]
    fn swipe_recognizes_fast_horizontal_flick() {
        let (h, log) = swipe_collect(SwipeRecognizer::new());
        fast_flick_right(&h);
        assert_eq!(log.borrow().as_slice(), &[SwipeDirection::Right]);
    }

    #[test]
    fn swipe_recognizes_vertical_direction() {
        let (h, log) = swipe_collect(SwipeRecognizer::new());
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 1, 0.0, -40.0, 16_000_000));
        h(&ev(TouchPhase::Moved, 1, 0.0, -80.0, 32_000_000));
        h(&ev(TouchPhase::Moved, 1, 0.0, -120.0, 48_000_000));
        h(&ev(TouchPhase::Ended, 1, 0.0, -120.0, 48_000_000));
        assert_eq!(log.borrow().as_slice(), &[SwipeDirection::Up]);
    }

    #[test]
    fn swipe_fails_slow_drag() {
        // 40 px over 500 ms → 80 px/s, well under the velocity threshold.
        let (h, log) = swipe_collect(SwipeRecognizer::new());
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 1, 40.0, 0.0, 500_000_000));
        h(&ev(TouchPhase::Ended, 1, 40.0, 0.0, 500_000_000));
        assert!(log.borrow().is_empty(), "slow drag is not a swipe");
    }

    #[test]
    fn swipe_direction_filter_rejects_disallowed_axis() {
        // Vertical-only recognizer sees a horizontal flick → no fire.
        let (h, log) = swipe_collect(SwipeRecognizer::new().directions(SwipeDirs::VERTICAL));
        fast_flick_right(&h);
        assert!(log.borrow().is_empty(), "horizontal flick rejected by VERTICAL filter");
    }

    #[test]
    fn swipe_gate_suppresses_recognition() {
        // A gated Ended must not fire, mirroring how the arbiter holds a
        // dependent until its prerequisite fails.
        let fired = Rc::new(Cell::new(0u32));
        let mut rec = {
            let fired = fired.clone();
            Swipe::new(SwipeRecognizer::new(), move |_d| fired.set(fired.get() + 1))
        };
        rec.update(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0), &RecognizerCtx::UNGATED);
        rec.update(&ev(TouchPhase::Moved, 1, 60.0, 0.0, 16_000_000), &RecognizerCtx::UNGATED);
        let gated = RecognizerCtx { may_recognize: false };
        let upd = rec.update(&ev(TouchPhase::Ended, 1, 120.0, 0.0, 32_000_000), &gated);
        assert_eq!(fired.get(), 0, "gated swipe does not fire");
        assert_eq!(upd.state, GestureState::Failed);
    }

    // -----------------------------------------------------------------
    // Rotate
    // -----------------------------------------------------------------

    fn rotate_collect() -> (TouchHandler, Rc<RefCell<Vec<RotateEvent>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let h = rotate(RotateRecognizer::new(), move |e| sink.borrow_mut().push(*e));
        (h, log)
    }

    #[test]
    fn rotate_recognizes_past_slop_with_cumulative_radians() {
        use std::f32::consts::FRAC_PI_4;
        let (h, log) = rotate_collect();
        // Line a→b starts horizontal (angle 0); rotate b to +45°.
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 100.0, 0.0, 0));
        h(&ev(TouchPhase::Moved, 2, 70.0, 70.0, 16_000_000));
        let events = log.borrow();
        assert!(
            matches!(events.first(), Some(RotateEvent::Began { .. })),
            "first event is Began: {events:?}"
        );
        match events.last() {
            Some(RotateEvent::Changed { rotation, .. }) => {
                assert!((*rotation - FRAC_PI_4).abs() < 1e-2, "got {rotation}")
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn rotate_ignores_below_slop() {
        let (h, log) = rotate_collect();
        h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        h(&ev(TouchPhase::Began, 2, 100.0, 0.0, 0));
        // atan2(2, 100) ≈ 0.02 rad, under the ~0.087 rad slop.
        h(&ev(TouchPhase::Moved, 2, 100.0, 2.0, 16_000_000));
        assert!(log.borrow().is_empty(), "tiny twist below slop fires nothing");
    }

    #[test]
    fn rotate_lone_finger_bubbles() {
        // One finger must IGNORE so a single-finger pan can coexist.
        let (h, _log) = rotate_collect();
        let r = h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
        assert!(!r.consumed, "lone finger bubbles for a sibling pan");
    }
}
