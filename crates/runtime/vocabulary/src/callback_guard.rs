//! Scope-guarding for author callbacks handed across the backend seam.
//!
//! # The problem this closes
//!
//! A backend is handed author callbacks (`on_press`, `on_activate`,
//! `on_change`, `on_scroll`, …) as plain `Rc<dyn Fn…>` with **no lifetime
//! contract attached**. Those closures capture signals owned by the
//! mounting scope. The backend stores them on native objects whose
//! lifetime it does not control, and native toolkits happily invoke them
//! after that scope is gone:
//!
//! - GTK emits `focus-leave` *while* a focused widget is being unparented
//!   — i.e. during the very teardown that frees the scope.
//! - A callback deferred to a run-loop source (GLib idle, `dispatch_async`,
//!   `Handler.post`) fires after a route change has dropped the screen.
//! - A gesture recognizer or observer outlives the view it was attached to
//!   by however long the platform keeps it alive.
//!
//! The callback then writes a freed signal slot, and `runtime_world`
//! raises `idealyst[stale-signal-handle]`. That panic is correct — the
//! write really is a bug — but it is raised on a stack the framework does
//! not own. On GTK it originates inside a GObject signal trampoline; on
//! Apple inside an objc callback. Both are `extern "C"` and **cannot
//! unwind**, so the process ABORTS rather than reporting a panic:
//!
//! ```text
//! panicked: signal used through a stale handle (world 1, slot 139)
//! ...gtk4::EventControllerFocus::connect_leave::leave_trampoline
//! panic in a function that cannot unwind
//! ```
//!
//! # Why the fix belongs here and not in each backend
//!
//! Every backend that can invoke a callback after teardown has this exact
//! shape, so N backends would each need their own bookkeeping, each with
//! its own coverage gaps (CLAUDE.md §7 — fix the cause once, not the
//! symptom per platform). And a backend cannot fix it properly anyway: it
//! has no way to ask "is the scope that produced this closure still
//! alive?". The framework does. So the framework hands over callbacks
//! that are already inert past their scope, and backends need no lifetime
//! discipline at all.
//!
//! # What "guarded" means
//!
//! Exactly: **after the mounting scope drops, calling the callback does
//! nothing**. It is not a fix for a callback that should have been
//! detached — a backend should still release native observers on teardown
//! where it can, because a guarded callback still costs a call. It is a
//! guarantee that failing to do so degrades to a no-op instead of killing
//! the process.
//!
//! Silently dropping the call is the correct semantic, not a papering-over:
//! the node is unmounted, so there is no longer any UI for the callback to
//! affect. The write it wanted to make has no observable consequence left.
//!
//! # Cost
//!
//! One [`ScopeAlive`] per mounting node that actually has callbacks —
//! plain `view`/`text` nodes allocate nothing. Acquiring it registers one
//! [`on_owned_drop`], a dependency-free keepalive effect owned by the
//! node's ownership scope. Share a single token across all of one node's
//! callbacks (see [`ScopeAlive::current`]).
//!
//! Note the anchor is `on_owned_drop` and deliberately NOT `on_scope_drop`:
//! the latter degrades to `on_cleanup` inside a running effect, which also
//! fires before that effect's next re-run, and a mount very often runs
//! inside a structural driver that re-runs while its nodes survive. See
//! [`ScopeAlive::current`] for the keyed-list case that forced this.

use std::cell::Cell;
use std::rc::Rc;

use runtime_world::on_owned_drop;

/// Liveness token for an ownership scope: `true` until that scope's
/// `Owned` drops, `false` forever after.
///
/// Cloning shares the flag — one token per node, wrapped around all of
/// that node's callbacks.
#[derive(Clone)]
pub struct ScopeAlive(Rc<Cell<bool>>);

impl ScopeAlive {
    /// A token for the innermost ownership scope — the mount handler, the
    /// component body, or the `collect_owned` region currently running.
    ///
    /// Outside any world the token is permanently `true`: nothing owns the
    /// scope, so there is no moment at which it could flip, and a
    /// backend-less unit test that calls a callback directly must still
    /// see it fire.
    ///
    /// Anchored with [`on_owned_drop`], NOT `on_scope_drop`. The latter
    /// defers to `on_cleanup` whenever an effect happens to be running, and
    /// `on_cleanup` also fires before that effect's next re-run — but a
    /// mount frequently runs inside a structural driver's effect that
    /// re-runs while the node it mounted survives. A keyed list is the
    /// clear case: its driver re-runs on every list edit, and keyed
    /// reconcile preserves the surviving rows. Under `on_scope_drop` the
    /// first unrelated edit flipped every live row's token to `false` and
    /// silently made its buttons inert. The flag must track the node's own
    /// ownership scope, which is exactly what `on_owned_drop` anchors to.
    ///
    /// Regression: `callbacks_survive_a_keyed_reconcile`.
    pub fn current() -> Self {
        let flag = Rc::new(Cell::new(true));
        let for_drop = flag.clone();
        on_owned_drop(move || for_drop.set(false));
        Self(flag)
    }

