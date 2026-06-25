//! Unit tests driving synthetic [`TouchEvent`]s — the same technique the core
//! recognizer tests and the `pan` SDK use. Drop-zone geometry is supplied as
//! fixed rects (rather than a mounted `ViewHandle`) so the registry +
//! hit-testing + payload delivery are exercised without a backend.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{Signal, TouchEvent, TouchId, TouchPhase, TouchPoint, ViewportRect};

use crate::context::{DroppableEntry, DroppableId};
use crate::recognizer::{Activation, DragPhase, DragRecognizer};
use crate::{DragContext, Draggable, DropOutcome};

// ---------------------------------------------------------------------------
// Manually-advanced test scheduler — mirrors the one in
// `runtime_core::touch::recognizers` tests. Without it, the native no-scheduler
// fallback runs `after_ms` callbacks *synchronously at call time*, firing the
// long-press timer before the recognizer has set up its state. We hold the
// callback pending and release it on an explicit clock advance.
// ---------------------------------------------------------------------------

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
    let cancelled: HashSet<u64> = CANCELLED.with(|c| c.borrow().clone());
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

/// First call wins (OnceLock); every test calls it but only the first installs.
fn install_test_scheduler_once() {
    install_scheduler(Box::new(TestScheduler));
}

fn ev(phase: TouchPhase, id: u64, x: f32, y: f32, ts_ns: u64) -> TouchEvent {
    TouchEvent {
        id: TouchId(id),
        phase,
        // view-local == window for these tests (single un-nested view).
        position: TouchPoint::new(x, y),
        window_position: TouchPoint::new(x, y),
        timestamp_ns: ts_ns,
        force: None,
    }
}

