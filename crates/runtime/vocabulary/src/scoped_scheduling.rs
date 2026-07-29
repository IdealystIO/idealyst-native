//! New-core scope-anchored scheduling — the glue shadows for the old
//! core's `after_ms_scoped` / `raf_loop_scoped` / `session::after_ms`.
//!
//! # Why the old fns can't be re-exported (the "welcome never animates"
//! bug)
//!
//! The old-core scoped variants anchor their cancellation through
//! old-core `reactive::on_cleanup`, which OUTSIDE any old-core scope
//! silently drops its callback immediately — and a new-core build never
//! has an old-core scope active. Dropping the callback drops the
//! captured `ScheduledTask` / `RafLoop` handle, whose `Drop` cancels
//! the timer/loop, so on the new core the old `after_ms_scoped` /
//! `raf_loop_scoped` were **inert**: the welcome app's `timeline!` acts
//! never fired and its raf-driven sun/planet pulse never started when
//! the embedded simulator first mounted on a new-core graph. (Same
//! root cause as glue's `AnimatedValue::bind` shadow — see the module
//! comment above `glue::animation`.)
//!
//! # Anchoring model
//!
//! An [`Anchor`] owns the schedule handles registered under it and a
//! `dead` flag every deferred callback checks before running (the
//! browser can dispatch a timer already queued for the current tick
//! even after cancellation — the old core's teardown-safety flag,
//! reproduced here). Where the anchor's `kill` comes from mirrors the
//! old capture rules:
//!
//! - **Inside an effect run** (`runtime_world::in_effect()`): killed via
//!   `runtime_world::on_cleanup` — the effect's next re-run or its
//!   owner's drop cancels every pending dispatch, exactly the old
//!   "timers die when the surrounding `effect!` re-runs / the Owner
//!   drops" contract.
//! - **Inside a build** (`runtime_world::is_entered()`, no effect
//!   running): killed by a dependency-free keepalive effect owned by
//!   the ambient collector (`component_scope` / the boot's
//!   `collect_owned`) — dies when the component subtree unrealizes,
//!   the same idiom as `glue::animation::scope_keepalive`.
//! - **Inside a deferred body registered here** (a nested
//!   `raf_loop_scoped` inside a `session::after_ms` callback — the
//!   welcome app's exact shape): the firing callback re-pushes its
//!   registering anchor onto a thread-local stack, so nested `*_scoped`
//!   calls attach to the SAME lifetime — the old contract's "the
//!   deferred callback re-enters the registering scope when it fires".
//! - **Outside all of the above**: no-op, mirroring the old "outside
//!   any reactive scope the scoped variants are inert" posture. Use
//!   `after_ms_detached` / raw `raf_loop` for detached work.
//!
//! No `cycle()` wrap and no busy-skip (old-core arena concerns): on the
//! new kernel, writes staged by a deferred body commit via the host
//! scheduler's post-dispatch flush hook, and staging is re-entrancy
//! safe by construction.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::scheduling::{after_ms, raf_loop};

thread_local! {
    /// Stack of anchors whose deferred bodies are currently running —
    /// the "re-enter the registering scope" channel for nested
    /// `*_scoped` calls.
    static DEFERRED: RefCell<Vec<Anchor>> = const { RefCell::new(Vec::new()) };
}

/// One cancellation domain: schedule handles + the shared dead flag.
/// Clones share the same domain.
#[derive(Clone)]
struct Anchor(Rc<AnchorInner>);

struct AnchorInner {
    dead: Cell<bool>,
    /// Owned `ScheduledTask` / `RafLoop` handles — dropping them is the
    /// cancellation. Holding them here creates an intentional
    /// `Rc`-cycle with the callbacks' anchor clones; `kill` breaks it.
    owned: RefCell<Vec<Box<dyn Any>>>,
}

impl Anchor {
    fn new() -> Anchor {
        Anchor(Rc::new(AnchorInner {
            dead: Cell::new(false),
            owned: RefCell::new(Vec::new()),
        }))
    }

    fn kill(&self) {
        self.0.dead.set(true);
        // Take first, drop outside the borrow: a handle's Drop may
        // re-enter scheduling machinery.
        let owned = std::mem::take(&mut *self.0.owned.borrow_mut());
        drop(owned);
    }

