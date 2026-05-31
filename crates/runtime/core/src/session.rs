//! Per-session-thread persistent state.
//!
//! The runtime-server sidecar runs one session per OS thread. A
//! `SessionMsg::Rerender` (hot-patch landed) drops the
//! reactive `Owner` and re-runs `mount(app)`, which means
//! every `signal!`, `animated!`, `effect!`, and `node_ref!`
//! in the user's tree gets fresh storage — and any
//! time-/state-driven animation in progress visibly snaps
//! back to its initial value.
//!
//! This module provides a thread-local typed registry that
//! lives *outside* the reactive `Owner`'s scope graph. Entries
//! are inserted on first call, retrieved on subsequent calls,
//! and survive any number of rerenders. The registry is freed
//! only when the session thread exits.
//!
//! # Composability
//!
//! This is the storage substrate for three layers of hot-patch
//! state retention:
//!
//! - **Layer 1 (this commit):** `epoch_micros()` — a session-
//!   wide time anchor. Author code that captures
//!   `runtime_core::session::epoch_micros()` once and reads
//!   wall-clock `time::now_micros()` per frame gets a
//!   continuous `elapsed` across rerenders, so time-driven
//!   animation phases don't restart.
//!
//! - **Layer 2 (follow-up):** `keyed_signal!(key, init)` /
//!   `keyed_animated!(key, init)` etc. — author-supplied keys.
//!   Reactive primitives stored in this registry survive
//!   rerenders, including in-flight spring/tween state.
//!
//! - **Layer 3 (eventual):** call-site-keyed hooks via macro-
//!   injected `concat!(file!(), line!(), column!())` keys. No
//!   author-visible key parameter; "Rules of Hooks" call-order
//!   discipline applies.
//!
//! All three layers share `get_or_init` for storage. The API
//! surface differs (named accessor for L1, key-arg macros for
//! L2, transparent macros for L3) but the underlying mechanism
//! is the same map.
//!
//! # What this is NOT
//!
//! - Not a replacement for `signal!` / `animated!`. Author code
//!   that doesn't opt in keeps the current behavior (fresh state
//!   on each rerender). This module is purely additive.
//! - Not a process-global store. Each session thread has its
//!   own registry. Cross-session state isn't shared (which is
//!   correct: each session is a logically-separate browser tab).
//! - Not persistent across sidecar respawns. A respawn ends the
//!   session thread, so the registry goes with it. Author code
//!   that wants cross-respawn state would need a different
//!   mechanism (file, IPC, etc.).

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Per-session-thread registry STACK. `&'static str` keys keep
    /// lookup allocation-free; values are heap-boxed `Any` so each
    /// call site can stash a different type.
    ///
    /// Index 0 is the always-present **root scope** — anything
    /// outside an explicit [`push_scope`] reads/writes there.
    /// Hosts that wrap an embedded app (the iOS wgpu host, the web
    /// wgpu host) push a fresh scope at mount and pop it on drop;
    /// the embedded app's `keyed(…)` calls land in that nested
    /// scope and disappear when the host tears down. Hot-patch
    /// rerenders happen inside the same scope and so survive.
    ///
    /// All session methods read/write the TOP scope only.
    static REGISTRIES: RefCell<Vec<HashMap<&'static str, Box<dyn Any>>>> =
        RefCell::new(vec![HashMap::new()]);
}

/// Push a fresh scope onto the per-thread registry stack. All
/// subsequent session reads/writes land in the new scope until
/// the returned guard drops (which pops it). The previous scope's
/// entries are inaccessible while the new scope is on top.
///
/// Hosts that own an embedded-app lifetime use this to scope the
/// embedded app's session-keyed state to the host's lifetime: the
/// iOS wgpu host pushes a scope inside `mount(…)` and stores the
/// guard in the returned `IosHostHandle`. When the handle drops
/// (e.g. `MountPolicy::LazyDisposing` releases the host screen),
/// the guard drops, the scope pops, the embedded app's session
/// AVs and epoch are gone, and the next mount starts from a fresh
/// `keyed(…, default)` baseline.
///
/// The guard is `!Send + !Sync`. It must be dropped on the same
/// thread that pushed it (the only one where its stack slot lives).
#[must_use = "drop the guard to pop the scope"]
pub struct ScopeGuard {
    _not_send: std::marker::PhantomData<*const ()>,
}