/// Register a fixed-rect drop zone directly into a context, returning its id.
/// Bypasses `Droppable::bind` (which needs a mounted view) so geometry is
/// deterministic in tests.
fn register_zone<T: Clone + 'static>(
    ctx: &DragContext<T>,
    rect: ViewportRect,
    is_over: Signal<bool>,
    accepts: Rc<dyn Fn(&T) -> bool>,
    on_enter: Option<Rc<dyn Fn(&T)>>,
    on_leave: Option<Rc<dyn Fn()>>,
    on_drop: Option<Rc<dyn Fn(T)>>,
) -> DroppableId {
    let id = DroppableId::next();
    ctx.register(DroppableEntry {
        id,
        rect: Rc::new(move || Some(rect)),
        accepts,
        is_over,
        on_enter,
        on_leave,
        on_drop,
    });
    id
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> ViewportRect {
    ViewportRect {
        x,
        y,
        width: w,
        height: h,
    }
}

// ---------------------------------------------------------------------------
// DragRecognizer — Immediate activation (fully synchronous, no timer)
// ---------------------------------------------------------------------------

#[test]
fn immediate_drag_emits_began_moved_ended() {
    let phases: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = phases.clone();
    let last_delta = Rc::new(Cell::new(TouchPoint::ZERO));
    let delta_sink = last_delta.clone();
    let h = DragRecognizer::new(Activation::immediate(), move |p| {
        match p {
            DragPhase::Began(_) => sink.borrow_mut().push("began"),
            DragPhase::Moved(s) => {
                sink.borrow_mut().push("moved");
                delta_sink.set(s.delta);
            }
            DragPhase::Ended { .. } => sink.borrow_mut().push("ended"),
            DragPhase::Cancelled => sink.borrow_mut().push("cancelled"),
        };
    })
    .into_handler();

    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    h(&ev(TouchPhase::Moved, 1, 40.0, 10.0, 16_000_000));
    h(&ev(TouchPhase::Ended, 1, 40.0, 10.0, 32_000_000));

    // Began, then the immediate zero-delta Moved, then the real Moved, Ended.
    let seq = phases.borrow().clone();
    assert_eq!(seq.first(), Some(&"began"));
    assert_eq!(seq.last(), Some(&"ended"));
    assert!(seq.contains(&"moved"));
    assert_eq!(
        last_delta.get(),
        TouchPoint::new(40.0, 10.0),
        "delta is cumulative from the commit position"
    );
}

#[test]
fn sub_slop_wobble_does_not_commit() {
    let committed = Rc::new(Cell::new(false));
    let sink = committed.clone();
    let h = DragRecognizer::new(Activation::immediate(), move |p| {
        if matches!(p, DragPhase::Began(_)) {
            sink.set(true);
        }
    })
    .into_handler();

    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // 3 px — under the 8 px immediate slop.
    h(&ev(TouchPhase::Moved, 1, 3.0, 0.0, 16_000_000));
    h(&ev(TouchPhase::Ended, 1, 3.0, 0.0, 32_000_000));
    assert!(!committed.get(), "a sub-slop touch must not begin a drag");
}

#[test]
fn long_press_commits_after_hold() {
    install_test_scheduler_once();
    reset_test_clock();
    let committed = Rc::new(Cell::new(false));
    let sink = committed.clone();
    let h = DragRecognizer::new(Activation::long_press(), move |p| {
        if matches!(p, DragPhase::Began(_)) {
            sink.set(true);
        }
    })
    .into_handler();

    h(&ev(TouchPhase::Began, 1, 10.0, 10.0, 0));
    // Hold still (within slop) and let the hold threshold elapse.
    assert!(!committed.get(), "no commit before the hold elapses");
    advance_ms(crate::DEFAULT_DRAG_LONG_PRESS_MS);
    assert!(committed.get(), "drag commits after the hold threshold");
    // Now it tracks like any drag.
    let r = h(&ev(TouchPhase::Moved, 1, 40.0, 10.0, 16_000_000));
    assert!(r.consumed && r.claim, "an active drag claims the touch");
    h(&ev(TouchPhase::Ended, 1, 40.0, 10.0, 32_000_000));
}

#[test]
fn long_press_abandons_when_finger_moves_before_hold() {
    install_test_scheduler_once();
    reset_test_clock();
    let committed = Rc::new(Cell::new(false));
    let sink = committed.clone();
    let h = DragRecognizer::new(Activation::long_press(), move |p| {
        if matches!(p, DragPhase::Began(_)) {
            sink.set(true);
        }
    })
    .into_handler();

    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // Move 30 px (past the 10 px long-press slop) before the hold elapses.
    let r = h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    // Consumed (we owned the touch) but NOT claimed (left to native scroll).
    assert!(r.consumed && !r.claim);
    // Even once the timer would have fired, the drag was abandoned.
    advance_ms(crate::DEFAULT_DRAG_LONG_PRESS_MS);
    h(&ev(TouchPhase::Ended, 1, 30.0, 0.0, 32_000_000));
    assert!(!committed.get(), "moving before the hold must not start a drag");
}

// ---------------------------------------------------------------------------
// DragContext — hit-testing, hover edges, payload delivery
// ---------------------------------------------------------------------------

#[test]
fn hover_enter_leave_edges_fire_once() {
    let ctx: DragContext<u64> = DragContext::new();
    let is_over = Signal::new(false);
    let enters = Rc::new(Cell::new(0u32));
    let leaves = Rc::new(Cell::new(0u32));
    let e = enters.clone();
    let l = leaves.clone();
    register_zone(
        &ctx,
        rect(100.0, 0.0, 50.0, 50.0),
        is_over,
        Rc::new(|_| true),
        Some(Rc::new(move |_| e.set(e.get() + 1))),
        Some(Rc::new(move || l.set(l.get() + 1))),
        None,
    );

    ctx.begin(7);
    // Start outside the zone.
    ctx.update(TouchPoint::new(0.0, 0.0));
    assert!(!is_over.get());
    // Move in.
    ctx.update(TouchPoint::new(120.0, 25.0));
    assert!(is_over.get(), "is_over flips true on enter");
    // Move within — no extra enter.
    ctx.update(TouchPoint::new(130.0, 25.0));
    // Move out.
    ctx.update(TouchPoint::new(0.0, 0.0));
    assert!(!is_over.get(), "is_over flips false on leave");

    assert_eq!(enters.get(), 1, "on_enter fires exactly once");
    assert_eq!(leaves.get(), 1, "on_leave fires exactly once");
}

#[test]
fn drop_delivers_payload_and_clears_state() {
    let ctx: DragContext<u64> = DragContext::new();
    let is_over = Signal::new(false);
    let dropped: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = dropped.clone();
    register_zone(
        &ctx,
        rect(100.0, 0.0, 50.0, 50.0),
        is_over,
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(move |p| sink.borrow_mut().push(p))),
    );

    ctx.begin(42);
    ctx.update(TouchPoint::new(120.0, 25.0));
    assert!(ctx.dragging().get());
    let landed = ctx.finish(TouchPoint::new(120.0, 25.0));

    assert!(landed, "finish over an accepting target returns true");
    assert_eq!(*dropped.borrow(), vec![42]);
    assert!(!ctx.dragging().get(), "dragging clears after finish");
    assert!(!is_over.get(), "hover clears after finish");
    assert!(ctx.payload().is_none(), "payload cleared after finish");
}