    fn is_dead(&self) -> bool {
        self.0.dead.get()
    }

    fn own(&self, handle: impl Any) {
        if self.is_dead() {
            // Anchor already killed (e.g. a straggler fire scheduling a
            // sibling): dropping the handle cancels it immediately.
            return;
        }
        self.0.owned.borrow_mut().push(Box::new(handle));
    }
}

/// Kill-on-drop guard — the payload handed to whichever owner
/// (effect cleanup / keepalive effect) anchors the domain's lifetime.
struct KillOnDrop(Anchor);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.kill();
    }
}

/// Resolve the anchor for a `*_scoped` registration at THIS call site,
/// per the module-doc capture rules. `None` = "outside any scope" —
/// the caller must be inert (old-core parity).
fn current_anchor() -> Option<Anchor> {
    if let Some(top) = DEFERRED.with(|d| d.borrow().last().cloned()) {
        return Some(top);
    }
    if runtime_world::in_effect() {
        let anchor = Anchor::new();
        let guard = KillOnDrop(anchor.clone());
        runtime_world::on_cleanup(move || drop(guard));
        return Some(anchor);
    }
    if runtime_world::is_entered() {
        let anchor = Anchor::new();
        let guard = KillOnDrop(anchor.clone());
        // Dependency-free effect: runs once, never re-fires, owns the
        // guard for the collector's lifetime (component subtree /
        // boot `collect_owned` — world root as last resort).
        let _ = runtime_world::effect(move || {
            let _ = &guard;
        });
        return Some(anchor);
    }
    None
}

/// Run `f` with `anchor` pushed as the ambient deferred anchor, so
/// nested `*_scoped` registrations inside the body attach to the same
/// lifetime. Pop is panic-safe.
fn with_deferred_anchor(anchor: &Anchor, f: impl FnOnce()) {
    DEFERRED.with(|d| d.borrow_mut().push(anchor.clone()));
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            let _ = DEFERRED.try_with(|d| {
                d.borrow_mut().pop();
            });
        }
    }
    let _pop = PopGuard;
    f();
}

/// New-core mirror of `runtime_core::scheduling::after_ms_scoped` — a
/// one-shot timer that dies with the registering scope (see the module
/// docs for the exact anchor rules).
pub fn after_ms_scoped<F: FnOnce() + 'static>(delay_ms: i32, f: F) {
    let Some(anchor) = current_anchor() else {
        // Outside any scope: inert, mirroring the old core's documented
        // "the captured task is dropped immediately" posture.
        return;
    };
    let anchor_cb = anchor.clone();
    let task = after_ms(delay_ms, move || {
        if anchor_cb.is_dead() {
            return;
        }
        with_deferred_anchor(&anchor_cb, f);
    });
    anchor.own(task);
}

/// New-core mirror of `runtime_core::scheduling::raf_loop_scoped` — a
/// recurring animation-frame loop that dies with the registering scope.
pub fn raf_loop_scoped<F: FnMut() + 'static>(mut f: F) {
    let Some(anchor) = current_anchor() else {
        return;
    };
    let anchor_cb = anchor.clone();
    let handle = raf_loop(move || {
        if anchor_cb.is_dead() {
            return;
        }
        f();
    });
    anchor.own(handle);
}

/// New-core mirror of `runtime_core::session::after_ms` — schedule
/// relative to the session epoch (already-elapsed offsets fire on the
/// next tick), anchored like [`after_ms_scoped`]. The epoch/registry
/// machinery is the shared old-core `session` module (pure
/// thread-local state, core-agnostic); only the scope anchoring
/// diverges — which is the entire reason this shadow exists.
pub fn session_after_ms(at_session_ms: u64, body: impl FnOnce() + 'static) {
    let elapsed_us =
        runtime_core::time::now_micros().saturating_sub(runtime_core::session::epoch_micros());
    let elapsed_ms = elapsed_us / 1000;
    let delay_ms = at_session_ms.saturating_sub(elapsed_ms);
    let delay_ms_i32 = delay_ms.min(i32::MAX as u64) as i32;
    after_ms_scoped(delay_ms_i32, body);
}