pub fn push_scope() -> ScopeGuard {
    REGISTRIES.with(|r| r.borrow_mut().push(HashMap::new()));
    ScopeGuard { _not_send: std::marker::PhantomData }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        REGISTRIES.with(|r| {
            let mut stack = r.borrow_mut();
            // Defensive: never pop the always-present root scope.
            // If something went very wrong (e.g. a guard from a
            // different thread reached here, or guards dropped out
            // of order) we'd rather no-op than corrupt the stack.
            if stack.len() > 1 {
                stack.pop();
            }
        });
    }
}

/// Read-modify-write helper for the top scope. All other session
/// APIs go through this so the top-only invariant lives in one place.
fn with_top<R>(f: impl FnOnce(&mut HashMap<&'static str, Box<dyn Any>>) -> R) -> R {
    REGISTRIES.with(|r| {
        let mut stack = r.borrow_mut();
        let top = stack
            .last_mut()
            .expect("session::REGISTRIES root scope always present");
        f(top)
    })
}

/// Return the value stored under `key`, or initialise it from
/// `init` and store it. Subsequent calls with the same key
/// return the same value (cloned out) — `init` is invoked at
/// most once per session-thread per key.
///
/// `T` must be `Clone` because we return a value, not a
/// reference (returning a reference would extend the borrow
/// across the closure that the caller likely wants to do
/// reactive work in). For `Rc`-backed reactive primitives
/// (`Signal`, `AnimatedValue`, …) clone is cheap.
///
/// **Reentrancy safe.** `init` runs outside the registry's
/// borrow so it can recursively call `get_or_init` for other
/// keys without panicking on `BorrowMutError`. This matters
/// for Layer 2/3 where init closures may construct other
/// reactive primitives.
///
/// **Type-mismatch tolerant.** If the stored entry's concrete
/// type doesn't match `T` (e.g. author changed the type of a
/// keyed signal between hot-patches), the stale entry is
/// overwritten with a fresh `init()`. The previous value is
/// dropped. This is the only way to recover; preserving the
/// old typed value would be a memory leak that author code
/// can't observe.
/// Drop every entry in the current session-thread's registry.
///
/// Intended for hosts that mount-then-unmount an embedded app
/// (e.g. `render_wgpu::Host::unmount` for a navigator-hidden
/// preview): the embedded app's `session::animated(...)` AVs
/// otherwise outlive the unmount — they're keyed in the global
/// REGISTRY by `&'static str`, so dropping the reactive scope
/// drops the scope's clones but leaves the registry's clone
/// holding the AV's `Inner`, which keeps its `TickRegistration`
/// live and so the animation clock keeps ticking it forever.
/// Calling this on unmount drops the registry's clone, which (if
/// it was the last clone) drops the `Inner` and unregisters the
/// tick.
///
/// **Scope is the entire thread.** This wipes every keyed entry,
/// not just the embedded app's. In practice `session::animated`
/// is rarely used outside embedded apps (welcome-style demos);
/// outer apps tend to drive their reactive state through plain
/// `Signal` / `Effect` / non-session AVs. If a use case ever
/// needs partitioned registries (one per Host), that becomes a
/// real refactor; `clear()` is the pragmatic interim.
pub fn clear() {
    with_top(|top| top.clear());
}

/// Drop the cached session epoch so the next [`epoch_micros`]
/// call re-initialises from `time::now_micros()`. Used together
/// with [`clear_prefix`] when a host wants the next mount of an
/// embedded app to behave like a brand-new session — the welcome
/// demo's session-relative `timeline!` acts replay from time=0
/// and the `raf_loop_scoped` body's `elapsed_us = now - epoch`
/// starts at zero again.
///
/// Pair `clear_prefix("…")` (wipes the app's AVs) with this
/// (wipes the session clock) for a complete embedded-app reset.
/// Without this call, a `clear_prefix` alone would leave the
/// epoch frozen at its original install time — the welcome's
/// `session::after_ms(glare_start, …)` collapses to delay=0 and
/// the raf body's elapsed jumps straight to the middle of the
/// orbit, defeating the visible-reset.
pub fn reset_epoch() {
    with_top(|top| {
        top.remove("__epoch_us");
    });
}

/// Clear every keyed entry whose key starts with `prefix`. Lets a
/// nested embedded app (the welcome demo running inside a
/// `Simulator` chassis on the website) wipe just its own AVs
/// without touching the outer app's session-keyed state.
///
/// Pair with the `MountPolicy::LazyDisposing` navigator path: the
/// outer scope tears down on blur, and on the next fresh mount the
/// embedded app's `use_*` constructor calls `clear_prefix("…")`
/// so its `keyed(…, default)` calls return fresh AVs at default
/// values — the welcome's act timeline replays from time=0, the
/// sun/planet `raf_loop` starts from the new session epoch, and
/// the demo truly resets instead of resuming mid-orbit.
///
/// Also clears the internal `__epoch_us` if its key matches the
/// prefix — embedded apps that pass their own prefix won't normally
/// hit that, but the API doesn't special-case `__`-prefixed keys.
pub fn clear_prefix(prefix: &str) {
    with_top(|top| {
        top.retain(|key, _| !key.starts_with(prefix))
    });
}

pub fn get_or_init<T: 'static + Clone>(
    key: &'static str,
    init: impl FnOnce() -> T,
) -> T {
    // Fast path: present and type matches. Short read borrow on
    // the top scope. Going through `with_top` here would borrow
    // mut; do a manual immutable borrow path instead.
    let existing: Option<T> = REGISTRIES.with(|r| {
        let stack = r.borrow();
        let top = stack
            .last()
            .expect("session::REGISTRIES root scope always present");
        top.get(key).and_then(|b| b.downcast_ref::<T>()).cloned()
    });
    if let Some(v) = existing {
        return v;
    }
    // Slow path: build outside any borrow, then insert under a
    // short write borrow. The two borrows are separate so `init`
    // can call back in.
    let v = init();
    with_top(|top| {
        top.insert(key, Box::new(v.clone()));
    });
    v
}

