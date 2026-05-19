//! Stock gesture recognizers built on the raw [`TouchEvent`] stream.
//!
//! Each recognizer is a pure function from `(config, callback) →
//! TouchHandler` — the returned handler is installed on a primitive
//! via `Bound::<ViewHandle>::on_touch(...)` (or any future primitive
//! with a touch slot). State lives inside the closure via
//! `Rc<RefCell<…>>`, so multiple primitives can share the same
//! recognizer factory without entangling state.
//!
//! Recognizers consume the [`TouchPhase::Began`] event optimistically:
//! once a finger lands inside a subscribed view, the touch is owned by
//! the recognizer through `Ended` / `Cancelled`. This is the simplest
//! model that works for v1; future iterations will introduce a
//! requireToFail-style coordination layer so a tap recognizer can
//! release the touch to a parent scroll container when the user pans
//! instead of taps. See `docs/native-touch-plan.md`.
//!
//! Each recognizer matches a single concurrent finger. Multi-finger
//! recognizers (pinch, rotate, two-finger pan) will live alongside
//! these as their own factories.

use std::cell::RefCell;
use std::rc::Rc;

use crate::scheduling::{after_ms, ScheduledTask};
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

/// Build a single-finger tap [`TouchHandler`].
///
/// Fires `on_tap` once per qualifying touch: `Began` → `Ended` with
/// total movement ≤ `slop_px` and duration ≤ `max_duration_ms`.
/// Movement past slop or duration past timeout marks the interaction
/// rejected; subsequent events still consume (the recognizer keeps
/// ownership of the touch) but no callback fires.
///
/// ```ignore
/// view(children!(...))
///     .on_touch(tap(TapRecognizer::new(), || println!("tapped")))
/// ```
pub fn tap<F: Fn() + 'static>(config: TapRecognizer, on_tap: F) -> TouchHandler {
    let state = Rc::new(RefCell::new(TapState::Idle));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        let mut s = state.borrow_mut();
        match (ev.phase, *s) {
            // New touch begins while idle: start tracking.
            (TouchPhase::Began, TapState::Idle) => {
                *s = TapState::Tracking {
                    id: ev.id,
                    start: ev.position,
                    start_ts_ns: ev.timestamp_ns,
                };
                TouchResponse::CONSUMED
            }
            // A second finger lands while we're tracking the first.
            // Single-finger recognizer ignores extras — return
            // unconsumed so an outer handler (or another recognizer
            // sibling) can pick them up.
            (TouchPhase::Began, _) => TouchResponse::IGNORED,

            // Movement on the tracked finger: re-check slop / timeout.
            (TouchPhase::Moved, TapState::Tracking { id, start, start_ts_ns }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let exceeded_slop = (dx * dx + dy * dy) > config.slop_px * config.slop_px;
                let exceeded_time = ev.timestamp_ns.saturating_sub(start_ts_ns)
                    > config.max_duration_ms * 1_000_000;
                if exceeded_slop || exceeded_time {
                    *s = TapState::Rejected { id };
                }
                TouchResponse::CONSUMED
            }
            // Movement on the tracked finger after rejection: keep
            // ownership but no further state change.
            (TouchPhase::Moved, TapState::Rejected { id }) if id == ev.id => {
                TouchResponse::CONSUMED
            }
            // Movement we don't care about (foreign id while tracking,
            // or events with no active state). Don't consume.
            (TouchPhase::Moved, _) => TouchResponse::IGNORED,

            // Release on the tracked finger: fire if still tracking
            // AND the final event also passes slop / timeout.
            (TouchPhase::Ended, TapState::Tracking { id, start, start_ts_ns }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let within_slop = (dx * dx + dy * dy) <= config.slop_px * config.slop_px;
                let within_time = ev.timestamp_ns.saturating_sub(start_ts_ns)
                    <= config.max_duration_ms * 1_000_000;
                *s = TapState::Idle;
                if within_slop && within_time {
                    on_tap();
                }
                TouchResponse::CONSUMED
            }
            (TouchPhase::Ended, TapState::Rejected { id }) if id == ev.id => {
                *s = TapState::Idle;
                TouchResponse::CONSUMED
            }
            (TouchPhase::Ended, _) => TouchResponse::IGNORED,

            // Cancellation: reset, never fire.
            (TouchPhase::Cancelled, TapState::Tracking { id, .. })
            | (TouchPhase::Cancelled, TapState::Rejected { id })
                if id == ev.id =>
            {
                *s = TapState::Idle;
                TouchResponse::CONSUMED
            }
            (TouchPhase::Cancelled, _) => TouchResponse::IGNORED,
        }
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

