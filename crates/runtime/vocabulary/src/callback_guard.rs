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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_world::{current_effect, effect_depth, in_collector, on_owned_drop, Effect};

thread_local! {
    /// Stack of tokens whose guarded callbacks are currently running,
    /// each paired with the [`effect_depth`] observed when it was pushed.
    /// [`ScopeAlive::current`] consults it so a token acquired *during* a
    /// callback inherits that callback's lifetime instead of silently
    /// anchoring to nothing; the depth is what lets it tell an effect
    /// running INSIDE that callback from the effect the callback is itself
    /// running inside. Innermost last.
    static ACTIVE: RefCell<Vec<(Anchor, usize)>> = const { RefCell::new(Vec::new()) };
}

/// What a [`ScopeAlive`] token is actually tied to.
///
/// Two shapes, because the two lifetimes the framework can anchor to are
/// reported differently: an ownership scope flips a flag when its `Owned`
/// drops, while a running effect answers a liveness query about its own
/// slot. Both are cheap to clone (an `Rc` bump / a `Copy` triple).
#[derive(Clone)]
enum Anchor {
    /// Flipped by an [`on_owned_drop`] keepalive owned by the ownership
    /// scope — or never flipped at all, for [`ScopeAlive::immortal`] and
    /// for a token acquired outside any world.
    Flag(Rc<Cell<bool>>),
    /// The effect whose body was running when the token was taken. Dies
    /// with the effect's OWNER (its slot is freed then), not on the
    /// effect's re-runs — see [`ScopeAlive::current`].
    Effect(Effect),
}

impl Anchor {
    fn alive(&self) -> bool {
        match self {
            Anchor::Flag(f) => f.get(),
            Anchor::Effect(e) => e.is_alive(),
        }
    }
}

/// Pops the ACTIVE entry on drop, so an early return or a panic inside a
/// guarded callback cannot leave a stale token on the stack.
struct ActiveGuard;

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|s| {
            s.borrow_mut().pop();
        });
    }
}

fn push_active(anchor: &Anchor) -> ActiveGuard {
    let depth = effect_depth();
    ACTIVE.with(|s| s.borrow_mut().push((anchor.clone(), depth)));
    ActiveGuard
}

