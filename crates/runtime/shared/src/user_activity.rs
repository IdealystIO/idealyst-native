//! App-wide "the person is using the app" heartbeat — the substrate under
//! `user_activity()`.
//!
//! An app cannot see every touch through its own views: a native touch
//! reaches a parent's `on_touch` only when the view under the finger
//! passes it up (buttons, text fields and scroll views don't), and
//! portals (modals, sheets) mount outside the app tree. Idle clocks —
//! kiosk "still there?" prompts, session timeouts, auto-lock — need every
//! interaction, so each backend installs one passive observer at the
//! window level that calls [`report`] on every pointer down, touch begin,
//! key press, wheel or scroll, from any window or portal.
//!
//! ## Contract for backends
//!
//! - **Observe only.** The observer never consumes, delays or reorders an
//!   event (iOS: a never-recognizing gesture recognizer; Android: a
//!   `Window.Callback` wrapper that calls through; web: passive
//!   capture-phase listeners; macOS: an `NSEvent` monitor that returns the
//!   event; GTK / Win32: the window's own dispatch).
//! - **Installed at boot, unconditionally.** Unlike `key_input` (whose
//!   source is installed only while a listener is live because iOS and
//!   Android must take focus to receive hardware keys), the activity
//!   source has no side effects, so it is always on.
//! - Call [`report`] from the raw platform callback; when it returns
//!   `true`, schedule the backend's flush so world signals commit.
//!
//! ## Throttling
//!
//! [`report`] ticks at most once per [`MIN_TICK_INTERVAL_MICROS`] (one
//! 60 Hz frame) so a scroll or wheel burst doesn't re-run every subscriber
//! per event. A dropped report is at most one frame newer than the tick
//! before it, which an idle clock measured in seconds cannot notice. The
//! throttle reads [`crate::time::now_micros`]; with no time source
//! installed that reads `0` forever, and the throttle is skipped instead
//! of freezing the counter at its first tick.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Minimum spacing between two ticks: one frame at 60 Hz. Activity is a
/// liveness heartbeat, not an event stream — per-event consumers use the
/// event handlers themselves.
pub const MIN_TICK_INTERVAL_MICROS: u64 = 16_667;

type Handler = Rc<dyn Fn(u64)>;

