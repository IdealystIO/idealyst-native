//! Shared per-frame clock: one frame source fanned out to every `raf_loop`
//! and `after_animation_frame` subscriber.
//!
//! The macOS scheduler drives its animation clock from here (see
//! `scheduler::raf_loop`). Pure Rust and platform-neutral: the platform side
//! is a [`FrameSource`] type parameter, so the scheduling logic (start/stop,
//! re-entrancy, display-vs-backstop gating, one hook per frame) is unit-tested
//! with a fake source and no run loop.
//!
//! # Contract
//!
//! - **One source for all subscribers.** The first subscriber starts the
//!   source; the clock stops it when the last subscriber leaves, so an idle app
//!   has no frame callback scheduled at all.
//! - **Frame semantics match `requestAnimationFrame`.** A subscriber added
//!   during a tick first runs on the NEXT tick. A one-shot runs exactly once. A
//!   subscriber cancelled by an earlier subscriber in the same tick does not
//!   run.
//! - **Two tick origins.** [`TickOrigin::Display`] ticks come from the display
//!   (a `CADisplayLink`) and always run. [`TickOrigin::Timer`] ticks come from
//!   a wall-clock backstop and run only when the display has not ticked for
//!   [`DISPLAY_STALL_US`]. The display link stops calling back when its view
//!   is not on any display (a never-shown or minimized window, see
//!   `-[NSView displayLinkWithTarget:selector:]`); the backstop keeps
//!   animations and one-shots advancing there, at the timer's rate, the same
//!   as a timer-only clock would. While the display ticks, the backstop's
//!   ticks are skipped, so the frames stay on vsync.
//! - **One post-tick hook per frame.** `after_tick` (the scheduler passes the
//!   dispatch hook that schedules the reactive flush) fires once after a tick
//!   in which at least one subscriber ran, not once per subscriber.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::scheduling::ScheduleHandle;

/// How long the display may go without ticking before backstop timer ticks
/// are allowed to run, in microseconds. Three 60 Hz frames: longer than any
/// display interval a live link produces (a 48 Hz adaptive-sync floor is
/// 20.8 ms), short enough that a suspended link hands over to the backstop
/// within a few frames.
pub(crate) const DISPLAY_STALL_US: u64 = 50_000;

/// Where a tick came from. See the module docs for how each is gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TickOrigin {
    /// A display-synchronised callback (`CADisplayLink`).
    Display,
    /// The wall-clock backstop (`NSTimer`).
    Timer,
}

/// The platform side of the clock: something that calls
/// [`FrameClock::on_tick`] every frame between `start` and `stop`.
///
/// `start`/`stop` are called while the clock's state is borrowed, so they must
/// not call back into the clock synchronously.
pub(crate) trait FrameSource {
    /// Begin delivering ticks. Called when the first subscriber arrives.
    fn start(&mut self);
    /// Stop delivering ticks. Called when the last subscriber leaves.
    fn stop(&mut self);
}