/// Liveness token for an ownership scope: `true` until that scope's
/// `Owned` drops, `false` forever after.
///
/// Cloning shares the anchor — one token per node, wrapped around all of
/// that node's callbacks.
#[derive(Clone)]
pub struct ScopeAlive(Anchor);

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
    ///
    /// # The rungs, and why they are in this order
    ///
    /// Three places can supply a lifetime, and the rule is **the innermost
    /// dynamic scope wins** — the same rule at each rung, not three
    /// special cases:
    ///
    /// 1. **A build is in progress** (`in_collector`) — anchor to the
    ///    subtree being built. First, so a handler that realizes a subtree
    ///    (a navigator push) gives that subtree its own lifetime rather
    ///    than the button's, and so a subtree realized from inside a
    ///    driver effect outlives that driver's next re-run.
    /// 2. **An effect body is running, and it is inside any guarded
    ///    callback on the stack** — anchor to that effect, which dies with
    ///    the `Owned` that collected it.
    /// 3. **A guarded callback is running** — inherit its token.
    /// 4. Otherwise a fresh `on_owned_drop` anchor, which outside a world
    ///    is permanently live.
    ///
    /// Rung 2 is the one that was missing, and its absence was an app
    /// abort rather than a leak. An effect RE-RUN is not a build
    /// (`run_effect` pushes no collector) and, when the flush is driven by
    /// the host's post-dispatch hook, no guarded callback is on the stack
    /// either — so a `spawn_then` issued from the re-run fell through to
    /// rung 4, where `on_owned_drop` with no ambient collector registers a
    /// **world-root-owned** keepalive. That token never flips, so the
    /// callback ran into a disposed component and its first `Signal::set`
    /// aborted with `idealyst[stale-signal-handle]`. On web an ordinary
    /// "create a record, then open its detail screen" flow reaches it
    /// every time, because `StackRetention` disposes the covered screen
    /// there.
    ///
    /// The anchor is the running effect's own slot liveness
    /// ([`current_effect`]), NOT `on_cleanup`: `on_cleanup` fires before
    /// the effect's next RE-RUN as well as at teardown, so it would cancel
    /// an in-flight `spawn_then` every time its effect re-ran. The
    /// scheduling helpers (`scoped_scheduling::current_anchor`) do want
    /// that shorter lifetime for timers — a superseded timer should not
    /// fire — but a superseded fetch has already gone out, and dropping
    /// its result is a behaviour change, not a fix.
    ///
    /// The depth comparison at rung 2 is what keeps rungs 2 and 3 from
    /// fighting. A guarded callback can be invoked from inside an effect
    /// body (a framework driver calling an author closure), and there the
    /// callback is the inner scope and its token the more precise anchor;
    /// an effect running at a depth the callback never saw is the inner
    /// scope instead. Comparing [`effect_depth`] now against the depth
    /// recorded when the callback was pushed distinguishes the two exactly.
    ///
    /// Regression: `effect_rerun_spawned_task_dies_with_its_owner`,
    /// `effect_rerun_spawned_task_applies_while_its_owner_lives`.
    pub fn current() -> Self {
        // Rung 1 — a BUILD is in progress (mount walk, component body, any
        // `collect_owned` region): anchor to the subtree being built.
        if in_collector() {
            return Self::anchored();
        }
        // Rungs 2 and 3 — whichever of "the effect running now" and "the
        // guarded callback we are inside" is the INNER scope.
        let active = ACTIVE.with(|s| s.borrow().last().cloned());
        let depth_at_push = active.as_ref().map(|(_, d)| *d).unwrap_or(0);
        if effect_depth() > depth_at_push {
            if let Some(eff) = current_effect() {
                return Self(Anchor::Effect(eff));
            }
        }
        // No build and no inner effect, but we are running INSIDE a guarded
        // callback: inherit that callback's token. Without this, a token
        // acquired at handler time anchors to nothing — `on_owned_drop` is
        // inert outside a world and world-root-owned inside one — so it
        // stays `true` forever and guards nothing. That is the shape a
        // `spawn_then` called from an `on_press` has, which is the common
        // one.
        //
        // Regression: `handler_spawned_task_dies_with_its_node`.
        if let Some((anchor, _)) = active {
            return Self(anchor);
        }
        Self::anchored()
    }

    /// A fresh token tied to the innermost ownership scope, or permanently
    /// live when there is no scope to tie to.
    fn anchored() -> Self {
        let flag = Rc::new(Cell::new(true));
        let for_drop = flag.clone();
        on_owned_drop(move || for_drop.set(false));
        Self(Anchor::Flag(flag))
    }

    /// Run `f` with this token as the ambient one, so a nested
    /// [`current`](Self::current) inherits it. The seam `spawn_then` uses
    /// to keep a chained task tied to the same node as its parent.
    pub fn run_within<R>(&self, f: impl FnOnce() -> R) -> R {
        let _g = push_active(&self.0);
        f()
    }

    /// A token that never flips — for callbacks with no scope to tie to.
    pub fn immortal() -> Self {
        Self(Anchor::Flag(Rc::new(Cell::new(true))))
    }

    /// Flip a flag-anchored token dead by hand — test-only, standing in
    /// for the scope teardown that normally does it.
    #[cfg(test)]
    fn kill(&self) {
        match &self.0 {
            Anchor::Flag(f) => f.set(false),
            Anchor::Effect(_) => unreachable!("effect-anchored tokens die with their slot"),
        }
    }

    /// Is the scope still alive?
    pub fn get(&self) -> bool {
        self.0.alive()
    }

    /// Wrap a nullary callback.
    pub fn wrap0(&self, f: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
        let anchor = self.0.clone();
        Rc::new(move || {
            if anchor.alive() {
                let _g = push_active(&anchor);
                f();
            }
        })
    }

    /// Wrap a one-argument callback.
    pub fn wrap1<A: 'static>(&self, f: Rc<dyn Fn(A)>) -> Rc<dyn Fn(A)> {
        let anchor = self.0.clone();
        Rc::new(move |a| {
            if anchor.alive() {
                let _g = push_active(&anchor);
                f(a);
            }
        })
    }

    /// Wrap a two-argument callback (`on_scroll`).
    pub fn wrap2<A: 'static, B: 'static>(&self, f: Rc<dyn Fn(A, B)>) -> Rc<dyn Fn(A, B)> {
        let anchor = self.0.clone();
        Rc::new(move |a, b| {
            if anchor.alive() {
                let _g = push_active(&anchor);
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
        let anchor = self.0.clone();
        Rc::new(move |a| if anchor.alive() { let _g = push_active(&anchor); f(a) } else { R::default() })
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
        let anchor = self.0.clone();
        Rc::new(move |ev| if anchor.alive() { let _g = push_active(&anchor); f(ev) } else { KeyOutcome::Default })
    }

    /// `on_blur`. Dead scope → [`BlurOutcome::Allow`](runtime_shared::primitives::text_input::BlurOutcome::Allow): never veto a blur on
    /// behalf of a node that no longer exists, or focus would be trapped.
    pub fn wrap_blur(
        &self,
        f: runtime_shared::primitives::text_input::BlurHandler,
    ) -> runtime_shared::primitives::text_input::BlurHandler {
        use runtime_shared::primitives::text_input::BlurOutcome;
        let anchor = self.0.clone();
        Rc::new(move || if anchor.alive() { let _g = push_active(&anchor); f() } else { BlurOutcome::Allow })
    }

    /// `on_touch`. Dead scope → `TouchResponse::default()` (neither
    /// consumed nor claimed), so the event bubbles to a live ancestor
    /// instead of being swallowed by a node that is gone.
    pub fn wrap_touch(&self, f: runtime_shared::TouchHandler) -> runtime_shared::TouchHandler {
        let anchor = self.0.clone();
        Rc::new(move |ev| {
            if anchor.alive() {
                let _g = push_active(&anchor);
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
        let anchor = self.0.clone();
        Rc::new(move |ev| {
            if anchor.alive() {
                let _g = push_active(&anchor);
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
        let anchor = self.0.clone();
        Rc::new(move |ev| {
            if anchor.alive() {
                let _g = push_active(&anchor);
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
        alive.kill();
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

    // -- Anchor selection -------------------------------------------------
    //
    // These build a real world so the rung order in `current()` is
    // exercised against actual collector / effect / callback state rather
    // than a mock. Each pins one rung's OUTCOME (does the token flip, and
    // when), which is the only property the rest of the framework depends
    // on.

    use runtime_world::{collect_owned, effect, signal, World};

    /// Rung 2 — the bug this whole enum exists for.
    ///
    /// An effect RE-RUN is not a build (`run_effect` pushes no collector)
    /// and, driven from a plain flush, has no guarded callback on the
    /// stack either. Before rung 2 existed the re-run fell through to
    /// `anchored()`, whose `on_owned_drop` with no ambient collector
    /// registers a WORLD-ROOT-owned keepalive — a token that never flips.
    /// A `spawn_then` issued there guarded nothing, and its callback ran
    /// into a disposed component: `idealyst[stale-signal-handle]`.
    #[test]
    fn a_token_taken_during_an_effect_rerun_dies_with_the_effects_owner() {
        let w = World::new();
        let taken: Rc<RefCell<Vec<ScopeAlive>>> = Rc::new(RefCell::new(Vec::new()));
        let t = taken.clone();
        let (tick, owned) = w.enter(|| {
            collect_owned(|| {
                let tick = signal(0i32);
                effect(move || {
                    let _ = tick.get();
                    t.borrow_mut().push(ScopeAlive::current());
                });
                tick
            })
        });
        assert_eq!(taken.borrow().len(), 1, "the effect ran once at creation");

        w.enter(|| tick.set(1));
        w.flush();
        assert_eq!(taken.borrow().len(), 2, "and once more on the re-run");
        assert!(taken.borrow()[1].get(), "the re-run's token starts alive");

        // Tearing down the subtree frees the effect's slot.
        drop(owned);
        assert!(
            !taken.borrow()[1].get(),
            "a token taken during an effect RE-RUN must flip when the scope              that owns the effect is torn down — otherwise a `spawn_then`              issued from the re-run writes a freed slot and aborts",
        );
        assert!(!taken.borrow()[0].get(), "the first run's collector-anchored token too");
    }

    /// The over-kill guard. Rung 2 anchors to the effect's SLOT, which its
    /// owner frees — deliberately not to `on_cleanup`, which also fires
    /// before the effect's next re-run and would cancel an in-flight fetch
    /// every time its own effect re-ran.
    #[test]
    fn an_effect_rerun_token_survives_that_effect_rerunning_again() {
        let w = World::new();
        let taken: Rc<RefCell<Vec<ScopeAlive>>> = Rc::new(RefCell::new(Vec::new()));
        let t = taken.clone();
        let (tick, _owned) = w.enter(|| {
            collect_owned(|| {
                let tick = signal(0i32);
                effect(move || {
                    let _ = tick.get();
                    t.borrow_mut().push(ScopeAlive::current());
                });
                tick
            })
        });

        w.enter(|| tick.set(1));
        w.flush();
        w.enter(|| tick.set(2));
        w.flush();

        assert!(
            taken.borrow()[1].get(),
            "an in-flight task must survive its own effect re-running — the              IO has already gone out, and its node is still mounted",
        );
    }

    /// Rungs 2 vs 3. A guarded callback can be invoked from INSIDE an
    /// effect body (a framework driver calling an author closure); there
    /// the callback is the inner scope and its token the precise anchor,
    /// so the depth recorded at push time must keep rung 2 from claiming
    /// it.
    #[test]
    fn a_guarded_callback_running_inside_an_effect_keeps_its_own_token() {
        let w = World::new();
        let outer = ScopeAlive::immortal();
        let inner: Rc<RefCell<Option<ScopeAlive>>> = Rc::new(RefCell::new(None));
        let i = inner.clone();
        let o = outer.clone();
        let (tick, owned) = w.enter(|| {
            collect_owned(|| {
                let tick = signal(0i32);
                effect(move || {
                    // Only on a RE-RUN: the first run is a build, where
                    // rung 1 (the collector) correctly claims the token
                    // regardless of what is on the callback stack.
                    if tick.get() > 0 {
                        o.run_within(|| {
                            *i.borrow_mut() = Some(ScopeAlive::current());
                        });
                    }
                });
                tick
            })
        });
        w.enter(|| tick.set(1));
        w.flush();

        drop(owned); // the effect's owner goes; the callback's node did not
        assert!(
            inner.borrow().as_ref().expect("taken").get(),
            "the ambient callback token is the INNER scope here and must win              over the effect it happens to be running inside",
        );
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
        alive.kill();
        f();
        assert_eq!(hits.get(), 1, "the clone must observe the original's flip");
    }
}
