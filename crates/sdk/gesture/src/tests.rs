//! Arbitration matrix for [`GestureGroup`]: priority, require-to-fail,
//! simultaneous recognition, claim aggregation, single-recognizer parity,
//! and the long-press off-stream (timer) path through the arbiter.

use super::*;
use runtime_core::{
    scheduling::{install_scheduler, ScheduleHandle, Scheduler},
    LongPress, LongPressRecognizer, Pan, PanEvent, PanRecognizer, Pinch, PinchEvent,
    PinchRecognizer, Tap, TapRecognizer, TouchId, TouchPoint,
};
use std::cell::{Cell, RefCell};

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

/// A `Pan` that appends `tag:phase` to a shared log on each event.
fn recording_pan(log: Rc<RefCell<Vec<String>>>, tag: &'static str) -> Pan {
    Pan::new(PanRecognizer::new(), move |e: &PanEvent| {
        let p = match e {
            PanEvent::Began { .. } => "began",
            PanEvent::Moved { .. } => "moved",
            PanEvent::Ended { .. } => "ended",
            PanEvent::Cancelled => "cancelled",
        };
        log.borrow_mut().push(format!("{tag}:{p}"));
    })
}

fn recording_pinch(log: Rc<RefCell<Vec<String>>>, tag: &'static str) -> Pinch {
    Pinch::new(PinchRecognizer::new(), move |e: &PinchEvent| {
        let p = match e {
            PinchEvent::Began { .. } => "began",
            PinchEvent::Changed { .. } => "changed",
            PinchEvent::Ended { .. } => "ended",
            PinchEvent::Cancelled => "cancelled",
        };
        log.borrow_mut().push(format!("{tag}:{p}"));
    })
}

// ---------------------------------------------------------------------------
// Single-recognizer parity
// ---------------------------------------------------------------------------

#[test]
fn single_tap_group_matches_bare_factory() {
    let fires = Rc::new(Cell::new(0u32));
    let h = {
        let fires = fires.clone();
        let mut g = GestureGroup::new();
        g.add(Tap::new(TapRecognizer::new(), move || {
            fires.set(fires.get() + 1)
        }));
        g.handler()
    };
    let r1 = h(&ev(TouchPhase::Began, 1, 10.0, 10.0, 0));
    assert!(r1.consumed, "tap owns the touch on Began");
    let r2 = h(&ev(TouchPhase::Ended, 1, 11.0, 11.0, 50_000_000));
    assert!(r2.consumed);
    assert_eq!(fires.get(), 1, "a one-recognizer group taps like the factory");

    // And it recovers for the next interaction (group reset on last lift).
    h(&ev(TouchPhase::Began, 2, 10.0, 10.0, 0));
    h(&ev(TouchPhase::Ended, 2, 10.0, 10.0, 10_000_000));
    assert_eq!(fires.get(), 2);
}

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[test]
fn priority_higher_added_recognizer_wins_simultaneous_begin() {
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let h = {
        let mut g = GestureGroup::new();
        g.add(recording_pan(log.clone(), "a")); // higher priority
        g.add(recording_pan(log.clone(), "b"));
        g.handler()
    };
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // One move past slop: both pans would begin this event; A wins.
    h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    // A subsequent move only A should still be receiving.
    h(&ev(TouchPhase::Moved, 1, 40.0, 0.0, 32_000_000));

    let l = log.borrow();
    assert!(l.contains(&"a:began".to_string()), "winner A began: {l:?}");
    // B began on the same event then was cancelled by A's exclusivity.
    assert!(l.contains(&"b:began".to_string()), "B began before losing: {l:?}");
    assert!(l.contains(&"b:cancelled".to_string()), "loser B cancelled: {l:?}");
    // Only A keeps streaming; B's last event is its cancellation.
    assert!(
        l.iter().filter(|s| s.as_str() == "a:moved").count() >= 2,
        "winner A keeps receiving moves: {l:?}"
    );
    let last_b = l.iter().rev().find(|s| s.starts_with("b:")).unwrap();
    assert_eq!(last_b, "b:cancelled", "loser B receives nothing after cancel: {l:?}");
}