enum Callback {
    Every(Box<dyn FnMut() + 'static>),
    Once(Option<Box<dyn FnOnce() + 'static>>),
}

struct Entry {
    id: u64,
    alive: Rc<Cell<bool>>,
    callback: Rc<RefCell<Callback>>,
}

struct Inner<S> {
    entries: Vec<Entry>,
    next_id: u64,
    /// The source is started (between `start` and `stop`).
    running: bool,
    /// A tick is invoking subscribers right now.
    ticking: bool,
    /// When the display last ticked, in the caller's microsecond clock.
    /// Cleared on stop so a restart does not inherit a stale value.
    last_display_tick_us: Option<u64>,
    source: S,
}

/// The fan-out clock. Hold it in an `Rc`; subscriptions keep a `Weak` to it.
pub(crate) struct FrameClock<S: FrameSource> {
    inner: RefCell<Inner<S>>,
    after_tick: fn(),
}

/// Removes one subscription. Object-safe so a subscription doesn't carry the
/// source type.
trait Unsubscribe {
    fn unsubscribe(&self, id: u64);
}

impl<S: FrameSource + 'static> FrameClock<S> {
    pub(crate) fn new(source: S, after_tick: fn()) -> Rc<Self> {
        Rc::new(Self {
            inner: RefCell::new(Inner {
                entries: Vec::new(),
                next_id: 0,
                running: false,
                ticking: false,
                last_display_tick_us: None,
                source,
            }),
            after_tick,
        })
    }

    /// Run `f` on every tick until the returned subscription is dropped or
    /// cancelled.
    pub(crate) fn subscribe_every(
        self: &Rc<Self>,
        f: Box<dyn FnMut() + 'static>,
    ) -> FrameSubscription {
        self.subscribe(Callback::Every(f))
    }

    /// Run `f` on the next tick, once. Dropping or cancelling the subscription
    /// before then cancels it.
    pub(crate) fn subscribe_once(
        self: &Rc<Self>,
        f: Box<dyn FnOnce() + 'static>,
    ) -> FrameSubscription {
        self.subscribe(Callback::Once(Some(f)))
    }

    fn subscribe(self: &Rc<Self>, callback: Callback) -> FrameSubscription {
        let alive = Rc::new(Cell::new(true));
        let id = {
            let mut inner = self.inner.borrow_mut();
            let id = inner.next_id;
            inner.next_id += 1;
            inner.entries.push(Entry {
                id,
                alive: alive.clone(),
                callback: Rc::new(RefCell::new(callback)),
            });
            if !inner.running {
                inner.running = true;
                inner.source.start();
            }
            id
        };
        let weak: Weak<Self> = Rc::downgrade(self);
        let clock: Weak<dyn Unsubscribe> = weak;
        FrameSubscription { clock, id, alive: Some(alive) }
    }

    /// One frame from the source. `now_us` is a monotonic microsecond clock
    /// (any epoch, the same for every call). Returns whether any subscriber
    /// ran.
    pub(crate) fn on_tick(&self, origin: TickOrigin, now_us: u64) -> bool {
        let snapshot: Vec<(Rc<Cell<bool>>, Rc<RefCell<Callback>>)> = {
            let mut inner = self.inner.borrow_mut();
            if !inner.running || inner.ticking {
                return false;
            }
            match origin {
                TickOrigin::Display => inner.last_display_tick_us = Some(now_us),
                TickOrigin::Timer => {
                    if let Some(last) = inner.last_display_tick_us {
                        if now_us.saturating_sub(last) < DISPLAY_STALL_US {
                            return false;
                        }
                    }
                }
            }
            inner.ticking = true;
            let snapshot = inner
                .entries
                .iter()
                .map(|e| (e.alive.clone(), e.callback.clone()))
                .collect();
            // One-shots leave the list now: they run this tick and never again.
            inner
                .entries
                .retain(|e| matches!(*e.callback.borrow(), Callback::Every(_)));
            snapshot
        };

        let mut ran = false;
        for (alive, callback) in &snapshot {
            if !alive.get() {
                continue;
            }
            ran = true;
            let once = match &mut *callback.borrow_mut() {
                Callback::Once(f) => {
                    alive.set(false);
                    f.take()
                }
                Callback::Every(_) => None,
            };
            match once {
                Some(f) => f(),
                None => {
                    // Not `Once`: the borrow is held while the closure runs, so
                    // a closure can't be dropped mid-call (its entry's `Rc` is
                    // also held by `snapshot`).
                    if let Callback::Every(f) = &mut *callback.borrow_mut() {
                        f();
                    }
                }
            }
        }

        {
            let mut inner = self.inner.borrow_mut();
            inner.ticking = false;
            if inner.entries.is_empty() && inner.running {
                inner.running = false;
                inner.last_display_tick_us = None;
                inner.source.stop();
            }
        }
        // Closures (and whatever they captured) drop outside any clock borrow,
        // so a captured subscription unsubscribing from its `Drop` is fine.
        drop(snapshot);
        if ran {
            (self.after_tick)();
        }
        ran
    }

    /// Whether the source is started.
    pub(crate) fn is_running(&self) -> bool {
        self.inner.borrow().running
    }

    /// Live subscribers (one-shots that have not fired yet included).
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.inner.borrow().entries.len()
    }

