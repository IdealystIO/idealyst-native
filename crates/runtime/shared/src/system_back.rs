//! The platform's system Back — Android's back button and back gesture —
//! offered to app code before the platform acts on it.
//!
//! ## Shape
//!
//! Any number of **handlers** ([`add_handler`]) can be registered. When
//! the backend receives a system Back that nothing native consumed (no
//! stack navigator popped it), it calls [`dispatch`]: handlers run newest
//! first, and the first to return [`BackOutcome::Consume`] ends it. If
//! every handler returns [`BackOutcome::Default`] (or none is registered),
//! the platform does what it would have done without the app — on Android,
//! finish the Activity or move its task to the back.
//!
//! ## Where Back reaches the handlers
//!
//! Native navigators take Back first: a stack presenter pops its own
//! screens and enforces a screen's `back_enabled(false)` itself. A handler
//! sees the Back that would otherwise leave the app — at the root of every
//! navigator, or in an app with none. That is the hook a kiosk needs
//! ("Back at my root must not exit") and the one a "press Back again to
//! exit" prompt needs.
//!
//! ## Why the backend is told whether anyone listens
//!
//! [`install_back_host`] hands the backend a `wanted` flag that is `true`
//! only while a handler is registered. Android enables its
//! `OnBackPressedCallback` only then: an enabled callback disables the
//! system's predictive back-to-home animation, and an app that never asks
//! for Back must keep it.
//!
//! ## Platforms without a system Back
//!
//! iOS, macOS, the desktop backends and terminals have no system Back;
//! the web's history Back cannot be intercepted (the navigators' URL sync
//! follows it instead). Those backends never install a host or call
//! [`dispatch`], so handlers are registered and never run. Registering one
//! is always safe; it is how one author tree stays identical everywhere.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What a back handler did with a system Back.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BackOutcome {
    /// Handled: stop here. The platform does nothing (the app stays in
    /// front on Android).
    Consume,
    /// Not handled: offer it to the next (older) handler, and when none
    /// is left, let the platform do its default.
    Default,
}

/// The backend's half: told `true` while at least one handler is
/// registered, `false` when the last one goes. Returns whether it applied
/// the change; `false` means "busy, ask again" (the backend was mid-borrow)
/// and the dispatcher retries on the next microtask.
pub type BackHost = Rc<dyn Fn(bool) -> bool>;

type HandlerFn = Rc<dyn Fn() -> BackOutcome>;

struct Handler {
    id: u64,
    run: HandlerFn,
}

#[derive(Default)]
struct State {
    handlers: Vec<Handler>,
    next_id: u64,
    host: Option<BackHost>,
    /// What the host was last told and accepted.
    told: bool,
    retry_pending: bool,
}

runtime_tls::thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Re-entrancy guard for [`dispatch`]: a handler that itself triggers
    /// a Back dispatch (it shouldn't) is not offered the nested one.
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
}

/// RAII registration from [`add_handler`]; dropping it removes the
/// handler, and when it was the last one the backend stops asking for
/// Back.
#[must_use = "dropping a BackHandler removes it immediately"]
pub struct BackHandler {
    id: u64,
}

impl Drop for BackHandler {
    fn drop(&mut self) {
        let id = self.id;
        let _ = STATE.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.handlers.retain(|h| h.id != id);
            }
        });
        reconcile();
    }
}

impl std::fmt::Debug for BackHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackHandler").field("id", &self.id).finish()
    }
}

/// Register a system-Back handler until the guard drops. Newer handlers
/// run first.
pub fn add_handler(handler: impl Fn() -> BackOutcome + 'static) -> BackHandler {
    let id = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.handlers.push(Handler { id, run: Rc::new(handler) });
        id
    });
    reconcile();
    BackHandler { id }
}

/// Whether any handler is registered — what the backend's `wanted` flag
/// tracks.
pub fn wants_back() -> bool {
    STATE.with(|s| !s.borrow().handlers.is_empty())
}

/// Offer one system Back to the handlers, newest first. Returns `true`
/// when one consumed it; `false` means the backend performs the
/// platform's default.
///
/// Called by the backend from its raw platform callback, outside every
/// framework-wrapped handler: the backend schedules its flush afterwards
/// so writes a handler staged commit.
pub fn dispatch() -> bool {
    if DISPATCHING.with(|d| d.replace(true)) {
        return false;
    }
    // Snapshot outside the borrow: a handler may add or drop handlers
    // (a navigator popping unmounts the screen that registered one).
    let handlers: Vec<HandlerFn> =
        STATE.with(|s| s.borrow().handlers.iter().rev().map(|h| h.run.clone()).collect());
    let mut consumed = false;
    for run in handlers {
        if run() == BackOutcome::Consume {
            consumed = true;
            break;
        }
    }
    DISPATCHING.with(|d| d.set(false));
    consumed
}

/// Install (`Some`) or remove (`None`) the backend's host. Called by the
/// boot path (`runtime_vocabulary::backend::install_env_services`) with a
/// closure over the live backend, and with `None` when the backend goes.
/// A new host is told the current `wanted` state at once, so handlers
/// registered before boot are not lost.
pub fn install_back_host(host: Option<BackHost>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.host = host;
        // A new backend has been told nothing yet.
        s.told = false;
    });
    reconcile_forced();
}