runtime_tls::thread_local! {
    /// Ticks so far. Monotonic for the thread's life.
    static COUNT: Cell<u64> = const { Cell::new(0) };
    /// Clock reading of the last tick; `None` before the first.
    static LAST_TICK_AT: Cell<Option<u64>> = const { Cell::new(None) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    static LISTENERS: RefCell<Vec<(u64, Handler)>> = const { RefCell::new(Vec::new()) };
}

/// The number of activity ticks so far (`0` before the first interaction).
pub fn count() -> u64 {
    COUNT.with(Cell::get)
}

/// Backend entry point: the person touched, clicked, typed, scrolled or
/// wheeled. Handler-safe (no world needed). Returns whether this report
/// ticked — `false` when throttled — so the caller schedules its flush
/// only when something changed.
pub fn report() -> bool {
    report_at(crate::time::now_micros())
}

/// [`report`] with an explicit clock reading (tests, and callers that
/// already hold one). `now == 0` means "no clock" and is never throttled.
pub fn report_at(now: u64) -> bool {
    if now != 0 {
        let throttled = LAST_TICK_AT
            .with(Cell::get)
            .is_some_and(|last| now.saturating_sub(last) < MIN_TICK_INTERVAL_MICROS);
        if throttled {
            return false;
        }
        LAST_TICK_AT.with(|l| l.set(Some(now)));
    }
    tick();
    true
}

/// Tick once, unthrottled: activity that was already throttled where it
/// happened. The runtime-server sidecar calls this for each
/// `AppToDev::UserActivity` the device relays — re-throttling on the
/// sidecar's clock could drop a relayed tick that arrived within a frame
/// of the previous one over a jittery connection.
pub fn report_relayed() {
    tick();
}

fn tick() {
    let n = COUNT.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    // Snapshot outside the borrow: a listener may add or drop another.
    let listeners: Vec<Handler> = LISTENERS.with(|l| l.borrow().iter().map(|(_, h)| h.clone()).collect());
    for h in listeners {
        h(n);
    }
}

/// Run `f` with the new tick count on every tick until the guard drops.
/// Runs inside the backend's platform callback, outside `World::enter`:
/// writes to signals stage and commit on the backend's flush.
pub fn add_listener(f: impl Fn(u64) + 'static) -> UserActivityListener {
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    LISTENERS.with(|l| l.borrow_mut().push((id, Rc::new(f))));
    UserActivityListener { id }
}

/// Keeps an [`add_listener`] callback registered; dropping it removes it.
#[must_use = "dropping the guard removes the listener immediately"]
pub struct UserActivityListener {
    id: u64,
}

impl Drop for UserActivityListener {
    fn drop(&mut self) {
        let id = self.id;
        // `try_with`: a guard dropped during thread teardown must not panic
        // on an already-destroyed thread-local.
        let _ = LISTENERS.try_with(|l| l.borrow_mut().retain(|(i, _)| *i != id));
    }
}

/// Zero the counter and throttle and drop every listener. Test isolation.
#[doc(hidden)]
pub fn reset_for_tests() {
    COUNT.with(|c| c.set(0));
    LAST_TICK_AT.with(|l| l.set(None));
    LISTENERS.with(|l| l.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_counts_and_notifies_listeners() {
        reset_for_tests();
        let seen = Rc::new(Cell::new(0));
        let s = seen.clone();
        let guard = add_listener(move |n| s.set(n));
        assert!(report_at(0));
        assert!(report_at(0));
        assert_eq!((count(), seen.get()), (2, 2));
        drop(guard);
        report_at(0);
        assert_eq!((count(), seen.get()), (3, 2), "dropped guard stops notifications");
    }

    /// A scroll burst (one report per event, faster than a frame) must
    /// tick once per frame, not once per event.
    #[test]
    fn reports_inside_one_frame_tick_once() {
        reset_for_tests();
        assert!(report_at(1_000));
        assert!(!report_at(1_000 + MIN_TICK_INTERVAL_MICROS - 1), "same frame is throttled");
        assert_eq!(count(), 1);
        assert!(report_at(1_000 + MIN_TICK_INTERVAL_MICROS), "next frame ticks");
        assert_eq!(count(), 2);
    }

    /// No time source installed reads `0`; the throttle must not swallow
    /// every report after the first.
    #[test]
    fn without_a_clock_every_report_ticks() {
        reset_for_tests();
        for _ in 0..5 {
            assert!(report_at(0));
        }
        assert_eq!(count(), 5);
    }

    /// Relayed ticks were throttled at the source; the receiver must not
    /// drop one that lands inside its own throttle window.
    #[test]
    fn relayed_reports_are_never_throttled() {
        reset_for_tests();
        assert!(report_at(1_000));
        report_relayed();
        report_relayed();
        assert_eq!(count(), 3);
    }

    /// A listener that drops its own guard (or adds another) while being
    /// notified must not hit a RefCell double borrow.
    #[test]
    fn listener_may_drop_its_guard_reentrantly() {
        reset_for_tests();
        let slot: Rc<RefCell<Option<UserActivityListener>>> = Rc::new(RefCell::new(None));
        let s = slot.clone();
        let calls = Rc::new(Cell::new(0));
        let c = calls.clone();
        *slot.borrow_mut() = Some(add_listener(move |_| {
            c.set(c.get() + 1);
            s.borrow_mut().take();
        }));
        report_at(0);
        report_at(0);
        assert_eq!(calls.get(), 1, "the listener removed itself after the first tick");
    }
}