    /// A token that never flips — for callbacks with no scope to tie to.
    pub fn immortal() -> Self {
        Self(Rc::new(Cell::new(true)))
    }

    /// Is the scope still alive?
    pub fn get(&self) -> bool {
        self.0.get()
    }

    /// Wrap a nullary callback.
    pub fn wrap0(&self, f: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
        let alive = self.0.clone();
        Rc::new(move || {
            if alive.get() {
                f();
            }
        })
    }

    /// Wrap a one-argument callback.
    pub fn wrap1<A: 'static>(&self, f: Rc<dyn Fn(A)>) -> Rc<dyn Fn(A)> {
        let alive = self.0.clone();
        Rc::new(move |a| {
            if alive.get() {
                f(a);
            }
        })
    }

    /// Wrap a two-argument callback (`on_scroll`).
    pub fn wrap2<A: 'static, B: 'static>(&self, f: Rc<dyn Fn(A, B)>) -> Rc<dyn Fn(A, B)> {
        let alive = self.0.clone();
        Rc::new(move |a, b| {
            if alive.get() {
                f(a, b);
            }
        })
    }

    /// Wrap a callback that RETURNS a value the backend acts on (touch and
    /// file-drop handlers return a `TouchResponse`). A dead scope yields
    /// `R::default()`, which for `TouchResponse` is the
    /// don't-claim-the-gesture answer — the right thing to tell a toolkit
    /// about a node that no longer exists.
    pub fn wrap1_ret<A, R>(&self, f: Rc<dyn Fn(A) -> R>) -> Rc<dyn Fn(A) -> R>
    where
        A: 'static,
        R: Default + 'static,
    {
        let alive = self.0.clone();
        Rc::new(move |a| if alive.get() { f(a) } else { R::default() })
    }

    /// Wrap an `Option`al callback, leaving `None` as `None` — a backend
    /// distinguishes "no handler" from "handler that does nothing"
    /// (`AppEnvOps::url_opener`, `on_scroll`), so wrapping a `None` into a
    /// live no-op would change behavior.
    pub fn wrap0_opt(&self, f: Option<Rc<dyn Fn()>>) -> Option<Rc<dyn Fn()>> {
        f.map(|f| self.wrap0(f))
    }

    /// `Option` form of [`wrap2`](Self::wrap2).
    pub fn wrap2_opt<A: 'static, B: 'static>(
        &self,
        f: Option<Rc<dyn Fn(A, B)>>,
    ) -> Option<Rc<dyn Fn(A, B)>> {
        f.map(|f| self.wrap2(f))
    }

    // -- Concrete handler types -------------------------------------------
    //
    // These take a BY-REFERENCE argument, so they can't go through the
    // generic `wrap1_ret` (the HRTB does not fall out of `A: 'static`).
    // Each names the value a dead scope answers with, and each of those is
    // the behavior the framework already documents for "no handler at all"
    // — so an unmounted node replies exactly as a node that never
    // subscribed would.

    /// `on_key_down`. Dead scope → [`KeyOutcome::Default`](runtime_shared::primitives::key::KeyOutcome::Default): let the
    /// platform's own behavior run, which is what happens with no handler.
    pub fn wrap_key(
        &self,
        f: runtime_shared::primitives::key::KeyDownHandler,
    ) -> runtime_shared::primitives::key::KeyDownHandler {
        use runtime_shared::primitives::key::KeyOutcome;
        let alive = self.0.clone();
        Rc::new(move |ev| if alive.get() { f(ev) } else { KeyOutcome::Default })
    }

    /// `on_blur`. Dead scope → [`BlurOutcome::Allow`](runtime_shared::primitives::text_input::BlurOutcome::Allow): never veto a blur on
    /// behalf of a node that no longer exists, or focus would be trapped.
    pub fn wrap_blur(
        &self,
        f: runtime_shared::primitives::text_input::BlurHandler,
    ) -> runtime_shared::primitives::text_input::BlurHandler {
        use runtime_shared::primitives::text_input::BlurOutcome;
        let alive = self.0.clone();
        Rc::new(move || if alive.get() { f() } else { BlurOutcome::Allow })
    }

    /// `on_touch`. Dead scope → `TouchResponse::default()` (neither
    /// consumed nor claimed), so the event bubbles to a live ancestor
    /// instead of being swallowed by a node that is gone.
    pub fn wrap_touch(&self, f: runtime_shared::TouchHandler) -> runtime_shared::TouchHandler {
        let alive = self.0.clone();
        Rc::new(move |ev| {
            if alive.get() {
                f(ev)
            } else {
                runtime_shared::TouchResponse::default()
            }
        })
    }

    /// File-drop handler. Same reasoning as [`wrap_touch`](Self::wrap_touch).
    pub fn wrap_file_drop(
        &self,
        f: runtime_shared::FileDropHandler,
    ) -> runtime_shared::FileDropHandler {
        let alive = self.0.clone();
        Rc::new(move |ev| {
            if alive.get() {
                f(ev)
            } else {
                runtime_shared::TouchResponse::default()
            }
        })
    }

    /// `on_hover` — plain notification, nothing to answer with.
    pub fn wrap_hover(&self, f: runtime_shared::HoverHandler) -> runtime_shared::HoverHandler {
        self.wrap1(f)
    }

    /// Wheel / scroll-wheel handler.
    pub fn wrap_wheel(&self, f: runtime_shared::WheelHandler) -> runtime_shared::WheelHandler {
        let alive = self.0.clone();
        Rc::new(move |ev| {
            if alive.get() {
                f(ev)
            } else {
                runtime_shared::TouchResponse::default()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn outside_a_world_the_token_stays_alive() {
        // `on_scope_drop` is inert outside a world, so nothing can flip the
        // flag. A callback must still fire — otherwise every unit test that
        // invokes one directly would silently no-op.
        let alive = ScopeAlive::current();
        assert!(alive.get());
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        let f = alive.wrap0(Rc::new(move || h.set(h.get() + 1)));
        f();
        f();
        assert_eq!(hits.get(), 2);
    }

    #[test]
    fn a_dead_scope_makes_every_arity_inert() {
        let alive = ScopeAlive::immortal();
        let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));

        let l = log.clone();
        let f0 = alive.wrap0(Rc::new(move || l.borrow_mut().push("f0")));
        let l = log.clone();
        let f1 = alive.wrap1(Rc::new(move |_: i32| l.borrow_mut().push("f1")));
        let l = log.clone();
        let f2 = alive.wrap2(Rc::new(move |_: f32, _: f32| l.borrow_mut().push("f2")));
        let l = log.clone();
        let fr = alive.wrap1_ret(Rc::new(move |_: i32| {
            l.borrow_mut().push("fr");
            7u8
        }));

        f0();
        f1(1);
        f2(0.0, 0.0);
        assert_eq!(fr(1), 7, "a live scope returns the callback's real value");
        assert_eq!(*log.borrow(), ["f0", "f1", "f2", "fr"]);

        // Kill the scope: every arity must go quiet, and the returning one
        // must fall back to `Default` rather than calling through.
        alive.0.set(false);
        log.borrow_mut().clear();
        f0();
        f1(1);
        f2(0.0, 0.0);
        assert_eq!(fr(1), 0, "a dead scope returns Default, not the callback's value");
        assert!(
            log.borrow().is_empty(),
            "no callback may run after its scope dropped — this is what turns a \
             stale-signal ABORT (raised inside a non-unwinding C trampoline) into \
             a no-op",
        );
    }

    #[test]
    fn none_stays_none() {
        // A backend distinguishes "no handler" from "handler that does
        // nothing"; wrapping `None` into a live no-op would change behavior.
        let alive = ScopeAlive::immortal();
        assert!(alive.wrap0_opt(None).is_none());
        assert!(alive
            .wrap2_opt(None::<Rc<dyn Fn(f32, f32)>>)
            .is_none());
        assert!(alive.wrap0_opt(Some(Rc::new(|| {}))).is_some());
    }

    #[test]
    fn clones_share_one_flag() {
        // One token per node, wrapped around all of that node's callbacks —
        // so a single scope drop silences the whole node.
        let alive = ScopeAlive::immortal();
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        let f = alive.clone().wrap0(Rc::new(move || h.set(h.get() + 1)));
        f();
        alive.0.set(false);
        f();
        assert_eq!(hits.get(), 1, "the clone must observe the original's flip");
    }
}