/// Clear all state (handlers, host). Test isolation only.
#[doc(hidden)]
pub fn reset_for_tests() {
    let old = STATE.with(|s| std::mem::take(&mut *s.borrow_mut()));
    drop(old);
    DISPATCHING.with(|d| d.set(false));
}

fn reconcile() {
    reconcile_inner(false);
}

/// Tell a freshly installed host the current state even when it is
/// `false`: the host may have started with its callback in any state.
fn reconcile_forced() {
    reconcile_inner(true);
}

fn reconcile_inner(force: bool) {
    let plan = STATE.try_with(|s| {
        let s = s.try_borrow().ok()?;
        let want = !s.handlers.is_empty();
        if want == s.told && !force {
            return None;
        }
        Some((want, s.host.clone()?))
    });
    let Ok(Some((want, host))) = plan else {
        return;
    };
    // Call the host outside any STATE borrow: it re-enters the backend.
    let applied = host(want);
    STATE.with(|s| {
        if applied {
            s.borrow_mut().told = want;
        }
    });
    if !applied {
        schedule_retry();
    }
}

/// The backend was busy — try again once the current stack unwinds. One
/// retry in flight at a time; without a scheduler `schedule_microtask`
/// runs inline on native, and the pending flag stops that from recursing.
fn schedule_retry() {
    let first = STATE.with(|s| !std::mem::replace(&mut s.borrow_mut().retry_pending, true));
    if !first {
        return;
    }
    // Reconcile BEFORE clearing the flag: a still-busy host re-enters
    // `schedule_retry`, which must see the retry as pending (an inline
    // microtask would otherwise recurse).
    crate::scheduling::schedule_microtask(|| {
        reconcile_forced();
        STATE.with(|s| s.borrow_mut().retry_pending = false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> Rc<RefCell<Vec<bool>>> {
        reset_for_tests();
        let told = Rc::new(RefCell::new(Vec::new()));
        let t = told.clone();
        install_back_host(Some(Rc::new(move |wanted| {
            t.borrow_mut().push(wanted);
            true
        })));
        told
    }

    #[test]
    fn a_new_host_is_told_the_current_state() {
        reset_for_tests();
        let early = add_handler(|| BackOutcome::Consume);
        let told = Rc::new(RefCell::new(Vec::new()));
        let t = told.clone();
        install_back_host(Some(Rc::new(move |w| {
            t.borrow_mut().push(w);
            true
        })));
        assert_eq!(*told.borrow(), vec![true], "a handler registered before boot is not lost");
        drop(early);
        assert_eq!(*told.borrow(), vec![true, false]);
    }

    #[test]
    fn the_host_wants_back_only_while_a_handler_is_registered() {
        let told = wire();
        assert_eq!(*told.borrow(), vec![false], "install reports the initial state");
        let a = add_handler(|| BackOutcome::Default);
        let b = add_handler(|| BackOutcome::Default);
        assert_eq!(*told.borrow(), vec![false, true], "the second handler changes nothing");
        drop(a);
        assert!(wants_back());
        drop(b);
        assert!(!wants_back());
        assert_eq!(*told.borrow(), vec![false, true, false]);
    }

    #[test]
    fn newest_handler_runs_first_and_consume_stops_the_walk() {
        let _told = wire();
        let log = Rc::new(RefCell::new(Vec::new()));
        let (l1, l2) = (log.clone(), log.clone());
        let _old = add_handler(move || {
            l1.borrow_mut().push("old");
            BackOutcome::Consume
        });
        let new = add_handler(move || {
            l2.borrow_mut().push("new");
            BackOutcome::Default
        });
        assert!(dispatch(), "the older handler consumed it");
        assert_eq!(*log.borrow(), vec!["new", "old"]);
        drop(new);
        log.borrow_mut().clear();
        assert!(dispatch());
        assert_eq!(*log.borrow(), vec!["old"]);
    }

    #[test]
    fn nobody_consuming_leaves_the_platform_default() {
        let _told = wire();
        assert!(!dispatch(), "no handler: the platform's default");
        let _h = add_handler(|| BackOutcome::Default);
        assert!(!dispatch());
    }

    #[test]
    fn a_handler_may_drop_handlers_while_dispatching() {
        let _told = wire();
        let slot: Rc<RefCell<Option<BackHandler>>> = Rc::new(RefCell::new(None));
        let s = slot.clone();
        *slot.borrow_mut() = Some(add_handler(move || {
            // A navigator pop unmounting the screen that registered this.
            s.borrow_mut().take();
            BackOutcome::Consume
        }));
        assert!(dispatch());
        assert!(!wants_back(), "the handler removed itself");
        assert!(!dispatch());
    }

    #[test]
    fn a_busy_host_is_asked_again() {
        reset_for_tests();
        let busy = Rc::new(Cell::new(true));
        let told = Rc::new(RefCell::new(Vec::new()));
        let (b, t) = (busy.clone(), told.clone());
        install_back_host(Some(Rc::new(move |w| {
            if b.get() {
                return false;
            }
            t.borrow_mut().push(w);
            true
        })));
        // No scheduler is installed in unit tests, so the retry microtask
        // runs inline and finds the host still busy; it then waits for the
        // next change.
        let h = add_handler(|| BackOutcome::Consume);
        assert!(told.borrow().is_empty());
        busy.set(false);
        drop(h);
        let _h2 = add_handler(|| BackOutcome::Consume);
        assert_eq!(told.borrow().last(), Some(&true));
    }
}