#[test]
fn drop_fires_on_leave_so_hover_visual_resets() {
    // Regression: a drop ON a hovered target must still fire its `on_leave`
    // (and clear `is_over`), or a callback-driven highlight stays stuck "on"
    // after release.
    let ctx: DragContext<u64> = DragContext::new();
    let is_over = Signal::new(false);
    let leaves = Rc::new(Cell::new(0u32));
    let l = leaves.clone();
    register_zone(
        &ctx,
        rect(0.0, 0.0, 100.0, 100.0),
        is_over,
        Rc::new(|_| true),
        None,
        Some(Rc::new(move || l.set(l.get() + 1))),
        Some(Rc::new(|_| {})),
    );

    ctx.begin(1);
    ctx.update(TouchPoint::new(50.0, 50.0)); // hover the zone (is_over → true)
    assert!(is_over.get());
    assert!(ctx.finish(TouchPoint::new(50.0, 50.0)), "drops on the zone");
    assert!(!is_over.get(), "is_over clears on drop");
    assert_eq!(leaves.get(), 1, "on_leave fires on drop so the highlight resets");
}

#[test]
fn drop_outside_any_target_returns_false() {
    let ctx: DragContext<u64> = DragContext::new();
    register_zone(
        &ctx,
        rect(100.0, 0.0, 50.0, 50.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(|_| {})),
    );
    ctx.begin(1);
    let landed = ctx.finish(TouchPoint::new(0.0, 0.0)); // miss
    assert!(!landed);
    assert!(!ctx.dragging().get());
}

#[test]
fn accepts_predicate_rejects_incompatible_payload() {
    let ctx: DragContext<u64> = DragContext::new();
    let is_over = Signal::new(false);
    // Only accepts even payloads.
    register_zone(
        &ctx,
        rect(0.0, 0.0, 100.0, 100.0),
        is_over,
        Rc::new(|p: &u64| p % 2 == 0),
        None,
        None,
        Some(Rc::new(|_| {})),
    );

    ctx.begin(3); // odd — rejected
    ctx.update(TouchPoint::new(50.0, 50.0));
    assert!(!is_over.get(), "rejected payload never hovers");
    assert!(
        !ctx.finish(TouchPoint::new(50.0, 50.0)),
        "rejected payload never drops"
    );
}