/// Microseconds reference captured the first time this
/// function is called on the current session thread. Survives
/// hot-patch rerenders for the lifetime of the session.
///
/// Use case: author code captures this once and compares to
/// `time::now_micros()` per frame to compute a *continuous*
/// elapsed-since-anchor that doesn't reset when a hot-patch
/// reruns the capturing function:
///
/// ```ignore
/// // Before (resets every rerender):
/// let epoch = runtime_core::time::now_micros();
///
/// // After (anchored to session start; survives rerenders):
/// let epoch = runtime_core::session::epoch_micros();
/// ```
///
/// The anchor is *first call*, not session start — author code
/// chooses when to anchor. For an animation that should "start
/// from session boot" this means calling it early; for one
/// that should "start from when the user does X" call it at X.
///
/// The internal key is `__epoch_us` (double-underscore prefix
/// reserves it from author-supplied keys at the Layer 2 API).
pub fn epoch_micros() -> u64 {
    get_or_init("__epoch_us", crate::time::now_micros)
}

// ---------------------------------------------------------------
// Typed reactive-primitive accessors (Layer 2)
// ---------------------------------------------------------------
//
// These wrap `get_or_init` for the three reactive primitives
// welcome-class apps reach for most: `AnimatedValue`, `Signal`,
// and one-shot init blocks. Each call returns the SAME instance
// (cloned via cheap `Rc` clone) every time it runs on the same
// session thread — so the value held by the primitive persists
// across hot-patch rerenders.
//
// Why this helps: after `SessionMsg::Rerender`, the old reactive
// `Owner` drops + the new `mount(app)` runs. With plain
// `animated!(0.0_f32)`, that builds a fresh `AnimatedValue` whose
// value is 0.0 again — every animation snaps back to its initial
// state. With `session::animated("welcome_opacity", 0.0_f32)`,
// the new run retrieves the existing AV with its current value
// intact (e.g. 1.0 if it had already faded in), and any re-run of
// the timeline's `TweenTo::new(1.0, ...)` factory sees current ==
// target and produces a no-op tween. The view continues to
// display the value it was already showing.
//
// Subscriptions added by repeated `.bind(...)` calls leak per
// rerender — old subscriptions hold dangling `Ref`s whose
// arena slots were freed when the old scope dropped, so they
// no-op on fire but still cost a closure call. For dev-mode
// rerender frequencies (~1/edit) this is sub-microsecond noise.
// A future cleanup pass can dedupe; not the bottleneck.