    /// Reach the source, with whether it is started. For swapping what the
    /// source is bound to (the macOS display link's owner view).
    pub(crate) fn with_source<R>(&self, f: impl FnOnce(&mut S, bool) -> R) -> R {
        let mut inner = self.inner.borrow_mut();
        let running = inner.running;
        f(&mut inner.source, running)
    }
}

impl<S: FrameSource + 'static> Unsubscribe for FrameClock<S> {
    fn unsubscribe(&self, id: u64) {
        let removed: Vec<Entry> = {
            let mut inner = self.inner.borrow_mut();
            let (removed, kept): (Vec<Entry>, Vec<Entry>) =
                std::mem::take(&mut inner.entries).into_iter().partition(|e| e.id == id);
            inner.entries = kept;
            if inner.entries.is_empty() && inner.running && !inner.ticking {
                inner.running = false;
                inner.last_display_tick_us = None;
                inner.source.stop();
            }
            removed
        };
        // Dropped outside the borrow: a closure's captures may unsubscribe too.
        drop(removed);
    }
}

/// A live `raf_loop` / `after_animation_frame` registration. Dropping or
/// cancelling it removes the subscriber; the last one to go stops the source.
pub(crate) struct FrameSubscription {
    clock: Weak<dyn Unsubscribe>,
    id: u64,
    /// `None` once cancelled (cancel is idempotent).
    alive: Option<Rc<Cell<bool>>>,
}

impl FrameSubscription {
    fn cancel_inner(&mut self) {
        let Some(alive) = self.alive.take() else {
            return;
        };
        // Mark first: a tick already holding this subscriber in its snapshot
        // skips it.
        alive.set(false);
        // The clock is gone at thread teardown; nothing left to remove from.
        if let Some(clock) = self.clock.upgrade() {
            clock.unsubscribe(self.id);
        }
    }
}

impl ScheduleHandle for FrameSubscription {
    fn cancel(&mut self) {
        self.cancel_inner();
    }
}

