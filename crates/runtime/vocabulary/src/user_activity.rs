//! Author surface for app-wide user activity: [`user_activity`] (a
//! reactive tick counter) and [`add_user_activity_listener`] (a guard, for
//! code outside a reactive scope).
//!
//! The substrate is `runtime_shared::user_activity` — see its module docs
//! for the backend contract (one passive window-level observer per
//! backend, never consuming an event, throttled to one tick per frame).
//!
//! ## Per-world signal
//!
//! Same shape as `app_input`'s host-fact signals: each world owns a
//! `Signal<u64>` created through [`unscoped`] (world-root-owned, not
//! adopted by whichever subtree first reads it), seeded from the shared
//! count and kept in step by a substrate listener. Backends report from
//! raw platform callbacks outside `World::enter`; the listener's write
//! stages on the captured handle and commits on the backend's flush.
//! Writes into a dead world are silent no-ops.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::user_activity::{self as substrate, UserActivityListener};
use runtime_world::{inject, provide, signal, unscoped, ReadSignal, Signal};

#[derive(Clone)]
struct ActivityCtx {
    count: Signal<u64>,
    /// Keeps the substrate listener feeding `count` alive exactly as long
    /// as the ctx (the world's context map and [`LAST_CTX`] hold clones).
    _feed: Rc<UserActivityListener>,
}

thread_local! {
    /// Handler fallback (same rationale as `viewport::LAST_CTX`): author
    /// code reading the signal from an event handler runs outside `enter`.
    static LAST_CTX: RefCell<Option<ActivityCtx>> = const { RefCell::new(None) };
}

fn ctx() -> ActivityCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
    }
    if let Some(ctx) = inject::<ActivityCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let count = unscoped(|| signal(substrate::count()));
    let feed = substrate::add_listener(move |n| count.set(n));
    let ctx = ActivityCtx { count, _feed: Rc::new(feed) };
    unscoped(|| provide(ctx.clone()));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

/// Ticks every time the person uses the app — a pointer or touch down, a
/// key press, a wheel or scroll, typing in a text field — in any window or
/// portal (modals and sheets included), at most once per frame. The value
/// is a count of ticks; read it to subscribe, and restart an idle clock
/// whenever it changes:
///
/// ```ignore
/// let activity = user_activity();
/// effect!({
///     activity.get();          // re-runs on every tick
///     restart_idle_timer();
/// });
/// ```
///
/// Observing only: nothing about an event changes because this exists.
pub fn user_activity() -> ReadSignal<u64> {
    ctx().count.read_only()
}

/// Run `f` on every [`user_activity`] tick until the returned guard drops
/// — for code that keeps no reactive scope (a session-timeout service
/// owned by a struct). `f` runs inside the platform's event callback; a
/// signal it sets commits on the backend's flush.
pub fn add_user_activity_listener(f: impl Fn() + 'static) -> UserActivityListener {
    substrate::add_listener(move |_| f())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{effect, World};
    use std::cell::Cell;

    /// The platform callback fires outside `enter`; an author effect
    /// reading `user_activity()` must re-fire on the flush.
    #[test]
    fn report_from_platform_callback_refires_author_effect() {
        std::thread::spawn(|| {
            substrate::reset_for_tests();
            let world = World::new();
            let runs = world.enter(|| {
                let activity = user_activity();
                let runs = Rc::new(Cell::new(0usize));
                let r = runs.clone();
                let _e = effect(move || {
                    activity.get();
                    r.set(r.get() + 1);
                });
                runs
            });
            world.flush();
            assert_eq!(runs.get(), 1);

            assert!(substrate::report_at(0));
            world.flush();
            assert_eq!(runs.get(), 2, "a tick re-runs subscribers");

            // Two ticks before one flush coalesce into one re-run.
            substrate::report_at(0);
            substrate::report_at(0);
            world.flush();
            assert_eq!(runs.get(), 3);
            assert_eq!(world.enter(|| user_activity().get()), 3);
        })
        .join()
        .expect("activity re-fires subscribers");
    }

    /// Activity before any world read `user_activity()` seeds the signal
    /// instead of being lost.
    #[test]
    fn ctx_created_after_activity_seeds_from_the_count() {
        std::thread::spawn(|| {
            substrate::reset_for_tests();
            substrate::report_at(0);
            substrate::report_at(0);
            let world = World::new();
            assert_eq!(world.enter(|| user_activity().get()), 2);
        })
        .join()
        .expect("late ctx seeds from the substrate count");
    }

    /// Reads from a handler (outside `enter`) resolve to the mounted
    /// world's ctx instead of panicking.
    #[test]
    fn read_outside_enter_falls_back_to_last_ambient() {
        std::thread::spawn(|| {
            substrate::reset_for_tests();
            let world = World::new();
            world.enter(user_activity);
            substrate::report_at(0);
            world.flush();
            assert_eq!(user_activity().peek(), 1);
        })
        .join()
        .expect("handler-side read falls back to the last ambient ctx");
    }

    #[test]
    fn listener_guard_runs_until_dropped() {
        std::thread::spawn(|| {
            substrate::reset_for_tests();
            let calls = Rc::new(Cell::new(0));
            let c = calls.clone();
            let guard = add_user_activity_listener(move || c.set(c.get() + 1));
            substrate::report_at(0);
            drop(guard);
            substrate::report_at(0);
            assert_eq!(calls.get(), 1);
        })
        .join()
        .expect("listener guard");
    }
}