/// Build a single-finger pan [`TouchHandler`].
///
/// State machine:
/// - `TouchPhase::Began` → start tracking (no callback yet).
/// - First `Moved` past `slop_px` → fire `PanEvent::Began` with
///   the touch's current view-local position, then immediately
///   fire a `Moved` with `delta = (0, 0)`.
/// - Subsequent `Moved` → fire `PanEvent::Moved` with cumulative
///   delta from `Began` + smoothed velocity.
/// - `Ended` while active → fire `PanEvent::Ended { velocity }`.
/// - `Cancelled` while active → fire `PanEvent::Cancelled`.
///
/// The handler returns:
/// - `CONSUMED` while tracking (slop not exceeded) — owns the
///   touch but hasn't preempted anything yet.
/// - `CONSUMED | CLAIM` once active — tells the backend to
///   suppress parent scroll containers via the claim protocol.
///
/// Single-finger only; secondary touches return `IGNORED` so an
/// outer pinch recognizer (future work) can pick them up.
pub fn pan<F: Fn(&PanEvent) + 'static>(config: PanRecognizer, on_pan: F) -> TouchHandler {
    use std::cell::Cell;
    let on_pan = Rc::new(on_pan);
    let state: Rc<Cell<PanState>> = Rc::new(Cell::new(PanState::Idle));
    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        let current = state.get();
        match (ev.phase, current) {
            (TouchPhase::Began, PanState::Idle) => {
                state.set(PanState::Tracking {
                    id: ev.id,
                    start: ev.position,
                });
                TouchResponse::CONSUMED
            }
            (TouchPhase::Began, _) => TouchResponse::IGNORED,

            (TouchPhase::Moved, PanState::Tracking { id, start }) if id == ev.id => {
                let dx = ev.position.x - start.x;
                let dy = ev.position.y - start.y;
                let dist2 = dx * dx + dy * dy;
                if dist2 > config.slop_px * config.slop_px {
                    state.set(PanState::Active {
                        id: ev.id,
                        start,
                        last_position: ev.position,
                        last_ts_ns: ev.timestamp_ns,
                        velocity: TouchPoint::ZERO,
                    });
                    on_pan(&PanEvent::Began { position: ev.position });
                    on_pan(&PanEvent::Moved {
                        position: ev.position,
                        delta: TouchPoint::new(dx, dy),
                        velocity: TouchPoint::ZERO,
                    });
                    TouchResponse::CLAIMED
                } else {
                    TouchResponse::CONSUMED
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
                state.set(PanState::Active {
                    id: ev.id,
                    start,
                    last_position: ev.position,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: new_velocity,
                });
                on_pan(&PanEvent::Moved {
                    position: ev.position,
                    delta: TouchPoint::new(dx, dy),
                    velocity: new_velocity,
                });
                TouchResponse::CLAIMED
            }
            (TouchPhase::Moved, _) => TouchResponse::IGNORED,

            (TouchPhase::Ended, PanState::Tracking { id, .. }) if id == ev.id => {
                // Never crossed slop — no Began fired, no Ended.
                state.set(PanState::Idle);
                TouchResponse::CONSUMED
            }
            (TouchPhase::Ended, PanState::Active { id, velocity, .. }) if id == ev.id => {
                state.set(PanState::Idle);
                on_pan(&PanEvent::Ended { velocity });
                TouchResponse::CONSUMED
            }
            (TouchPhase::Ended, _) => TouchResponse::IGNORED,

            (TouchPhase::Cancelled, PanState::Tracking { id, .. }) if id == ev.id => {
                state.set(PanState::Idle);
                TouchResponse::CONSUMED
            }
            (TouchPhase::Cancelled, PanState::Active { id, .. }) if id == ev.id => {
                state.set(PanState::Idle);
                on_pan(&PanEvent::Cancelled);
                TouchResponse::CONSUMED
            }
            (TouchPhase::Cancelled, _) => TouchResponse::IGNORED,
        }
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
    /// fires (which transitions us to `Fired`) or we cancel it on
    /// release / slop-exceed / cancellation.
    Tracking {
        id: TouchId,
        start: TouchPoint,
        timer: ScheduledTask,
    },
    /// Timer already fired; we've delivered `on_long_press`. Keep
    /// ownership through `Ended` so the touch doesn't reflow into
    /// the bubble dispatcher.
    Fired { id: TouchId },
    /// Slop exceeded before timer fired — recognizer dropped the
    /// gesture but keeps the finger to stay coherent.
    Rejected { id: TouchId },
}