// ---------------------------------------------------------------------------
// require-to-fail
// ---------------------------------------------------------------------------

#[test]
fn require_to_fail_tap_fires_after_pan_fails() {
    let taps = Rc::new(Cell::new(0u32));
    let panlog = Rc::new(RefCell::new(Vec::<String>::new()));
    let h = {
        let taps = taps.clone();
        let mut g = GestureGroup::new();
        let tap = g.add(Tap::new(TapRecognizer::new(), move || taps.set(taps.get() + 1)));
        let pan = g.add(recording_pan(panlog.clone(), "pan"));
        g.require_to_fail(tap, pan);
        g.handler()
    };
    // Press and release without moving: pan fails on Ended (never crossed
    // slop), which unblocks tap *within the same event* (dependency order).
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    h(&ev(TouchPhase::Ended, 1, 1.0, 1.0, 40_000_000));
    assert_eq!(taps.get(), 1, "tap recognizes once pan fails");
    assert!(panlog.borrow().is_empty(), "pan never began: {:?}", panlog.borrow());
}

#[test]
fn require_to_fail_pan_wins_and_cancels_tap_on_drag() {
    let taps = Rc::new(Cell::new(0u32));
    let panlog = Rc::new(RefCell::new(Vec::<String>::new()));
    let h = {
        let taps = taps.clone();
        let mut g = GestureGroup::new();
        let tap = g.add(Tap::new(TapRecognizer::new(), move || taps.set(taps.get() + 1)));
        let pan = g.add(recording_pan(panlog.clone(), "pan"));
        g.require_to_fail(tap, pan);
        g.handler()
    };
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // Move 9px: past pan slop (8) but within tap slop (10), so tap doesn't
    // self-reject — the arbiter must cancel it because pan begins.
    h(&ev(TouchPhase::Moved, 1, 9.0, 0.0, 16_000_000));
    h(&ev(TouchPhase::Ended, 1, 9.0, 0.0, 40_000_000));
    assert_eq!(taps.get(), 0, "tap is cancelled by the winning pan");
    let l = panlog.borrow();
    assert!(l.contains(&"pan:began".to_string()), "pan began: {l:?}");
    assert!(l.contains(&"pan:ended".to_string()), "pan ended cleanly: {l:?}");
}

// ---------------------------------------------------------------------------
// simultaneous
// ---------------------------------------------------------------------------

#[test]
fn simultaneous_pan_and_pinch_both_recognize() {
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let h = {
        let mut g = GestureGroup::new();
        let pan = g.add(recording_pan(log.clone(), "pan"));
        let pinch = g.add(recording_pinch(log.clone(), "pinch"));
        g.allow_simultaneous(pan, pinch);
        g.handler()
    };
    // Finger 1 down, drag it → pan begins (pinch still tracking one finger).
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    let r = h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    assert!(r.claim, "active pan claims the touch");
    assert!(log.borrow().contains(&"pan:began".to_string()));

    // Second finger down, spread the pair → pinch begins; pan must survive.
    h(&ev(TouchPhase::Began, 2, 30.0, 40.0, 32_000_000));
    h(&ev(TouchPhase::Moved, 2, 30.0, 120.0, 48_000_000));
    let l = log.borrow();
    assert!(l.contains(&"pinch:began".to_string()), "pinch began: {l:?}");
    assert!(
        !l.contains(&"pan:cancelled".to_string()),
        "simultaneous pan not cancelled by pinch: {l:?}"
    );
}

// ---------------------------------------------------------------------------
// claim aggregation
// ---------------------------------------------------------------------------