#[test]
fn nested_targets_innermost_wins() {
    let ctx: DragContext<u64> = DragContext::new();
    let dropped: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let outer_sink = dropped.clone();
    let inner_sink = dropped.clone();
    // Big outer, small inner, both contain (50,50).
    register_zone(
        &ctx,
        rect(0.0, 0.0, 200.0, 200.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(move |_| outer_sink.borrow_mut().push("outer"))),
    );
    register_zone(
        &ctx,
        rect(40.0, 40.0, 40.0, 40.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(move |_| inner_sink.borrow_mut().push("inner"))),
    );

    ctx.begin(1);
    ctx.finish(TouchPoint::new(50.0, 50.0));
    assert_eq!(
        *dropped.borrow(),
        vec!["inner"],
        "smallest-area (innermost) target wins"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: Draggable handler driving a registered drop zone
// ---------------------------------------------------------------------------

#[test]
fn draggable_lands_payload_on_zone() {
    let ctx: DragContext<u64> = DragContext::new();
    let dropped: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let drop_sink = dropped.clone();
    register_zone(
        &ctx,
        rect(100.0, 0.0, 100.0, 100.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(move |p| drop_sink.borrow_mut().push(p))),
    );

    let outcomes: Rc<RefCell<Vec<DropOutcome>>> = Rc::new(RefCell::new(Vec::new()));
    let out_sink = outcomes.clone();
    let drag = Draggable::new(&ctx, || 99u64)
        .activation(Activation::immediate())
        .on_release(move |o| out_sink.borrow_mut().push(o));
    let h = drag.handler();

    // Press at origin, drag into the zone at (150,50), release there.
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    h(&ev(TouchPhase::Moved, 1, 150.0, 50.0, 16_000_000));
    h(&ev(TouchPhase::Ended, 1, 150.0, 50.0, 32_000_000));

    assert_eq!(*dropped.borrow(), vec![99], "payload delivered to the zone");
    assert_eq!(*outcomes.borrow(), vec![DropOutcome::Landed]);
}

#[test]
fn draggable_miss_reports_missed_outcome() {
    let ctx: DragContext<u64> = DragContext::new();
    register_zone(
        &ctx,
        rect(100.0, 0.0, 100.0, 100.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        None,
        Some(Rc::new(|_| panic!("must not drop on a miss"))),
    );

    let outcomes: Rc<RefCell<Vec<DropOutcome>>> = Rc::new(RefCell::new(Vec::new()));
    let out_sink = outcomes.clone();
    let drag = Draggable::new(&ctx, || 1u64)
        .activation(Activation::immediate())
        .on_release(move |o| out_sink.borrow_mut().push(o));
    let h = drag.handler();

    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    // Drag 30 px — past slop, but nowhere near the zone at x>=100.
    h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    h(&ev(TouchPhase::Ended, 1, 30.0, 0.0, 32_000_000));

    assert_eq!(*outcomes.borrow(), vec![DropOutcome::Missed]);
    assert!(!ctx.dragging().get());
}

#[test]
fn draggable_cancel_reports_cancelled_and_clears() {
    let ctx: DragContext<u64> = DragContext::new();
    let leaves = Rc::new(Cell::new(0u32));
    let l = leaves.clone();
    register_zone(
        &ctx,
        rect(0.0, 0.0, 200.0, 200.0),
        Signal::new(false),
        Rc::new(|_| true),
        None,
        Some(Rc::new(move || l.set(l.get() + 1))),
        Some(Rc::new(|_| {})),
    );

    let outcomes: Rc<RefCell<Vec<DropOutcome>>> = Rc::new(RefCell::new(Vec::new()));
    let out_sink = outcomes.clone();
    let drag = Draggable::new(&ctx, || 1u64)
        .activation(Activation::immediate())
        .on_release(move |o| out_sink.borrow_mut().push(o));
    let h = drag.handler();

    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    h(&ev(TouchPhase::Moved, 1, 50.0, 50.0, 16_000_000)); // active, over the zone
    h(&ev(TouchPhase::Cancelled, 1, 50.0, 50.0, 32_000_000));

    assert_eq!(*outcomes.borrow(), vec![DropOutcome::Cancelled]);
    assert_eq!(leaves.get(), 1, "cancel fires the hovered target's on_leave");
    assert!(!ctx.dragging().get());
}

#[test]
fn ghost_position_tracks_release_point_for_drop_animation() {
    // The drop-animation hand-off (reveal the hidden source where the ghost was
    // let go, then spring it into its slot) depends on `ghost_position()`
    // reporting the ghost's window top-left = pointer − grab-offset, AND on that
    // value surviving the release so `on_release` can read it.
    install_test_scheduler_once();
    let ctx: DragContext<u64> = DragContext::new();

    let drag = Draggable::new(&ctx, || 7u64)
        .activation(Activation::immediate())
        .preview(|| runtime_core::fragment(Vec::new()));
    let h = drag.handler();

    // Press at (10,20); the drag commits on the first move past the 8 px slop —
    // that move's position is the grab offset (where in the element the finger
    // sits). View == window in these tests, so grab offset = (10,40).
    h(&ev(TouchPhase::Began, 1, 10.0, 20.0, 0));
    h(&ev(TouchPhase::Moved, 1, 10.0, 40.0, 16_000_000));
    // A real move: ghost top-left = pointer − grab offset = (60−10, 90−40).
    h(&ev(TouchPhase::Moved, 1, 60.0, 90.0, 32_000_000));
    assert_eq!(
        ctx.ghost_position(),
        (50.0, 50.0),
        "ghost top-left = pointer − grab offset"
    );

    // Persists through release so the drop animation can anchor to it.
    h(&ev(TouchPhase::Ended, 1, 60.0, 90.0, 48_000_000));
    assert_eq!(ctx.ghost_position(), (50.0, 50.0));
}

#[test]
fn long_press_commit_claims_off_stream() {
    // The fix for "iOS scroll view steals the drag": a LongPress commits from
    // the timer, off the touch stream, so it must invoke the backend's
    // node-bound claim THERE (cancelling the ancestor scroller before the first
    // move) rather than waiting for the next `Moved` — by which point the native
    // pan has already recognized and cancelled the touch.
    install_test_scheduler_once();
    reset_test_clock();
    let claims = Rc::new(Cell::new(0u32));
    let sink = claims.clone();
    // Stand in for the backend publishing a claim closure for the in-flight
    // touch (iOS does this on `Began`, scoped to the synchronous dispatch).
    runtime_core::set_active_touch_claim(Some(Rc::new(move || sink.set(sink.get() + 1))));

    let h = DragRecognizer::new(Activation::long_press(), |_| {}).into_handler();
    h(&ev(TouchPhase::Began, 1, 10.0, 10.0, 0));
    // The backend clears it after dispatch — so the recognizer must have grabbed
    // its OWN clone on `Began` for the off-stream commit to still claim.
    runtime_core::set_active_touch_claim(None);

    assert_eq!(claims.get(), 0, "no claim before the hold elapses");
    advance_ms(crate::DEFAULT_DRAG_LONG_PRESS_MS);
    assert_eq!(claims.get(), 1, "the long-press commit claims off-stream, at commit time");

    h(&ev(TouchPhase::Ended, 1, 10.0, 10.0, 32_000_000));
}

#[test]
fn abandoned_long_press_never_claims() {
    // A finger that moves past slop before the hold is a SCROLL, not a drag —
    // it must never claim, leaving the gesture to the native scroll container.
    install_test_scheduler_once();
    reset_test_clock();
    let claims = Rc::new(Cell::new(0u32));
    let sink = claims.clone();
    runtime_core::set_active_touch_claim(Some(Rc::new(move || sink.set(sink.get() + 1))));

    let h = DragRecognizer::new(Activation::long_press(), |_| {}).into_handler();
    h(&ev(TouchPhase::Began, 1, 0.0, 0.0, 0));
    runtime_core::set_active_touch_claim(None);
    // Past the 10 px long-press slop before the hold → abandon.
    h(&ev(TouchPhase::Moved, 1, 30.0, 0.0, 16_000_000));
    advance_ms(crate::DEFAULT_DRAG_LONG_PRESS_MS);
    assert_eq!(claims.get(), 0, "an abandoned long-press leaves the touch to native scroll");

    h(&ev(TouchPhase::Ended, 1, 30.0, 0.0, 32_000_000));
}