impl Drop for FrameSubscription {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static HOOK_FIRES: Cell<u32> = const { Cell::new(0) };
    }

    fn count_hook() {
        HOOK_FIRES.with(|c| c.set(c.get() + 1));
    }

    fn hook_fires() -> u32 {
        HOOK_FIRES.with(|c| c.get())
    }

    /// Records start/stop calls so tests can see the source's lifecycle.
    #[derive(Default)]
    struct FakeSource {
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl FrameSource for FakeSource {
        fn start(&mut self) {
            self.log.borrow_mut().push("start");
        }
        fn stop(&mut self) {
            self.log.borrow_mut().push("stop");
        }
    }

    fn clock() -> (Rc<FrameClock<FakeSource>>, Rc<RefCell<Vec<&'static str>>>) {
        HOOK_FIRES.with(|c| c.set(0));
        let log = Rc::new(RefCell::new(Vec::new()));
        (FrameClock::new(FakeSource { log: log.clone() }, count_hook), log)
    }

    fn counter() -> (Rc<Cell<u32>>, Box<dyn FnMut() + 'static>) {
        let n = Rc::new(Cell::new(0));
        let n2 = n.clone();
        (n, Box::new(move || n2.set(n2.get() + 1)))
    }

    /// No idle frame callback: the source starts with the first subscriber and
    /// stops when the last one is dropped.
    #[test]
    fn source_runs_only_while_something_is_subscribed() {
        let (clock, log) = clock();
        assert!(!clock.is_running());
        let (_, f) = counter();
        let a = clock.subscribe_every(f);
        let (_, g) = counter();
        let b = clock.subscribe_every(g);
        assert_eq!(*log.borrow(), vec!["start"], "one source for every subscriber");
        drop(a);
        assert!(clock.is_running(), "one subscriber left");
        drop(b);
        assert!(!clock.is_running());
        assert_eq!(*log.borrow(), vec!["start", "stop"]);
        // Restarts on demand.
        let (_, h) = counter();
        let _c = clock.subscribe_every(h);
        assert_eq!(*log.borrow(), vec!["start", "stop", "start"]);
    }

    /// Several `raf_loop`s share one tick, and the flush hook fires once per
    /// frame rather than once per loop.
    #[test]
    fn subscribers_share_a_tick_and_the_hook_fires_once_per_frame() {
        let (clock, _) = clock();
        let (a_n, a) = counter();
        let (b_n, b) = counter();
        let _a = clock.subscribe_every(a);
        let _b = clock.subscribe_every(b);
        for frame in 0..3u64 {
            assert!(clock.on_tick(TickOrigin::Display, frame * 8_333));
        }
        assert_eq!((a_n.get(), b_n.get()), (3, 3));
        assert_eq!(hook_fires(), 3, "one hook per frame, not per subscriber");
    }

    /// `after_animation_frame` semantics: a one-shot fires on the next tick,
    /// once, and the source stops after it if nothing else is subscribed.
    #[test]
    fn one_shot_fires_once_then_the_source_stops() {
        let (clock, log) = clock();
        let n = Rc::new(Cell::new(0));
        let n2 = n.clone();
        let _sub = clock.subscribe_once(Box::new(move || n2.set(n2.get() + 1)));
        assert_eq!(clock.subscriber_count(), 1);
        clock.on_tick(TickOrigin::Display, 0);
        clock.on_tick(TickOrigin::Display, 8_333);
        assert_eq!(n.get(), 1);
        assert_eq!(*log.borrow(), vec!["start", "stop"], "stops after the last one-shot");
        assert_eq!(hook_fires(), 1, "a one-shot's writes get their flush");
    }

    #[test]
    fn a_cancelled_one_shot_never_runs() {
        let (clock, log) = clock();
        let n = Rc::new(Cell::new(0));
        let n2 = n.clone();
        let sub = clock.subscribe_once(Box::new(move || n2.set(1)));
        drop(sub);
        assert!(!clock.is_running());
        assert!(!clock.on_tick(TickOrigin::Display, 0), "a stray tick after stop runs nothing");
        assert_eq!(n.get(), 0);
        assert_eq!(*log.borrow(), vec!["start", "stop"]);
        assert_eq!(hook_fires(), 0, "no subscriber ran, so no flush");
    }

    /// rAF semantics: subscribing from inside a tick runs on the next tick.
    #[test]
    fn a_subscriber_added_during_a_tick_first_runs_next_tick() {
        let (clock, _) = clock();
        let inner_n = Rc::new(Cell::new(0));
        let held: Rc<RefCell<Vec<FrameSubscription>>> = Rc::new(RefCell::new(Vec::new()));
        let (c, i, h) = (clock.clone(), inner_n.clone(), held.clone());
        let _outer = clock.subscribe_once(Box::new(move || {
            let i = i.clone();
            h.borrow_mut().push(c.subscribe_every(Box::new(move || i.set(i.get() + 1))));
        }));
        clock.on_tick(TickOrigin::Display, 0);
        assert_eq!(inner_n.get(), 0, "added this tick, so not run this tick");
        assert!(clock.is_running(), "the new subscriber keeps the source running");
        clock.on_tick(TickOrigin::Display, 8_333);
        assert_eq!(inner_n.get(), 1);
    }

    /// The animation clock drops its `raf_loop` from inside its own tick when
    /// the last tween finishes. That must not panic (no clock borrow is held
    /// while subscribers run) and must stop the source at the end of the tick.
    #[test]
    fn a_loop_cancelling_itself_mid_tick_stops_the_source() {
        let (clock, log) = clock();
        let slot: Rc<RefCell<Option<FrameSubscription>>> = Rc::new(RefCell::new(None));
        let n = Rc::new(Cell::new(0));
        let (s, n2) = (slot.clone(), n.clone());
        let sub = clock.subscribe_every(Box::new(move || {
            n2.set(n2.get() + 1);
            if n2.get() == 2 {
                s.borrow_mut().take();
            }
        }));
        *slot.borrow_mut() = Some(sub);
        clock.on_tick(TickOrigin::Display, 0);
        clock.on_tick(TickOrigin::Display, 8_333);
        assert!(!clock.is_running());
        assert!(!clock.on_tick(TickOrigin::Display, 16_666));
        assert_eq!(n.get(), 2);
        assert_eq!(*log.borrow(), vec!["start", "stop"]);
    }

    /// A subscriber cancelled by an earlier one in the same tick is skipped.
    #[test]
    fn a_subscriber_cancelled_earlier_in_the_tick_is_skipped() {
        let (clock, _) = clock();
        let victim_slot: Rc<RefCell<Option<FrameSubscription>>> = Rc::new(RefCell::new(None));
        let vs = victim_slot.clone();
        let _killer = clock.subscribe_every(Box::new(move || {
            vs.borrow_mut().take();
        }));
        let (victim_n, victim) = counter();
        *victim_slot.borrow_mut() = Some(clock.subscribe_every(victim));
        clock.on_tick(TickOrigin::Display, 0);
        assert_eq!(victim_n.get(), 0);
    }

    /// While the display ticks, the backstop timer's ticks are skipped, so
    /// frames stay on vsync instead of interleaving two clocks.
    #[test]
    fn backstop_ticks_are_skipped_while_the_display_ticks() {
        let (clock, _) = clock();
        let (n, f) = counter();
        let _sub = clock.subscribe_every(f);
        assert!(clock.on_tick(TickOrigin::Display, 0));
        assert!(!clock.on_tick(TickOrigin::Timer, 16_000));
        assert!(clock.on_tick(TickOrigin::Display, 8_333));
        assert!(!clock.on_tick(TickOrigin::Timer, 8_333 + DISPLAY_STALL_US - 1));
        assert_eq!(n.get(), 2);
    }

    /// The display link is silent for a view that is not on any display (a
    /// window never shown, or minimized). The backstop then advances the clock.
    #[test]
    fn backstop_ticks_when_the_display_is_silent() {
        let (clock, _) = clock();
        let (n, f) = counter();
        let _sub = clock.subscribe_every(f);
        // Never a display tick (no link, or a link that never fires).
        assert!(clock.on_tick(TickOrigin::Timer, 0));
        assert!(clock.on_tick(TickOrigin::Timer, 16_667));
        // The display ticked, then went silent past the stall window.
        assert!(clock.on_tick(TickOrigin::Display, 20_000));
        assert!(!clock.on_tick(TickOrigin::Timer, 33_333));
        assert!(clock.on_tick(TickOrigin::Timer, 20_000 + DISPLAY_STALL_US));
        assert_eq!(n.get(), 4);
    }

    /// A restart forgets the last display tick: the first frames after a
    /// restart must not be suppressed by a display tick from a previous run.
    #[test]
    fn a_restart_does_not_inherit_the_previous_display_tick() {
        let (clock, _) = clock();
        let (_, f) = counter();
        let sub = clock.subscribe_every(f);
        clock.on_tick(TickOrigin::Display, 1_000_000);
        drop(sub);
        let (n, g) = counter();
        let _sub = clock.subscribe_every(g);
        assert!(clock.on_tick(TickOrigin::Timer, 1_000_010), "no display tick in this run yet");
        assert_eq!(n.get(), 1);
    }

    /// Thread teardown can drop the clock before a subscription (the
    /// animation clock's own registry lives in a thread-local too).
    #[test]
    fn a_subscription_outliving_its_clock_drops_quietly() {
        let (clock, _) = clock();
        let (_, f) = counter();
        let sub = clock.subscribe_every(f);
        drop(clock);
        drop(sub);
    }

    #[test]
    fn cancel_is_idempotent() {
        let (clock, log) = clock();
        let (_, f) = counter();
        let mut sub = clock.subscribe_every(f);
        sub.cancel();
        sub.cancel();
        drop(sub);
        assert_eq!(*log.borrow(), vec!["start", "stop"]);
    }
}