/// Build a single-finger long-press [`TouchHandler`].
///
/// Fires `on_long_press` once per qualifying touch: the finger stays
/// within `slop_px` of its `Began` position for at least
/// `threshold_ms`. Movement past slop before the timer fires marks
/// the gesture rejected; release before the timer fires cancels
/// silently.
///
/// The fire happens on a scheduler tick (via
/// [`crate::scheduling::after_ms`]) on the same thread as the
/// framework — handlers can mutate signals safely.
pub fn long_press<F: Fn() + 'static>(
    config: LongPressRecognizer,
    on_long_press: F,
) -> TouchHandler {
    let on_long_press = Rc::new(on_long_press);
    let state: Rc<RefCell<LongPressState>> = Rc::new(RefCell::new(LongPressState::Idle));

    Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        let phase = ev.phase;
        let ev_id = ev.id;

        match phase {
            TouchPhase::Began => {
                // Single-finger: ignore extras while we're armed.
                if !matches!(*state.borrow(), LongPressState::Idle) {
                    return TouchResponse::IGNORED;
                }
                let timer = {
                    let cb = on_long_press.clone();
                    let state = state.clone();
                    let id = ev_id;
                    after_ms(config.threshold_ms as i32, move || {
                        // If we're still tracking the same id, fire.
                        let mut s = state.borrow_mut();
                        if let LongPressState::Tracking { id: cur, .. } = *s {
                            if cur == id {
                                *s = LongPressState::Fired { id };
                                drop(s);
                                cb();
                            }
                        }
                    })
                };
                *state.borrow_mut() = LongPressState::Tracking {
                    id: ev_id,
                    start: ev.position,
                    timer,
                };
                TouchResponse::CONSUMED
            }
            TouchPhase::Moved => {
                let mut s = state.borrow_mut();
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
                    LongPressState::Fired { id } | LongPressState::Rejected { id } if *id == ev_id => {
                        TouchResponse::CONSUMED
                    }
                    _ => TouchResponse::IGNORED,
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let mut s = state.borrow_mut();
                match &mut *s {
                    LongPressState::Tracking { id, timer, .. } if *id == ev_id => {
                        timer.cancel();
                        *s = LongPressState::Idle;
                        TouchResponse::CONSUMED
                    }
                    LongPressState::Fired { id } | LongPressState::Rejected { id } if *id == ev_id => {
                        *s = LongPressState::Idle;
                        TouchResponse::CONSUMED
                    }
                    _ => TouchResponse::IGNORED,
                }
            }
        }
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
    // Why custom: framework-core's default no-scheduler behavior on
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
}