/// Retrieve or initialise a session-persistent `AnimatedValue<T>`
/// at `key`. Equivalent to `animated!(initial)` except the
/// returned instance survives `SessionMsg::Rerender` so the AV's
/// current value (and any in-flight animator state) carries
/// across hot-patches.
///
/// Use this for AVs that drive *visible* state you want to keep
/// stable across saves: animated colors, positions, opacities.
/// AVs that are scratch-pads for intermediate math (recomputed
/// per-frame from other inputs) don't need it.
pub fn animated<T>(key: &'static str, initial: T) -> crate::animation::AnimatedValue<T>
where
    T: crate::animation::Animatable + 'static,
{
    get_or_init(key, || crate::animation::AnimatedValue::new(initial))
}

/// Retrieve or initialise a session-persistent `Signal<T>` at
/// `key`. Mirrors [`animated`] for reactive scalars. Useful for
/// view-state signals (selected tab, scroll position, form
/// values, etc.) where re-rendering a fresh `signal!(default)`
/// would visibly reset author state on every save.
pub fn signal<T: 'static + Clone>(key: &'static str, initial: T) -> crate::reactive::Signal<T> {
    get_or_init(key, || crate::reactive::Signal::new(initial))
}

/// Schedule `body` to run at `at_session_ms` milliseconds after
/// the session's [`epoch_micros`] anchor — NOT after this call.
///
/// On the first mount (session epoch ≈ "now"), this behaves
/// identically to [`crate::after_ms_scoped`]: schedule with a
/// `delay = at_session_ms` and fire after that delay elapses.
///
/// After a hot-patch rerender, the session epoch is unchanged
/// (preserved by [`epoch_micros`]) but `now` has advanced. The
/// elapsed-since-epoch can already be past `at_session_ms`, in
/// which case `delay` clamps to 0 and `body` fires on the next
/// scheduler drive — *immediately* from the user's perspective.
/// This is the missing piece for "the act timeline shouldn't
/// replay on every save": already-elapsed timeline events fire
/// at once, the corresponding tweens compute `current == target`
/// (because the AVs persisted via [`animated`]), and the visible
/// result is no animation — the scene is already in its
/// post-this-act state.
///
/// Anchored to the current reactive scope via the underlying
/// [`crate::after_ms_scoped`], so scope cleanup cancels any
/// pending firing.
pub fn after_ms(at_session_ms: u64, body: impl FnOnce() + 'static) {
    let elapsed_us = crate::time::now_micros().saturating_sub(epoch_micros());
    let elapsed_ms = elapsed_us / 1000;
    let delay_ms = at_session_ms.saturating_sub(elapsed_ms);
    // Clamp to i32 for the underlying scheduling API. Values
    // beyond ~24 days saturate harmlessly to i32::MAX.
    let delay_ms_i32 = delay_ms.min(i32::MAX as u64) as i32;
    crate::scheduling::after_ms_scoped(delay_ms_i32, body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_init_returns_same_value_on_repeat() {
        let a = get_or_init("test_repeat", || 42_u64);
        let b = get_or_init("test_repeat", || 99_u64);
        assert_eq!(a, 42);
        assert_eq!(b, 42, "init should run only once per key");
    }

    #[test]
    fn get_or_init_independent_keys() {
        let a = get_or_init("test_key_a", || 1_u64);
        let b = get_or_init("test_key_b", || 2_u64);
        assert_eq!(a, 1);
        assert_eq!(b, 2);
    }

    #[test]
    fn get_or_init_type_mismatch_reinitialises() {
        let _ = get_or_init("test_typeswap", || 7_u64);
        // Now ask for a string under the same key. Old u64 gets
        // replaced; we get a fresh String.
        let s = get_or_init("test_typeswap", || "hello".to_string());
        assert_eq!(s, "hello");
    }

    #[test]
    fn get_or_init_reentrancy_safe() {
        // The init closure for `outer` calls back into
        // get_or_init for `inner`. Pre-fix this would panic
        // with BorrowMutError because we'd be re-entering the
        // registry's borrow.
        let outer = get_or_init("test_reentry_outer", || {
            let inner = get_or_init("test_reentry_inner", || 100_u64);
            inner * 2
        });
        assert_eq!(outer, 200);
        assert_eq!(get_or_init("test_reentry_inner", || 0_u64), 100);
    }

    #[test]
    fn epoch_micros_is_stable_within_thread() {
        let first = epoch_micros();
        // Sleep doesn't matter — subsequent calls return the
        // *first* call's reading regardless of wall clock.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let second = epoch_micros();
        assert_eq!(first, second);
    }

    #[test]
    fn different_threads_have_independent_registries() {
        // Set a marker on this thread.
        let main_val = get_or_init("test_thread_isolation", || 1_u64);
        assert_eq!(main_val, 1);
        // A spawned thread sees its own registry — init runs
        // fresh, returns 2 not 1.
        let other = std::thread::spawn(|| {
            get_or_init("test_thread_isolation", || 2_u64)
        })
        .join()
        .unwrap();
        assert_eq!(other, 2);
        // This thread's value unchanged.
        assert_eq!(get_or_init("test_thread_isolation", || 99_u64), 1);
    }

    // -----------------------------------------------------------------
    // Scope-stack regression tests
    //
    // The scope-stack semantics are how `MountPolicy::LazyDisposing`
    // navigators reset an embedded wgpu host's session-keyed state
    // (welcome AVs, `__epoch_us`) on remount while preserving them
    // across hot-patch rerenders. The tests cover the invariants the
    // host-lifetime ownership relies on:
    //
    //   * a pushed scope masks root-scope entries (reads/writes hit
    //     the top only)
    //   * dropping the guard pops the scope AND restores root visibility
    //   * `clear` / `clear_prefix` / `reset_epoch` only affect the top
    //     scope (so embedded-app cleanup doesn't wipe outer-app state)
    //   * a guard that gets leaked still leaves the stack in a sane
    //     state because the root scope is non-poppable
    // -----------------------------------------------------------------

    #[test]
    fn push_scope_masks_root_scope_entries() {
        let _root = get_or_init("scope_test_masking", || 11_u64);
        let _guard = push_scope();
        // Top scope is empty — init runs fresh.
        let inside = get_or_init("scope_test_masking", || 22_u64);
        assert_eq!(inside, 22);
        // Writes from inside the scope land in the top, not the root.
        let inside_again = get_or_init("scope_test_masking", || 33_u64);
        assert_eq!(inside_again, 22);
        drop(_guard);
        // Back at the root, the original value is intact.
        let after = get_or_init("scope_test_masking", || 44_u64);
        assert_eq!(after, 11);
    }

    #[test]
    fn scope_guard_drop_pops_back_to_root() {
        let baseline = get_or_init("scope_test_pop", || 1_u64);
        assert_eq!(baseline, 1);
        {
            let _guard = push_scope();
            let _ = get_or_init("scope_test_pop", || 999_u64);
        }
        // After the guard dropped at end of inner block, root
        // entry is reachable again.
        let after = get_or_init("scope_test_pop", || 2_u64);
        assert_eq!(after, 1);
    }

    #[test]
    fn nested_scopes_isolate_per_level() {
        let _outer_guard = push_scope();
        let _ = get_or_init("scope_test_nest", || 100_u64);
        {
            let _inner_guard = push_scope();
            // Fresh scope, init runs.
            let inner = get_or_init("scope_test_nest", || 200_u64);
            assert_eq!(inner, 200);
        }
        // Back to outer scope, its value is preserved.
        let outer = get_or_init("scope_test_nest", || 300_u64);
        assert_eq!(outer, 100);
    }

    #[test]
    fn clear_only_affects_top_scope() {
        let root_keep = get_or_init("scope_test_clear_root", || 7_u64);
        assert_eq!(root_keep, 7);
        {
            let _guard = push_scope();
            let _ = get_or_init("scope_test_clear_inner", || 77_u64);
            clear(); // wipes the inner scope, not the root.
            let inner_after = get_or_init("scope_test_clear_inner", || 88_u64);
            assert_eq!(inner_after, 88, "inner key should be wiped");
        }
        // Root scope untouched.
        assert_eq!(
            get_or_init("scope_test_clear_root", || 0_u64),
            7,
            "root scope must survive an inner clear()",
        );
    }

    #[test]
    fn clear_prefix_only_affects_top_scope() {
        let _ = get_or_init("scope_test_prefix_root_a", || 1_u64);
        let _ = get_or_init("scope_test_prefix_root_b", || 2_u64);
        {
            let _guard = push_scope();
            let _ = get_or_init("scope_test_prefix_root_a", || 10_u64);
            clear_prefix("scope_test_prefix_root_");
            // Top scope: both keys gone, init runs again.
            assert_eq!(
                get_or_init("scope_test_prefix_root_a", || 11_u64),
                11,
            );
        }
        // Root scope: both keys intact.
        assert_eq!(get_or_init("scope_test_prefix_root_a", || 0_u64), 1);
        assert_eq!(get_or_init("scope_test_prefix_root_b", || 0_u64), 2);
    }

    #[test]
    fn reset_epoch_only_affects_top_scope() {
        let root_epoch = epoch_micros();
        {
            let _guard = push_scope();
            // Fresh epoch inside the scope.
            let inner_epoch = epoch_micros();
            // Inner epoch is captured at THIS call (not the root).
            // Hard to assert exact equality cross-platform without
            // racing the clock, but the contract is: distinct from
            // root because root_epoch was captured ~10us ago and
            // would round-trip identically only if the registries
            // were shared.
            std::thread::sleep(std::time::Duration::from_millis(2));
            reset_epoch();
            let inner_after = epoch_micros();
            assert!(
                inner_after >= inner_epoch,
                "after reset_epoch, next epoch_micros re-captures wall clock",
            );
        }
        // Root epoch untouched even after inner reset_epoch.
        assert_eq!(epoch_micros(), root_epoch);
    }

    #[test]
    fn root_scope_is_never_popped() {
        // Constructing a guard and dropping it many times must
        // not eat into the always-present root scope.
        for _ in 0..5 {
            let g = push_scope();
            drop(g);
        }
        // Root scope still functions.
        let v = get_or_init("scope_test_root_indestructible", || 42_u64);
        assert_eq!(v, 42);
    }
}