#[test]
fn claim_is_aggregated_when_any_recognizer_is_active() {
    let h = {
        let mut g = GestureGroup::new();
        g.add(recording_pan(Rc::new(RefCell::new(Vec::new())), "pan"));
        g.handler()
    };
    let r0 = h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    assert!(r0.consumed && !r0.claim, "tracking consumes but doesn't claim");
    let r1 = h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    assert!(r1.claim, "active pan claims");
}

// ---------------------------------------------------------------------------
// long-press through the arbiter (off-stream timer)
// ---------------------------------------------------------------------------

#[test]
fn long_press_fires_through_arbiter_timer() {
    install_test_scheduler_once();
    reset_test_clock();
    let fires = Rc::new(Cell::new(0u32));
    let h = {
        let fires = fires.clone();
        let mut g = GestureGroup::new();
        g.add(LongPress::new(LongPressRecognizer::new(), move || {
            fires.set(fires.get() + 1)
        }));
        g.handler()
    };
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    assert_eq!(fires.get(), 0, "not yet — timer pending");
    advance_ms(500);
    assert_eq!(fires.get(), 1, "long-press recognized via the async re-arbitration path");
}

#[test]
fn long_press_cancelled_by_competing_pan_before_timer() {
    install_test_scheduler_once();
    reset_test_clock();
    let fires = Rc::new(Cell::new(0u32));
    let panlog = Rc::new(RefCell::new(Vec::<String>::new()));
    let h = {
        let fires = fires.clone();
        let mut g = GestureGroup::new();
        g.add(LongPress::new(LongPressRecognizer::new(), move || {
            fires.set(fires.get() + 1)
        }));
        g.add(recording_pan(panlog.clone(), "pan"));
        g.handler()
    };
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // 9px: past pan slop (8), within long-press slop (10) — so long-press
    // doesn't self-reject; pan wins and the arbiter cancels it, dropping
    // (and cancelling) its pending timer.
    h(&ev(TouchPhase::Moved, 1, 9.0, 0.0, 16_000_000));
    advance_ms(500);
    assert_eq!(fires.get(), 0, "cancelled long-press timer must not fire");
    assert!(panlog.borrow().contains(&"pan:began".to_string()));
}

// ---------------------------------------------------------------------------
// Deterministic, manually-advanced test scheduler (mirrors the one in
// runtime-core's recognizer tests — see the rationale there). Needed
// because the default no-scheduler path fires `after_ms` synchronously at
// construction, before the long-press recognizer finishes arming.
// ---------------------------------------------------------------------------

struct TestScheduler;

thread_local! {
    static QUEUE: RefCell<Vec<TimerEntry>> = RefCell::new(Vec::new());
    static NOW_MS: Cell<u64> = const { Cell::new(0) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    static CANCELLED: RefCell<std::collections::HashSet<u64>> =
        RefCell::new(std::collections::HashSet::new());
}

struct TimerEntry {
    id: u64,
    fire_at_ms: u64,
    cb: Box<dyn FnOnce() + 'static>,
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
        f();
    }
    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(TestHandle { id: u64::MAX })
    }
    fn after_ms(&self, delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
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
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(TestHandle { id: u64::MAX })
    }
}

fn advance_ms(ms: u64) {
    NOW_MS.with(|n| n.set(n.get() + ms));
    let now = NOW_MS.with(|n| n.get());
    let ready: Vec<TimerEntry> = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
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
    let cancelled: std::collections::HashSet<u64> = CANCELLED.with(|c| c.borrow().clone());
    for entry in ready {
        if !cancelled.contains(&entry.id) {
            (entry.cb)();
        }
    }
}

fn reset_test_clock() {
    QUEUE.with(|q| q.borrow_mut().clear());
    NOW_MS.with(|n| n.set(0));
    NEXT_ID.with(|n| n.set(0));
    CANCELLED.with(|c| c.borrow_mut().clear());
}

fn install_test_scheduler_once() {
    install_scheduler(Box::new(TestScheduler));
}
