//! Single-threaded fine-grained reactivity.
//!
//! Implementation note: storage for signals and effects lives in a
//! thread-local arena. The handles you hold (`Signal<T>`, `EffectHandle`)
//! are small `Copy`-able tokens that index into the arena, rather than
//! `Rc<...>`-style owning references. This is what makes `Signal<T>: Copy`,
//! which eliminates the manual `.clone()` boilerplate at closure boundaries.
//!
//! ## Lifetime model
//!
//! - Slots in the arena are owned by a `Scope`. When the scope drops, its
//!   slots are freed.
//! - The renderer's `Owner` holds a `Scope`, so a UI tree's reactive state
//!   is freed when the owner drops.
//! - Reactive subtrees (e.g. inside `when()`) create nested scopes that
//!   drop independently when the subtree is replaced.
//!
//! ## Failure modes
//!
//! - Reading from a `Signal<T>` after its owning scope drops panics with a
//!   diagnostic message. There is no silent corruption.
//! - Subscriber sets are kept tight on the cleanup side: every dependency
//!   link is bidirectional, so `Effect`-drop and effect re-runs both remove
//!   the dead `EffectId` from every `Signal` it had read.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
// FxHashMap/FxHashSet are SipHash-free HashMap/HashSet aliases using rustc's
// FxHasher. Every collection here is keyed by an internal Signal/Effect integer
// id (never attacker-controlled), so the default SipHash is pure overhead on the
// framework's hottest path — see the crate's Cargo.toml note. Logic is identical;
// this is a hasher swap only.
use rustc_hash::{FxHashMap, FxHashSet};

// =============================================================================
// IDs and arena storage
// =============================================================================

/// Opaque index into the arena's signal slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct SignalId(u32);

/// Opaque index into the arena's effect slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct EffectId(u32);

/// Opaque index into the arena's ref slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct RefId(u32);

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
    static CURRENT: RefCell<Option<EffectId>> = const { RefCell::new(None) };
    /// Effects currently on the run-stack. When a signal write inside
    /// an effect's body fires the same effect's subscribers, we want
    /// to skip re-firing the effect that's already running — otherwise
    /// the inner re-run calls `clear_effect_dependencies` and wipes
    /// the dep set the outer run had just started recording, leaving
    /// the effect subscribed to nothing after the outer returns.
    ///
    /// Used by `run_effect` to short-circuit re-entrant calls for the
    /// same id. Different-id reentry (effect A's set fires effect B,
    /// which runs and reads other signals) is fine — only same-id
    /// reentry corrupts the dep set.
    static RUNNING: RefCell<FxHashSet<EffectId>> = RefCell::new(FxHashSet::default());

    /// Transitive depth of nested `run_effect` calls on the current
    /// thread. The same-id reentry guard (`RUNNING`) only catches the
    /// case where an effect's own write retriggers itself — it does not
    /// catch *mutual* loops where effect A writes a signal B's effect
    /// reads, B's effect writes a signal A's effect reads, and so on.
    /// Without a bound, that pattern stack-overflows the process.
    ///
    /// Threshold and panic live in `run_effect`. The counter is
    /// incremented on entry and decremented via the `DepthGuard` RAII
    /// so unwinding through a user-code panic still restores it.
    static EFFECT_DEPTH: RefCell<u32> = const { RefCell::new(0) };

    /// When `Some`, signal writes record their *dirtied signal* in this
    /// window instead of fanning out to subscribers inline. The fan-out
    /// (and, for `set_if_changed`, the change-detection decision) is
    /// deferred until the outermost `batch(..)` call returns. `None`
    /// outside any batch — writes fan out synchronously as before.
    ///
    /// We track dirty *signals* rather than pre-collected subscriber
    /// `EffectId`s so that net-zero windows can be elided: a signal set
    /// to `B` then back to `A` within one batch nets to no change, and
    /// its subscribers are never woken (see `DirtyWindow` /
    /// `Signal::set_if_changed`). Collecting subscribers eagerly per
    /// write — the previous model — made that net comparison impossible
    /// because by flush time only a flattened effect list remained.
    ///
    /// Nested `batch(..)` calls reuse the outer window: only the
    /// outermost batch flushes. This keeps "set a, then set b" inside a
    /// nested batch from running effects between the two writes when
    /// the outer batch hasn't completed yet.
    static BATCH_PENDING: RefCell<Option<DirtyWindow>> = const { RefCell::new(None) };

    /// Nesting depth of in-progress `memo` compute closures. Incremented
    /// before invoking the user's `f()` in `memo_with` and decremented
    /// on return. `Signal::set` and `Signal::update` consult it to
    /// reject writes from inside a memo's compute — memos are
    /// contractually pure derivations, and a write would (a) inject a
    /// side-effecting node into the dep graph and (b) re-trigger
    /// downstream subscribers during what should be a pure read.
    static MEMO_COMPUTE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };

    /// Backend-installable deferred-drop policy. When set, `Scope::drop`
    /// hands its drained effect boxes (and per-scope guards) to this
    /// function instead of dropping them synchronously. Backends that
    /// need to amortize teardown across frames — the web backend uses
    /// this to slice wasm-bindgen `Closure` drops over `requestAnimation
    /// Frame` so the cost doesn't land inside the apply window — install
    /// a policy at boot. Native backends leave it `None` and drops fall
    /// through to a synchronous `drop(boxes)`.
    ///
    /// The signature is a bare `fn` (not `Box<dyn Fn>`) because the
    /// policy is install-once and queue-state is the backend's job to
    /// store (typically another backend-local thread-local). This keeps
    /// the runtime-core slot zero-sized.
    static DROP_DEFERRAL: std::cell::Cell<Option<fn(Vec<Box<dyn Any>>)>> =
        const { std::cell::Cell::new(None) };

    /// Re-entrancy depth of in-flight *mutating* reactive operations on
    /// this thread: a running effect body, or a `with_signal_mut`
    /// window (which TAKES a signal's box out of the arena, leaving its
    /// slot `None` for the duration). While nonzero, the reactive arena
    /// is in an intermediate state — a signal slot may be absent, an
    /// effect's dep recording may be half-done.
    ///
    /// A deferred callback (a scope-anchored `raf_loop`/`after_ms` whose
    /// browser frame the OS dispatched during this window) that touched
    /// a signal now would panic: either "signal used after its scope was
    /// dropped" (the taken slot reads `None`) or corrupt the in-flight
    /// effect's dep set. The scope-anchored scheduling helpers consult
    /// [`is_reactive_busy`] and skip the offending invocation, re-arming
    /// on the next frame instead. See `crates/runtime/core/src/scheduling.rs`.
    static REACTIVE_BUSY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };

    /// Backend-installed callback fired each time the OUTERMOST reactive
    /// mutation window closes (`REACTIVE_BUSY` → 0). A backend uses it to
    /// flush a coalesced layout pass synchronously, before the run loop
    /// paints — so views inserted by a reactive update (any trigger: event,
    /// timer, async, hot-reload) are positioned before they're displayed,
    /// instead of flashing at their default (0,0) frame and snapping into
    /// place a frame later. `None` until a backend installs one.
    static ON_REACTIVE_IDLE: std::cell::RefCell<Option<std::rc::Rc<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };

    /// Re-entrancy guard for [`ON_REACTIVE_IDLE`]: set while the idle hook
    /// runs, so a signal mutation the hook itself performs (closing another
    /// reactive window at depth 0) doesn't recursively re-fire it.
    static IN_REACTIVE_IDLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Register a callback to run when the outermost reactive mutation window
/// closes. The macOS backend installs one that flushes its pending layout
/// pass synchronously (see `backend_macos`), turning the deferred
/// "insert → paint-at-(0,0) → next-turn-reposition" flicker into
/// "insert → layout → paint" within the same turn. Idempotent: a later call
/// replaces the hook. Backends that apply frames synchronously in `finish`
/// (web) don't need it.
pub fn install_reactive_idle_hook(f: std::rc::Rc<dyn Fn()>) {
    ON_REACTIVE_IDLE.with(|h| *h.borrow_mut() = Some(f));
}

/// `true` when a mutating reactive operation (an effect body or a
/// `with_signal_mut` window) is in flight on this thread. Scope-anchored
/// scheduling callbacks read this to avoid re-entering the reactive arena
/// while it's mid-mutation — see the `REACTIVE_BUSY` thread-local doc and
/// the teardown-race regression in `scheduling_scoped.rs`.
pub fn is_reactive_busy() -> bool {
    REACTIVE_BUSY.with(|c| c.get()) > 0
}

/// RAII guard that bumps [`REACTIVE_BUSY`] for the lifetime of a mutating
/// reactive window. Drop runs on unwind too, so a panic inside the window
/// doesn't leave the counter stuck high.
struct ReactiveBusyGuard;

impl ReactiveBusyGuard {
    fn enter() -> Self {
        REACTIVE_BUSY.with(|c| c.set(c.get() + 1));
        ReactiveBusyGuard
    }
}

impl Drop for ReactiveBusyGuard {
    fn drop(&mut self) {
        let depth = REACTIVE_BUSY.with(|c| {
            let n = c.get().saturating_sub(1);
            c.set(n);
            n
        });
        // Outermost window just closed: the mutation is fully applied and no
        // arena borrow is held, so it's safe to run the backend's idle hook
        // (a synchronous layout flush) here, before control returns to the
        // run loop / paint. Guarded so a mutation the hook performs doesn't
        // recurse. Cheap when nothing changed — the hook itself no-ops if no
        // layout pass is pending.
        if depth == 0 && !IN_REACTIVE_IDLE.with(|f| f.replace(true)) {
            let hook = ON_REACTIVE_IDLE.with(|h| h.borrow().clone());
            if let Some(hook) = hook {
                hook();
            }
            IN_REACTIVE_IDLE.with(|f| f.set(false));
        }
    }
}

/// Install a backend-specific deferred-drop policy that `Scope::drop` will
/// route effect/guard teardown through. The policy is a `fn` so it doesn't
/// allocate; the backend owns its own queue + scheduler state in a sibling
/// thread-local.
///
/// Designed for the web backend's rAF-sliced drain — wasm-bindgen `Closure`
/// drops are expensive and pile up inside the apply window otherwise. Native
/// backends never call this; their `Scope::drop` runs synchronously, which
/// is the right choice when teardown is cheap.
///
/// Pre-refactor this whole machinery lived behind `#[cfg(target_arch =
/// "wasm32")]` in runtime-core, which violated the framework-purity rule
/// (no platform-specific implementations in `framework/`). The cfg-gated
/// storage and scheduler now lives in `backend-web`; this hook is the
/// portable seam.
pub fn install_drop_deferral(policy: fn(Vec<Box<dyn Any>>)) {
    DROP_DEFERRAL.with(|c| c.set(Some(policy)));
}

/// Hand a batch of drained boxes to the installed deferral policy if one
/// exists; otherwise drop them synchronously. Empty-vec calls are a no-op
/// (no thread-local touch in the common case).
fn defer_or_drop(boxes: Vec<Box<dyn Any>>) {
    if boxes.is_empty() {
        return;
    }
    if let Some(policy) = DROP_DEFERRAL.with(|c| c.get()) {
        policy(boxes);
    } else {
        drop(boxes);
    }
}

struct Arena {
    signals: Vec<Option<Box<dyn Any>>>,
    /// Generation counter per signal slot, parallel to `signals`.
    /// Bumped every time a slot is freed (`take_signals_batched`), so a
    /// recycled slot gets a fresh generation. A `Signal<T>` handle
    /// records the generation it was minted with; a read/write through
    /// a handle whose generation no longer matches the slot's is
    /// recognised as STALE (the original signal's scope unmounted and
    /// the slot was reused, possibly by a different-typed signal) and
    /// becomes a safe no-op instead of aliasing the new occupant —
    /// which previously panicked with "signal type mismatch" (a
    /// process-aborting crash across the JNI/FFI boundary) or, worse,
    /// silently fired the wrong signal's subscribers. The classic
    /// generational-arena guard (Leptos/Slotmap).
    signal_gen: Vec<u32>,
    effects: Vec<Option<Box<dyn Any>>>,
    /// Outer `Option`: `None` once the slot is freed by its owning scope.
    /// Inner `Option<Box<dyn Any>>`: `None` while the ref exists but hasn't
    /// been filled by a mount yet; `Some` once mounted.
    refs: Vec<Option<Option<Box<dyn Any>>>>,

    /// Per-signal subscriber set, indexed parallel to `signals`. Lives
    /// on the arena (not on `SignalInner<T>`) so cleanup code that
    /// removes a dead `EffectId` from each subscribed signal can touch
    /// the set without knowing the signal's concrete `T` — the price
    /// of a generic `SignalInner` is that mutating it from a non-
    /// generic site is fiddly.
    ///
    /// Maintained as the inverse of `effect_dependencies`: an
    /// `(eid, sid)` link exists in `signal_subscribers[sid]` iff it
    /// exists in `effect_dependencies[eid]`.
    signal_subscribers: Vec<FxHashSet<EffectId>>,

    /// Per-effect dependency set, indexed parallel to `effects`. An
    /// entry `sid` here means "this effect's last run read signal
    /// `sid`". Cleared at the start of every re-run so the dep set
    /// reflects the *latest* run, not the union of all runs (matches
    /// what every fine-grained reactivity lib does — Solid, Reactively,
    /// MobX). Drained on effect-free so dead `EffectId`s don't sit in
    /// any signal's subscriber set.
    effect_dependencies: Vec<FxHashSet<SignalId>>,

    /// Per-signal JS notifier callbacks. At most one notifier per
    /// signal. Fires AFTER the Rust subscriber fan-out on every
    /// `Signal::set` / `Signal::update`. The closure typically reads
    /// the signal's current value (via its captured `Signal<T>`
    /// handle), stringifies it, and ships the new value across the
    /// wasm→JS boundary so a JS-side reactive layer can update its
    /// subscribers.
    ///
    /// Keyed by `SignalId` raw u32 (wrapped in `u64` to match the
    /// public `Signal::id()` API surface). `HashMap` rather than a
    /// parallel `Vec` because most signals have no JS subscribers —
    /// a `Vec<Option<Rc<dyn Fn()>>>` would waste a slot per
    /// non-subscribed signal.
    ///
    /// Cleanup: removed in `take_signals_batched` when the signal's
    /// slot is freed, so the notifier (which typically holds a
    /// `Weak<RefCell<Backend>>`) doesn't outlive its signal.
    signal_js_notifiers: FxHashMap<u64, std::rc::Rc<dyn Fn()>>,

    /// Freelists for recycling nulled slot ids. Without these, the
    /// arena vectors grow monotonically with the number of slots
    /// *ever* created — a tight rebuild loop that mounts and
    /// un-mounts 10k effects per iteration would balloon `effects`
    /// to ~165k null slots after just three iterations of an arena
    /// suite, with parallel growth in `effect_dependencies` /
    /// `signal_subscribers` (each a `Vec<HashSet<_>>`). The cache
    /// locality penalty + per-push capacity reallocation cost shows
    /// up as build times tripling between suite runs.
    ///
    /// Recycling is safe because every effect-drop path
    /// (`free_effect`, `take_effects_batched`) tears down the
    /// reverse-index links *before* releasing the slot id, so by
    /// the time an id enters a freelist, no subscriber set holds it.
    /// Same for signals — `take_signals_batched` clears the
    /// subscriber set for the slot before releasing the id.
    signal_free: Vec<u32>,
    effect_free: Vec<u32>,
    ref_free: Vec<u32>,
}

impl Arena {
    fn new() -> Self {
        Self {
            signals: Vec::new(),
            signal_gen: Vec::new(),
            effects: Vec::new(),
            refs: Vec::new(),
            signal_subscribers: Vec::new(),
            effect_dependencies: Vec::new(),
            signal_js_notifiers: FxHashMap::default(),
            signal_free: Vec::new(),
            effect_free: Vec::new(),
            ref_free: Vec::new(),
        }
    }

    /// Returns the slot id AND the slot's current generation. The
    /// caller stamps the generation into the `Signal<T>` handle so a
    /// later read/write can detect a recycled slot (see `signal_gen`).
    fn insert_signal<T: 'static>(&mut self, inner: SignalInner<T>) -> (SignalId, u32) {
        if let Some(idx) = self.signal_free.pop() {
            // Recycle a previously-freed slot. The slot itself is
            // `None` and `signal_subscribers[idx]` is empty (cleared
            // by `take_signals_batched`), so we just stash the new
            // value. Its generation was already bumped at free time, so
            // any still-living handle to the old occupant won't match.
            self.signals[idx as usize] = Some(Box::new(inner));
            // Defensive: in case a stale entry made it past cleanup.
            self.signal_subscribers[idx as usize].clear();
            (SignalId(idx), self.signal_gen[idx as usize])
        } else {
            let id = SignalId(self.signals.len() as u32);
            self.signals.push(Some(Box::new(inner)));
            self.signal_subscribers.push(FxHashSet::default());
            self.signal_gen.push(0);
            (id, 0)
        }
    }

    fn insert_effect(&mut self, inner: EffectInner) -> EffectId {
        if let Some(idx) = self.effect_free.pop() {
            self.effects[idx as usize] = Some(Box::new(inner));
            // Defensive: see `insert_signal`.
            self.effect_dependencies[idx as usize].clear();
            EffectId(idx)
        } else {
            let id = EffectId(self.effects.len() as u32);
            self.effects.push(Some(Box::new(inner)));
            self.effect_dependencies.push(FxHashSet::default());
            id
        }
    }

    fn insert_ref(&mut self) -> RefId {
        if let Some(idx) = self.ref_free.pop() {
            self.refs[idx as usize] = Some(None);
            RefId(idx)
        } else {
            let id = RefId(self.refs.len() as u32);
            self.refs.push(Some(None));
            id
        }
    }

    fn take_ref(&mut self, id: RefId) -> Option<Option<Box<dyn Any>>> {
        let taken = self.refs.get_mut(id.0 as usize).and_then(|s| s.take());
        if taken.is_some() {
            self.ref_free.push(id.0);
        }
        taken
    }

    /// Remove `eid` from every signal it currently subscribes to and
    /// drop its dep set. Used by the `free_effect` (handle drop)
    /// path and by `run_effect` (clear deps before re-run) so the
    /// inverse map stays consistent. Scope::drop uses
    /// `take_effects_batched` instead — same operation, amortized
    /// across the whole scope.
    fn unsubscribe_effect(&mut self, eid: EffectId) {
        let Some(slot) = self.effect_dependencies.get_mut(eid.0 as usize) else { return; };
        let deps = std::mem::take(slot);
        for sid in deps {
            if let Some(subs) = self.signal_subscribers.get_mut(sid.0 as usize) {
                subs.remove(&eid);
            }
        }
    }

    /// Take the contents out of `effects[id]` for every id in `ids`,
    /// leaving each slot `None` and unsubscribing each effect from
    /// the signals it had read. Collapses what would be
    /// `O(scope_effects × deps)` individual `HashSet::remove` calls
    /// into one `retain` per *distinct* dependency signal — a single
    /// 10k-row branch typically only depends on a small handful of
    /// signals (the active theme), so this turns 10k removes into
    /// ~1 retain.
    ///
    /// Returns the taken `EffectInner` boxes in the order `ids`
    /// were passed, skipping any slot that was already empty. The
    /// caller drops the boxes *after* releasing the ARENA borrow —
    /// an `EffectInner`'s captures may transitively own nested
    /// `Scope`s whose own `Drop` re-enters ARENA, and dropping them
    /// inside our borrow would panic "RefCell already borrowed". See
    /// `Scope::drop` for the dance.
    fn take_effects_batched(&mut self, ids: &[EffectId]) -> Vec<Box<dyn Any>> {
        // 1) Drain each effect's dep set into a `dead` set, recording
        //    the union of signals affected.
        let mut dead: FxHashSet<EffectId> =
            FxHashSet::with_capacity_and_hasher(ids.len(), Default::default());
        let mut affected: FxHashSet<SignalId> = FxHashSet::default();
        for &eid in ids {
            if let Some(slot) = self.effect_dependencies.get_mut(eid.0 as usize) {
                let deps = std::mem::take(slot);
                affected.extend(deps);
            }
            dead.insert(eid);
        }
        // 2) For each affected signal, do one `retain` filtering out
        //    every dead `EffectId` at once. O(subscribers) per signal,
        //    O(1) per element via `HashSet::contains`.
        for sid in affected {
            if let Some(subs) = self.signal_subscribers.get_mut(sid.0 as usize) {
                subs.retain(|eid| !dead.contains(eid));
            }
        }
        // 3) Null the slots, recycle the ids onto the freelist, and
        //    return the taken boxes.
        let mut out = Vec::with_capacity(ids.len());
        for &eid in ids {
            if let Some(slot) = self.effects.get_mut(eid.0 as usize) {
                if let Some(boxed) = slot.take() {
                    out.push(boxed);
                    self.effect_free.push(eid.0);
                }
            }
        }
        out
    }

    /// Batched version of `take_signal` for `Scope::drop`. Same shape
    /// as `take_effects_batched` but for signals: clears every
    /// subscriber set we own in one pass, then takes the slot
    /// contents. Subscribers' dep sets aren't touched — the next time
    /// each effect re-runs, `run_effect` clears its deps, so the
    /// stale `sid` is naturally evicted; if the effect never runs
    /// again (it's also being dropped), its slot will get the same
    /// treatment from `take_effects_batched`.
    fn take_signals_batched(&mut self, ids: &[SignalId]) -> Vec<Box<dyn Any>> {
        let mut out = Vec::with_capacity(ids.len());
        for &sid in ids {
            if let Some(set) = self.signal_subscribers.get_mut(sid.0 as usize) {
                set.clear();
            }
            // Drop any JS notifier for this signal — the closure
            // typically captures a `Weak<RefCell<Backend>>` and a
            // signal-stringifier, both of which become meaningless
            // once the signal slot is freed.
            self.signal_js_notifiers.remove(&(sid.0 as u64));
            // Drop any robot watch entry for this slot at the same point —
            // eager counterpart to the watch registry's lazy generation
            // pruning, so a freed signal leaves the inspector immediately.
            #[cfg(feature = "robot")]
            crate::robot::watch::on_signal_freed(sid.0);
            if let Some(slot) = self.signals.get_mut(sid.0 as usize) {
                if let Some(boxed) = slot.take() {
                    out.push(boxed);
                    // Bump the slot's generation so any still-living
                    // handle to this signal (e.g. captured by a
                    // detached/deferred callback that outlived the
                    // scope) is recognised as stale on its next
                    // read/write instead of aliasing whatever signal
                    // recycles this slot next.
                    if let Some(g) = self.signal_gen.get_mut(sid.0 as usize) {
                        *g = g.wrapping_add(1);
                    }
                    self.signal_free.push(sid.0);
                }
            }
        }
        out
    }

    /// Single-effect free path used by `Effect`'s own `Drop` when it
    /// owns the slot. Doesn't have the nested-Scope problem because
    /// an owning `Effect` handle is dropped *after* `Effect::new`
    /// returns, i.e. from user code that doesn't hold the arena.
    fn free_effect(&mut self, id: EffectId) {
        self.unsubscribe_effect(id);
        if let Some(slot) = self.effects.get_mut(id.0 as usize) {
            if slot.take().is_some() {
                self.effect_free.push(id.0);
            }
        }
    }
}

struct SignalInner<T> {
    value: T,
}

struct EffectInner {
    /// `None` while the effect is mid-run — `run_effect` takes the
    /// closure out before invoking it so signal callbacks can re-borrow
    /// the arena, then puts it back when the run finishes. Making this
    /// `Option` (rather than `Box<...>` with a per-fire `mem::replace`
    /// against a freshly-allocated no-op) saves one Box allocation
    /// per effect fire — material at hierarchy-scale fan-outs (2k+
    /// leaves all subscribing to one signal) where the allocator
    /// churn dominated the per-effect cost.
    run: Option<Box<dyn FnMut()>>,
    /// Callbacks registered via `on_cleanup` during the effect's last
    /// run. Drained and fired *before* the next re-run, and again on
    /// effect disposal via `Drop`. LIFO to mirror typical
    /// resource-acquisition order.
    cleanups: Vec<Box<dyn FnOnce()>>,
    /// Snapshot of the active-scope stack at the moment this effect
    /// was constructed. Restored onto `ACTIVE_SCOPE` for the duration
    /// of each re-run so `inject<T>` (and any other code that walks
    /// the scope chain) sees the effect's creation-time owners
    /// regardless of where in the call graph the signal write that
    /// triggered the re-run actually happened. Equivalent to Solid's
    /// "owner" field on a computation.
    ///
    /// Safety: raw pointers are valid for the effect's lifetime —
    /// scope-drop frees its adopted effects before its own teardown,
    /// so any scope on this snapshot is still live whenever its
    /// pointer is dereferenced.
    owning_stack: Vec<*mut Scope>,
    /// Opt-in fast path: when `true`, [`run_effect`] skips both
    /// `clear_effect_dependencies` (and the matching `signal.get`
    /// re-track on the way back in) — the caller has asserted the
    /// effect's dep set is stable across re-runs. Use only when
    /// every re-run reads exactly the same set of signals that the
    /// initial run did (the walker's reactive-text builder is the
    /// canonical caller). Set by [`Effect::new_with_stable_deps`]
    /// after the initial run; defaults to `false` for any effect
    /// created through [`Effect::new`].
    ///
    /// Why this is a win at hierarchy scale: every fire of a
    /// general-purpose Effect drains the effect's dep set (Vec
    /// alloc + HashSet remove per dep against a 2k-entry
    /// subscriber HashSet) and then re-inserts via the next
    /// `signal.get()`. For an effect with one stable dep that's
    /// dispatched 2k times in a fan-out, the clear/resub dance
    /// dominates the per-leaf cost.
    stable_deps: bool,
}

impl Drop for EffectInner {
    fn drop(&mut self) {
        // Same thread-death guard as `Scope::drop`: when this effect is
        // being dropped by the arena's own TLS destructor (leaked-at-exit
        // effect), running author cleanups would re-enter destroyed TLS
        // and abort. Drop them un-run; the thread is exiting.
        if ARENA.try_with(|_| ()).is_err() {
            self.cleanups.clear();
            return;
        }
        for cb in self.cleanups.drain(..).rev() {
            cb();
        }
    }
}

// =============================================================================
// untrack
// =============================================================================

/// Types that can be read as a tracked dependency of an effect — a
/// single `Signal<T>` or a tuple of trackables. The associated `Value`
/// is the resolved value(s) the consumer sees.
///
/// Implementors include `Signal<T>` (yielding `T`) and tuples of up to
/// four `Trackable`s (yielding the tuple of values). This is the trait
/// `on(deps, ..)` uses to separate "what to subscribe to" from "what
/// the body does."
pub trait Trackable: Copy + 'static {
    type Value: Clone + 'static;
    /// Reads the tracked value(s). Must be called from inside an effect
    /// for subscriptions to be recorded.
    fn track(&self) -> Self::Value;
}

impl<T: Clone + 'static> Trackable for Signal<T> {
    type Value = T;
    fn track(&self) -> T {
        self.get()
    }
}

impl<A: Trackable, B: Trackable> Trackable for (A, B) {
    type Value = (A::Value, B::Value);
    fn track(&self) -> Self::Value {
        (self.0.track(), self.1.track())
    }
}

impl<A: Trackable, B: Trackable, C: Trackable> Trackable for (A, B, C) {
    type Value = (A::Value, B::Value, C::Value);
    fn track(&self) -> Self::Value {
        (self.0.track(), self.1.track(), self.2.track())
    }
}

impl<A: Trackable, B: Trackable, C: Trackable, D: Trackable> Trackable for (A, B, C, D) {
    type Value = (A::Value, B::Value, C::Value, D::Value);
    fn track(&self) -> Self::Value {
        (self.0.track(), self.1.track(), self.2.track(), self.3.track())
    }
}

/// Reacts to changes in a specific set of dependencies, passing the new
/// and previous values to the body. Decouples "what to subscribe to"
/// from "what to read" — reads inside the body do NOT add to the
/// subscription set.
///
/// The body fires once at creation with `prev = None`, then once per
/// dependency change with `prev = Some(<last value>)`. For "only fire
/// on subsequent changes" semantics, use [`on_defer`].
///
/// ```ignore
/// // Single signal:
/// on(count, |new, prev| {
///     log!("{} -> {:?}", new, prev);
/// });
///
/// // Tuple of signals — body runs when either changes:
/// on((first, last), |(f, l), _prev| {
///     update_full_name(format!("{} {}", f, l));
/// });
/// ```
pub fn on<D, F>(deps: D, mut f: F) -> Effect
where
    D: Trackable,
    F: FnMut(&D::Value, Option<&D::Value>) + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;
    let prev: Rc<RefCell<Option<D::Value>>> = Rc::new(RefCell::new(None));
    Effect::new(move || {
        // Read deps under tracking — this is what builds the
        // subscription set.
        let new = deps.track();
        // Pull the previous value out before invoking the body. Cloning
        // here is cheap relative to the body's typical work; it lets
        // the body access `prev` without re-entering the RefCell.
        let prev_value = prev.borrow().clone();
        // Run the body untracked so reads inside it don't subscribe.
        untrack(|| f(&new, prev_value.as_ref()));
        *prev.borrow_mut() = Some(new);
    })
}

/// Like [`on`] but skips the initial run — the body only fires from the
/// first dependency change onward. The subscription set is still
/// established eagerly so no change is missed.
///
/// Useful for "react to user-driven changes, not initial mount" cases:
/// saving to disk, animating from a known value, kicking off a
/// fetch only when params actually change.
///
/// ```ignore
/// on_defer(query, |new, _| {
///     spawn_fetch(new.clone());
/// });
/// ```
pub fn on_defer<D, F>(deps: D, mut f: F) -> Effect
where
    D: Trackable,
    F: FnMut(&D::Value, Option<&D::Value>) + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;
    let prev: Rc<RefCell<Option<D::Value>>> = Rc::new(RefCell::new(None));
    Effect::new(move || {
        let new = deps.track();
        let prev_value = prev.borrow().clone();
        // Skip the very first invocation — the body only fires once
        // there's a meaningful "previous" to compare against.
        if prev_value.is_some() {
            untrack(|| f(&new, prev_value.as_ref()));
        }
        *prev.borrow_mut() = Some(new);
    })
}

/// Creates a memoized derivation backed by a [`Signal<T>`]. `f` is
/// auto-tracked: each signal it reads becomes a dependency. When any
/// dependency changes, `f` is re-evaluated and the new value is
/// **compared against the previous one with `PartialEq`** — subscribers
/// are only notified when the result actually differs.
///
/// The cache is the key win: three sites reading the same `memo` share
/// one computation per dep change. Equality-gated notification is
/// load-bearing for downstream perf — a derivation like
/// `count.get() > 10` only re-renders consumers when the boolean
/// actually flips, not every time `count` changes.
///
/// Returns a `Signal<T>` so the memo plugs into every existing consumer
/// (`.get()`, `text(|| memo.get())`, `.bind(...)`, style closures,
/// etc.) without a new type. The signal is owned by the active scope —
/// calling `memo` outside a scope is allowed but the underlying effect
/// will leak.
///
/// For types without `PartialEq`, or to override the equality check,
/// see [`memo_with`].
///
/// ```ignore
/// let first = signal("Jane".to_string());
/// let last = signal("Doe".to_string());
/// let full = memo(move || format!("{} {}", first.get(), last.get()));
///
/// // Anywhere a Signal<String> works:
/// text(move || full.get());
/// ```
/// Bundles a `Signal<S>` state cell with a typed action dispatcher,
/// in the shape of React's `useReducer`. Returns `(state, dispatch)`:
///
/// - `state` is a plain [`Signal<S>`] — every existing consumer
///   (`text(|| state.get())`, `.bind(...)`, `effect!`, `memo`,
///   stylesheet closures, etc.) works unchanged.
/// - `dispatch` is a typed `Fn(A)` that applies the user-supplied
///   reducer function `(&S, A) -> S` to the current state and writes
///   the result back.
///
/// This is intentionally **a pattern, not a primitive**: it composes
/// from `Signal` + a closure. No new arena slot type, no new
/// scope-cleanup path, no backend hooks. Generator backends (Roku)
/// that need structured transpilation of reducer dispatch should
/// reach for `Action`/`Derived` shapes instead — those carry the
/// metadata required to ship the function across the wire.
///
/// The reducer call is wrapped in `untrack` so calling `dispatch`
/// from inside an effect doesn't accidentally subscribe that effect
/// to the state signal. (`Signal::set` itself is non-subscribing;
/// the wrap is just for the `state.get()` read of the previous
/// value.)
///
/// ```ignore
/// enum Counter { Inc, Dec, Reset }
///
/// let (count, dispatch) = reducer(0i32, |&n, action| match action {
///     Counter::Inc   =>  n + 1,
///     Counter::Dec   =>  n - 1,
///     Counter::Reset =>  0,
/// });
///
/// button("+", move || dispatch(Counter::Inc));
/// text(move || format!("count: {}", count.get()));
/// ```
pub fn reducer<S, A>(
    initial: S,
    f: impl Fn(&S, A) -> S + 'static,
) -> (Signal<S>, impl Fn(A))
where
    S: Clone + 'static,
{
    let state = Signal::new(initial);
    let dispatch = move |action: A| {
        // A dispatch is one reactive cycle — uniform with the other entry
        // points and coalescing if a reducer ever writes sibling signals.
        // Nesting inside an event handler's cycle is a harmless no-op.
        cycle(|| {
            // Untracked read so a `dispatch` call from inside an effect
            // doesn't subscribe that effect to `state` (it's the
            // dispatcher's job to *cause* state changes, not to react
            // to them).
            let current = untrack(|| state.get());
            let next = f(&current, action);
            state.set(next);
        });
    };
    (state, dispatch)
}

/// A cached derived signal: recomputes `f` whenever a signal it reads
/// changes, and notifies subscribers only when the new value differs
/// from the old (`T: PartialEq`). Use it for derived state that's read
/// in several places or is expensive to compute — the work runs once
/// per dependency change, not once per read. For a value without
/// `PartialEq`, or a custom "close enough" comparison, use
/// [`memo_with`]. For a cheap one-off derivation, a plain closure or
/// `rx!` is lighter.
///
/// ```ignore
/// let count = signal(0);
/// let doubled = memo(move || count.get() * 2);
/// // `doubled` is a Signal<i32>; reads stay cached until `count` changes.
/// ```
///
/// This plain fn is the canonical form (the historical `memo!` macro was
/// removed — the author already writes the closure, so there was no
/// token work left to justify a macro; contrast `effect!`, which implies
/// `move` over a bare block).
///
/// Returns the READ half only ([`ReadSignal`]): a memo is a pure
/// derivation, so its output is not writable at the type level.
pub fn memo<T>(f: impl Fn() -> T + 'static) -> ReadSignal<T>
where
    T: Clone + PartialEq + 'static,
{
    memo_with(|a, b| a == b, f)
}

/// Like [`memo`] but with a caller-supplied equality function. Use this
/// for types that don't impl `PartialEq` (e.g. when `T` contains a
/// trait object) or when "equal enough to skip notification" doesn't
/// match `PartialEq` (e.g. tolerance-based float comparison).
pub fn memo_with<T, F, E>(eq: E, f: F) -> ReadSignal<T>
where
    T: Clone + 'static,
    F: Fn() -> T + 'static,
    E: Fn(&T, &T) -> bool + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    // Seed the output signal with an initial value computed under
    // `untrack` — the real subscription set gets recorded by the
    // effect's first run below. Doing this here (rather than letting
    // the effect's first run produce it) means consumers reading the
    // signal between `memo(..)` returning and the effect's first
    // notification get a coherent value instead of `T::default()`.
    //
    // Both this initial call and every subsequent re-run in the effect
    // below run with `MemoComputeGuard` active so `Signal::set` /
    // `Signal::update` from inside `f` panic loudly instead of
    // injecting a side-effecting node into the dep graph.
    let initial = {
        let _g = MemoComputeGuard::enter();
        untrack(|| f())
    };
    let signal = Signal::new(initial.clone());

    // The effect compares each new computation against its own
    // last-emitted value. Reading `signal.get()` from inside the effect
    // would subscribe the effect to its own output — fine for the
    // equality check itself, but it'd mean every `signal.set(new)` call
    // re-fires the effect (caught by the same-id reentry guard, but
    // wasteful). Holding `last` in an Rc<RefCell> keeps the comparison
    // off the dep graph entirely.
    let last: Rc<RefCell<T>> = Rc::new(RefCell::new(initial));
    let last_for_effect = last.clone();

    let e = Effect::new(move || {
        // Block-scope the guard so it covers only the user's `f()`. The
        // memo's own `signal.set(new)` below is the *output* write of
        // the derivation and must NOT be flagged.
        let new = {
            let _g = MemoComputeGuard::enter();
            f()
        };
        let differs = !eq(&*last_for_effect.borrow(), &new);
        if differs {
            *last_for_effect.borrow_mut() = new.clone();
            signal.set(new);
        }
    });

    // The effect must outlive this function. Inside an active scope,
    // the scope already adopted the slot (`e.owns == false`) and this is
    // a no-op. Outside any scope, the local binding's Drop would free the
    // slot — `persist` pins it for the lifetime of the thread instead,
    // the same way a bare `Signal::new` outside a scope is never reclaimed
    // (the returned handle is `Copy` with no `Drop`).
    e.persist();

    // Hand out only the read half: a memo is a pure derivation, and a
    // writable output invited a heisenbug — an author `.set()` "worked"
    // until the next dependency change silently clobbered it. The
    // internal `signal` binding above keeps the write capability for the
    // derivation effect itself.
    signal.read_only()
}

// =============================================================================
// Context (provide / inject)
// =============================================================================

/// Provides a value of type `T` to descendant scopes. The provision
/// lives until the current scope drops; inner scopes inherit it via
/// [`inject`], and inner provisions of the same type shadow outer ones
/// for that subtree.
///
/// Disambiguating two providers of the same Rust type is the caller's
/// job: wrap each in a distinct newtype (e.g. `struct PrimaryColor(...)`
/// vs `struct AccentColor(...)`) so the type system gives each
/// provision a unique key.
///
/// Panics if called outside any active scope, or from inside a memo's
/// compute closure (memos must be pure derivations).
///
/// ```ignore
/// // Once at app root:
/// provide(Theme::dark());
/// provide(Locale("en-US".into()));
///
/// // Anywhere in the subtree:
/// let theme: Option<Theme> = inject::<Theme>();
/// let locale: Locale = inject_or(Locale("en-US".into()));
/// ```
pub fn provide<T: 'static>(value: T) {
    assert_not_in_memo_compute();
    ACTIVE_SCOPE.with(|s| {
        let stack = s.borrow();
        let Some(&top) = stack.last() else {
            panic!(
                "`provide` called outside any active reactive scope. \
                 Wrap with `with_scope(..)` or call from inside a \
                 component or effect body."
            );
        };
        // SAFETY: identical invariant to `register_signal` etc —
        // ACTIVE_SCOPE only holds pointers to `Scope` values currently
        // borrowed by `with_scope`, so no aliasing.
        unsafe {
            (*top)
                .contexts
                .push((std::any::TypeId::of::<T>(), Box::new(value)));
        }
    });
}

/// Returns a clone of the nearest ancestor-provided value of type `T`.
/// Walks the active scope stack innermost-first — inner provisions
/// shadow outer ones. Returns `None` if no provider exists.
///
/// For non-`Clone` types, see [`with_inject`].
pub fn inject<T: Clone + 'static>() -> Option<T> {
    with_inject::<T, _>(|v| v.clone())
}

/// Like [`inject`] but returns `default` when no provider exists.
/// Convenience wrapper that avoids `unwrap_or` noise at read sites.
pub fn inject_or<T: Clone + 'static>(default: T) -> T {
    inject::<T>().unwrap_or(default)
}

/// Reads the nearest ancestor-provided value of type `T` by reference,
/// without cloning. Returns `Some(f(&value))` if a provider exists,
/// `None` otherwise.
///
/// Use this for types that aren't `Clone` or are expensive to clone:
/// `with_inject::<Theme, _>(|theme| theme.background)` is cheaper than
/// `inject::<Theme>().map(|t| t.background)` when `Theme` is large.
pub fn with_inject<T: 'static, R>(f: impl FnOnce(&T) -> R) -> Option<R> {
    let target = std::any::TypeId::of::<T>();
    ACTIVE_SCOPE.with(|s| {
        let stack = s.borrow();
        // Innermost scope first; within a scope, last-provided wins
        // (matches "later provision shadows earlier" if a single scope
        // ever provides the same type twice — undefined but harmless).
        for &scope_ptr in stack.iter().rev() {
            let scope = unsafe { &*scope_ptr };
            for (tid, boxed) in scope.contexts.iter().rev() {
                if *tid == target {
                    if let Some(v) = boxed.downcast_ref::<T>() {
                        return Some(f(v));
                    }
                }
            }
        }
        None
    })
}

/// Registers a callback to run when the surrounding reactive context
/// is torn down.
///
/// Resolution rules:
///
/// - If called from inside an `Effect`'s run, fires **before the next
///   re-run** and **on effect disposal**. Lets an effect release the
///   resources it acquired on its previous pass — timers, listeners,
///   in-flight requests — before the new pass replaces them.
/// - Otherwise, if called from inside a `Scope` (e.g. a component body
///   between mount and unmount, outside any effect), fires once when
///   the scope drops.
/// - Outside any reactive context, the callback is dropped immediately.
pub fn on_cleanup<F: FnOnce() + 'static>(f: F) {
    let mut slot: Option<Box<dyn FnOnce()>> = Some(Box::new(f));

    // Active-effect path: attach to the currently-running effect's
    // cleanup list so the callback fires on its next re-run / drop.
    let current_eid = CURRENT.with(|c| *c.borrow());
    if let Some(eid) = current_eid {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(any)) = a.effects.get_mut(eid.0 as usize) {
                if let Some(inner) = any.downcast_mut::<EffectInner>() {
                    if let Some(cb) = slot.take() {
                        inner.cleanups.push(cb);
                    }
                }
            }
        });
        if slot.is_none() {
            return;
        }
    }

    // Active-scope fallback: attach to the topmost scope's cleanup list.
    if let Some(cb) = slot.take() {
        ACTIVE_SCOPE.with(|s| {
            if let Some(&top) = s.borrow().last() {
                // SAFETY: ACTIVE_SCOPE pointers are only set while the
                // referenced Scope is borrowed by `with_scope`, mirroring
                // `register_signal` / `register_effect` / `adopt_guard`.
                unsafe { (*top).cleanups.push(cb); }
            }
            // No active scope: callback is dropped silently. Matches
            // Solid's `onCleanup` (top-level call is a no-op).
        });
    }
}

/// Runs `f` with subscription tracking disabled. Any `Signal::get()` calls
/// inside `f` will return their current value without subscribing the
/// enclosing effect.
pub fn untrack<R, F: FnOnce() -> R>(f: F) -> R {
    // Bump the user-intent depth so the snapshot-read diagnostic stays
    // quiet inside: `untrack` is the author (or a framework read path)
    // explicitly declaring "no subscription wanted". The WALKER's
    // build-region untrack (`untrack_for_build`) deliberately does NOT
    // bump it — see that fn for why.
    USER_UNTRACK_DEPTH.with(|d| d.set(d.get() + 1));
    let prev = CURRENT.with(|c| c.borrow_mut().take());
    let result = f();
    CURRENT.with(|c| *c.borrow_mut() = prev);
    USER_UNTRACK_DEPTH.with(|d| d.set(d.get() - 1));
    result
}

/// Walker-internal variant of [`untrack`] for build regions (`when`/
/// `switch`/`each`/`dynamic`/`presence` branch construction). Clears the
/// tracking context like `untrack` but does NOT count as declared
/// snapshot intent — component bodies rebuilt inside these regions must
/// still trip the untracked-build-read diagnostic (the hoisted-snapshot
/// trap is just as much a bug inside a `when` branch as at the root; a
/// plain `untrack` here would have silenced it exactly there).
pub(crate) fn untrack_for_build<R, F: FnOnce() -> R>(f: F) -> R {
    let prev = CURRENT.with(|c| c.borrow_mut().take());
    let result = f();
    CURRENT.with(|c| *c.borrow_mut() = prev);
    result
}

// =============================================================================
// Untracked-build-read diagnostic (the hoisted-snapshot trap)
// =============================================================================
//
// `let too_short = name.get().len() < 3;` at component-body level runs
// once and freezes — but LOOKS reactive. The macros can't catch it (they
// can't see types, and a snapshot is a legitimate idiom elsewhere), so
// the runtime does, where types are resolved: `#[component]` brackets
// every body with a build probe, and `Signal::get` warns when a read
// happens (a) during a component build, (b) with no tracked consumer
// (`CURRENT` empty, not in a memo compute), and (c) without declared
// snapshot intent (`untrack` / `.get_untracked()`). Same mechanism as
// MobX's `observableRequiresReaction` and Leptos's outside-tracking
// warning. Debug builds only; release compiles to nothing.

thread_local! {
    /// Depth of user-intent [`untrack`] calls (also bumped by internal
    /// read paths that use `untrack` — those are intentional untracked
    /// reads too). Distinct from the walker's `untrack_for_build`.
    static USER_UNTRACK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

#[cfg(any(debug_assertions, feature = "debug-stats"))]
thread_local! {
    /// Names of `#[component]` bodies currently executing (innermost
    /// last). Pushed/popped by [`ComponentBuildProbe`]. Compiled in debug
    /// builds (for the hoisted-snapshot diagnostic) AND under `debug-stats`
    /// in any build (so reactive-profile effect attribution can name the
    /// owning component — see `current_build_component` /
    /// `debug::record_effect_created`).
    static COMPONENT_BUILD_STACK: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

#[cfg(debug_assertions)]
thread_local! {
    /// Dedup + test-observation record: (component name, signal id)
    /// pairs already warned this thread.
    static SNAPSHOT_WARNINGS: RefCell<(
        std::collections::HashSet<(usize, u64)>,
        Vec<(&'static str, u64)>,
    )> = RefCell::new((std::collections::HashSet::new(), Vec::new()));
}

/// RAII marker for "a `#[component]` body is executing". Emitted by the
/// `#[component]` macro at the top of every body; do not construct by
/// hand.
#[doc(hidden)]
pub struct ComponentBuildProbe {
    #[cfg(any(debug_assertions, feature = "debug-stats"))]
    _priv: (),
}

#[doc(hidden)]
#[inline]
pub fn __component_build_probe(name: &'static str) -> ComponentBuildProbe {
    #[cfg(any(debug_assertions, feature = "debug-stats"))]
    {
        COMPONENT_BUILD_STACK.with(|s| s.borrow_mut().push(name));
        return ComponentBuildProbe { _priv: () };
    }
    #[cfg(not(any(debug_assertions, feature = "debug-stats")))]
    {
        let _ = name;
        ComponentBuildProbe {}
    }
}

impl Drop for ComponentBuildProbe {
    fn drop(&mut self) {
        #[cfg(any(debug_assertions, feature = "debug-stats"))]
        COMPONENT_BUILD_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Innermost `#[component]` currently building, if any. `None` when the
/// build stack is unavailable (neither `debug_assertions` nor `debug-stats`)
/// or empty (an effect created outside any component body — app init, an
/// event handler, a service install). Used by [`Effect::create`] to attribute
/// an effect to its owning component in the reactive profile.
#[inline]
pub(crate) fn current_build_component() -> Option<&'static str> {
    #[cfg(any(debug_assertions, feature = "debug-stats"))]
    {
        COMPONENT_BUILD_STACK.with(|s| s.borrow().last().copied())
    }
    #[cfg(not(any(debug_assertions, feature = "debug-stats")))]
    {
        None
    }
}

/// The diagnostic itself — called from `Signal::get` in debug builds.
/// Warns once per (component, signal) pair.
#[cfg(debug_assertions)]
fn maybe_warn_untracked_build_read(signal_id: u64) {
    // Tracked consumer running → this read subscribes; not a snapshot.
    if CURRENT.with(|c| c.borrow().is_some()) {
        return;
    }
    // Declared intent (user `untrack` / `.get_untracked()` / internal
    // untracked read paths) or a memo compute (deps tracked by the memo's
    // own effect) → quiet.
    if USER_UNTRACK_DEPTH.with(|d| d.get()) > 0 || MEMO_COMPUTE_DEPTH.with(|d| d.get()) > 0 {
        return;
    }
    let Some(component) = COMPONENT_BUILD_STACK.with(|s| s.borrow().last().copied()) else {
        // Not inside a component build (event handler, app init, …) —
        // an untracked read there is normal imperative code.
        return;
    };
    let fresh = SNAPSHOT_WARNINGS.with(|w| {
        let (seen, log) = &mut *w.borrow_mut();
        if seen.insert((component.as_ptr() as usize, signal_id)) {
            log.push((component, signal_id));
            true
        } else {
            false
        }
    });
    if fresh {
        crate::log_warn!(
            "[reactive] `.get()` during build of `{component}` outside any tracked \
             context — this read is a one-time snapshot and will NEVER update \
             (signal id {signal_id}). For a live derivation use `memo(move || …)` \
             or read inside the binding closure; if the snapshot is intentional, \
             use `.get_untracked()`."
        );
    }
}

/// Test/tooling hook: drain the warnings recorded so far on this thread.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn __take_untracked_build_read_warnings() -> Vec<(&'static str, u64)> {
    SNAPSHOT_WARNINGS.with(|w| {
        let (seen, log) = &mut *w.borrow_mut();
        seen.clear();
        std::mem::take(log)
    })
}

/// Runs `f` with the active-scope stack temporarily emptied. Any
/// `Signal::new` / `Effect::new` calls inside `f` will *not* be adopted
/// by the surrounding render scope — they live until the thread exits.
///
/// Used by registry-style stores (e.g. `TOKEN_REGISTRY`) whose entries
/// are thread-lifetime by contract: a render scope that happens to be
/// the first one to touch a registry-managed signal must not become its
/// owner, or the entry will dangle when the scope drops.
pub(crate) fn unscope<R, F: FnOnce() -> R>(f: F) -> R {
    let saved = ACTIVE_SCOPE.with(|s| std::mem::take(&mut *s.borrow_mut()));
    let result = f();
    ACTIVE_SCOPE.with(|s| *s.borrow_mut() = saved);
    result
}

/// Diagnostic snapshot of arena state. Counts in-use vs total slots
/// for signals, effects, and refs. `in_use` is the number of `Some`
/// slots; `total` is `Vec::len()`. Slots are never recycled today, so
/// `total` grows monotonically with the number of signals/effects/refs
/// ever created — useful for detecting if a rebuild loop is generating
/// slots faster than expected.
///
/// Also reports the sum of `len()` across all per-signal subscriber
/// sets and per-effect dependency sets, so a leak that left stale
/// entries in those sets would show up as `total_subscribers` or
/// `total_deps` growing while `in_use_*` stayed bounded.
pub fn arena_stats() -> ArenaStats {
    ARENA.with(|a| {
        let a = a.borrow();
        ArenaStats {
            signals_in_use: a.signals.iter().filter(|s| s.is_some()).count(),
            signals_total: a.signals.len(),
            effects_in_use: a.effects.iter().filter(|e| e.is_some()).count(),
            effects_total: a.effects.len(),
            refs_in_use: a.refs.iter().filter(|r| r.is_some()).count(),
            refs_total: a.refs.len(),
            total_subscribers: a.signal_subscribers.iter().map(|s| s.len()).sum(),
            total_deps: a.effect_dependencies.iter().map(|d| d.len()).sum(),
        }
    })
}

/// Current generation of a signal slot, or `0` if the index is out of
/// range. Robot-only: the signal-watch registry captures this at
/// registration time so a later read can detect a recycled slot. See
/// [`signal_is_live`].
#[cfg(feature = "robot")]
pub fn signal_generation(signal_id_raw: u64) -> u32 {
    ARENA.with(|a| {
        a.borrow()
            .signal_gen
            .get(signal_id_raw as usize)
            .copied()
            .unwrap_or(0)
    })
}

/// `true` if the slot for `signal_id_raw` is currently occupied AND its
/// generation still matches `gen`. Lets robot-side introspection read a
/// watched signal *without* risking `Signal::get`'s stale-read panic: a
/// freed-then-recycled slot fails the generation check, so the watch
/// registry skips it instead of reading the new occupant's value. One
/// arena borrow, two `Vec` index reads. Robot-only.
#[cfg(feature = "robot")]
pub fn signal_is_live(signal_id_raw: u64, gen: u32) -> bool {
    let idx = signal_id_raw as usize;
    ARENA.with(|a| {
        let a = a.borrow();
        a.signal_gen.get(idx).copied() == Some(gen)
            && a.signals.get(idx).map_or(false, |s| s.is_some())
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ArenaStats {
    pub signals_in_use: usize,
    pub signals_total: usize,
    pub effects_in_use: usize,
    pub effects_total: usize,
    pub refs_in_use: usize,
    pub refs_total: usize,
    pub total_subscribers: usize,
    pub total_deps: usize,
}

// =============================================================================
// Signal<T>
// =============================================================================

/// A copy-handle to a reactive value.
///
/// `Signal<T>` is `Copy`, so it can be captured into multiple closures
/// without explicit `.clone()` calls. The underlying storage lives in a
/// thread-local arena owned by the enclosing render `Owner` (which holds
/// a `Scope`); when the owner drops, the signal's slot is freed.
pub struct Signal<T> {
    id: SignalId,
    /// The arena slot generation this handle was minted with. If the
    /// slot is later freed and recycled, its generation advances and
    /// this handle becomes stale — reads/writes through it no-op rather
    /// than touch the slot's new occupant. See `Arena::signal_gen`.
    gen: u32,
    _phantom: PhantomData<T>,
}

impl<T> Copy for Signal<T> {}
impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self { *self }
}

impl<T> Default for Signal<T> {
    /// A *detached* signal: a sentinel id that points to no arena slot, so
    /// constructing it allocates nothing. This exists so a component whose
    /// props include a required `Signal` can still derive `Default` and be
    /// dispatched by `ui!` (which builds props via `..Default::default()`,
    /// evaluating that base on every render — hence it must be allocation
    /// free). The real signal is always supplied as a prop and overwrites
    /// this before use; if a required `Signal` prop is *omitted*, reading
    /// the detached signal panics with the standard "signal used after its
    /// scope was dropped" message rather than silently misbehaving.
    fn default() -> Self {
        Self { id: SignalId(u32::MAX), gen: 0, _phantom: PhantomData }
    }
}

impl<T> Signal<T> {
    /// Stable identifier for this signal's arena slot. Used by the
    /// f-string text slots and the Roku backend to wire reactive bindings:
    /// the macro captures `signal.id()` at expansion-call time so the
    /// `RokuBackend` can emit `BindText { signal_ids: [..], .. }`
    /// commands referencing this exact signal.
    ///
    /// The id is stable for the signal's lifetime. It's an arena slot
    /// index under the hood; we widen to `u64` so the wire format
    /// (which serializes signals as `u64`) doesn't depend on the
    /// internal `u32` width.
    ///
    /// Intended for macro and backend consumption — author code
    /// normally just uses `signal.get()` / `signal.set(..)`.
    pub fn id(&self) -> u64 {
        self.id.0 as u64
    }
}

impl<T: Clone + 'static> Signal<T> {
    /// Creates a signal in the global arena. The slot is freed when the
    /// surrounding render `Owner` drops. (For tests and ad-hoc usage outside
    /// a render tree, the slot leaks until the thread exits.)
    #[track_caller]
    pub fn new(value: T) -> Self {
        let (id, gen) = ARENA.with(|a| {
            a.borrow_mut().insert_signal(SignalInner { value })
        });
        register_signal(id);
        // Reactive-profile identity: stash the author-code creation site so a
        // profile can name this signal. `#[track_caller]` resolves the
        // `Location` at compile time — no runtime cost when `debug-stats` is
        // off (the call inlines to nothing). Public `signal()` is also
        // `#[track_caller]`, so the location points past the wrapper at author
        // code. Internal callers (memo output, reducer state) record their own
        // in-crate site, which is an accepted limit.
        crate::debug::record_signal_created(id.0, std::panic::Location::caller());
        Self { id, gen, _phantom: PhantomData }
    }

    /// Read the current value **without subscribing** — and, unlike a
    /// bare `.get()` outside a tracked context, with the intent
    /// DECLARED: this is the spelling for an intentional build-time
    /// snapshot, and it silences the dev-build hoisted-snapshot
    /// diagnostic. Leptos parity: same name, same semantics.
    pub fn get_untracked(&self) -> T {
        untrack(|| self.get())
    }

    pub fn get(&self) -> T {
        let sid = self.id;
        // Dev-build diagnostic: a `.get()` during a component build with
        // no tracked consumer is a one-time snapshot — usually the
        // hoisted-snapshot trap, occasionally intentional (then say
        // `.get_untracked()`). Zero code in release builds.
        #[cfg(debug_assertions)]
        maybe_warn_untracked_build_read(sid.0 as u64);
        // Read the value first, generation-checked. `None` means the
        // signal's slot was freed (scope unmounted) — a stale read. We
        // deliberately do NOT record a subscription in that case (the
        // slot's subscriber set belongs to whatever recycled it), and
        // we don't have a `T` to hand back, so this is the one stale
        // access that still panics — a read of a disposed signal is a
        // genuine logic error with no safe value to return. The
        // reported crash (and the dangerous use-after-free shape) is a
        // stale *write*, which `set`/`update` below turn into no-ops.
        let value = with_signal::<T, _>(sid, self.gen, |inner| inner.value.clone())
            .unwrap_or_else(|| {
                // `with_signal` returns `None` for two distinct situations
                // that need different diagnostics:
                //  - generation MISMATCH: the slot was freed (scope unmounted,
                //    `signal_gen` bumped) and possibly recycled — a genuine
                //    use-after-scope.
                //  - generation MATCHES but the slot is momentarily empty: the
                //    signal's box has been moved out of the arena by an
                //    in-progress `set`/`update` (or `async_reducer`/`reducer`
                //    apply) on THIS same signal, and we're reading it
                //    re-entrantly from inside that window — typically an effect
                //    woken by a synchronous fan-out the mutation kicked off.
                //    The slot isn't dropped, it's *taken* (a freed slot would
                //    have failed the generation check), so the "scope dropped"
                //    message would point authors at the wrong bug.
                let gen_still_live = ARENA.with(|a| {
                    a.borrow().signal_gen.get(sid.0 as usize).copied() == Some(self.gen)
                });
                if gen_still_live {
                    panic!(
                        "signal {:?} read re-entrantly while it was \
                         mid-mutation: its storage is moved out of the arena \
                         for the duration of its own `set`/`update`/reducer- \
                         apply closure, so a read reaching it from inside that \
                         window (e.g. an effect woken by a write the closure \
                         performed) finds it empty. Wrap the writes in \
                         `batch`/`cycle` so the fan-out defers until the \
                         closure returns, or move the dependent write into a \
                         separate `effect!`.",
                        sid
                    )
                }
                panic!("signal used after its scope was dropped (id {:?})", sid)
            });
        // Record subscription if an effect is currently running. The
        // arena holds the inverse map (`signal_subscribers` +
        // `effect_dependencies`) so each link is recorded under a
        // single mutable borrow.
        CURRENT.with(|c| {
            if let Some(eid) = *c.borrow() {
                ARENA.with(|a| {
                    let mut a = a.borrow_mut();
                    if let Some(subs) = a.signal_subscribers.get_mut(sid.0 as usize) {
                        subs.insert(eid);
                    }
                    if let Some(deps) = a.effect_dependencies.get_mut(eid.0 as usize) {
                        deps.insert(sid);
                    }
                });
            }
        });
        value
    }

    pub fn set(&self, value: T) {
        assert_not_in_memo_compute();
        // Stale write (slot freed/recycled since this handle was minted)
        // → no-op. Returning here is essential: skipping the subscriber
        // fan-out below means we never fire the new occupant's
        // subscribers with our (wrong-typed) write.
        if with_signal_mut::<T, _>(self.id, self.gen, |inner| {
            inner.value = value;
        })
        .is_none()
        {
            return;
        }
        // `set` is the always-notify primitive. Inside a batch, mark the
        // signal dirty with `force` so the flush wakes its subscribers
        // unconditionally (and taints any `set_if_changed` sharing the
        // window). Outside a batch, fan out now.
        if is_batching() {
            window_record(self.id, true, None);
        } else {
            // Subscriber lists are kept tight on the cleanup side (effect
            // drop / effect re-run), so no pruning pass needed here.
            fan_out_now(self.id);
        }
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        assert_not_in_memo_compute();
        // Stale update → no-op (see `set`).
        if with_signal_mut::<T, _>(self.id, self.gen, |inner| {
            f(&mut inner.value);
        })
        .is_none()
        {
            return;
        }
        if is_batching() {
            window_record(self.id, true, None);
        } else {
            fan_out_now(self.id);
        }
    }
}

impl<T: PartialEq + 'static> Signal<T> {
    /// Like [`set`](Signal::set), but skips the subscriber fan-out when
    /// the write leaves the value **equal** to what it held — eliminating
    /// needless effect re-runs / re-renders when app code re-sets a signal
    /// to a value it already has (re-applying derived state, syncing
    /// props, "set on every event" handlers).
    ///
    /// Inside a [`batch`], the comparison is against the **window-initial**
    /// value, not each intermediate: a signal set `A → B → A` within one
    /// batch nets to no change and never wakes its subscribers — strictly
    /// stronger than a per-write compare, which would see two real changes.
    /// Pairs with the way `when`/`memo` already memoize downstream, closing
    /// the no-op-rerender class app-wide.
    ///
    /// Compares in place — no `Clone` of the new value. `NaN` is never
    /// equal to itself, so a `NaN`-valued set always notifies (acceptable).
    pub fn set_if_changed(&self, value: T) {
        assert_not_in_memo_compute();
        if is_batching() {
            // Defer to the flush. Write the new value, capturing the OLD
            // value by move (free — it's being overwritten anyway). On the
            // signal's FIRST dirty this window, stash a closure that
            // compares that window-initial original against the final
            // value at flush time.
            let first = !BATCH_PENDING.with(|b| {
                b.borrow().as_ref().map(|w| w.entries.contains_key(&self.id)).unwrap_or(false)
            });
            let old = with_signal_mut::<T, _>(self.id, self.gen, |inner| {
                std::mem::replace(&mut inner.value, value)
            });
            let Some(old) = old else { return }; // stale → no-op
            if first {
                let check: Box<dyn FnOnce(&dyn Any) -> bool> = Box::new(move |cur: &dyn Any| {
                    // `cur` is this signal's live `SignalInner<T>`; changed
                    // iff its current value differs from the original.
                    cur.downcast_ref::<SignalInner<T>>()
                        .map(|si| si.value != old)
                        .unwrap_or(true)
                });
                window_record(self.id, false, Some(check));
            }
            return;
        }
        // Non-batched: compare-then-write inside one guard (no read-then-
        // write lock gap), fan out only on a real change.
        let changed = with_signal_mut::<T, _>(self.id, self.gen, |inner| {
            if inner.value == value {
                false
            } else {
                inner.value = value;
                true
            }
        });
        if changed == Some(true) {
            fan_out_now(self.id);
        }
    }
}

impl<T: PartialEq + Clone + 'static> Signal<T> {
    /// Like [`update`](Signal::update), but skips the fan-out when `f`
    /// leaves the value unchanged. Needs `Clone` to snapshot the original
    /// for comparison (`update` mutates in place); use
    /// [`set_if_changed`](Signal::set_if_changed) when you have the new
    /// value directly. Batch semantics match `set_if_changed` — the
    /// comparison is against the window-initial value.
    pub fn update_if_changed<F: FnOnce(&mut T)>(&self, f: F) {
        assert_not_in_memo_compute();
        if is_batching() {
            let first = !BATCH_PENDING.with(|b| {
                b.borrow().as_ref().map(|w| w.entries.contains_key(&self.id)).unwrap_or(false)
            });
            let old = with_signal_mut::<T, _>(self.id, self.gen, |inner| {
                let old = inner.value.clone();
                f(&mut inner.value);
                old
            });
            let Some(old) = old else { return };
            if first {
                let check: Box<dyn FnOnce(&dyn Any) -> bool> = Box::new(move |cur: &dyn Any| {
                    cur.downcast_ref::<SignalInner<T>>()
                        .map(|si| si.value != old)
                        .unwrap_or(true)
                });
                window_record(self.id, false, Some(check));
            }
            return;
        }
        let changed = with_signal_mut::<T, _>(self.id, self.gen, |inner| {
            let old = inner.value.clone();
            f(&mut inner.value);
            inner.value != old
        });
        if changed == Some(true) {
            fan_out_now(self.id);
        }
    }
}

/// Look up and invoke a JS-side notifier for `sid`, if one was
/// registered via [`register_signal_js_notifier`]. Called from
/// `Signal::set` / `Signal::update` after the Rust subscriber
/// fan-out completes.
///
/// The notifier closure typically reads the signal's current value
/// (via its captured `Signal<T>` handle), stringifies it, and ships
/// the new value across the wasm→JS boundary. Whatever it does is
/// opaque to the framework — we just call the closure if present.
///
/// We clone the `Rc` out under the arena borrow, then drop the
/// borrow before invoking the closure. The closure may re-enter the
/// arena (e.g. to read another signal) so we mustn't hold the
/// borrow across the call.
fn notify_js_subscriber(sid: SignalId) {
    let notifier = ARENA.with(|a| {
        a.borrow()
            .signal_js_notifiers
            .get(&(sid.0 as u64))
            .cloned()
    });
    if let Some(n) = notifier {
        n();
    }
}

/// Register a JS-side notifier for `signal_id_raw` (the `u64`
/// returned by [`Signal::id`]). Replaces any previously-registered
/// notifier for the same signal — at most one notifier per signal
/// is the contract, because the notifier's job is "ship the new
/// value to JS", and shipping twice is wasteful (the JS side
/// fans out to multiple bindings on its own).
///
/// `notifier` runs from inside `Signal::set` / `Signal::update`
/// AFTER the Rust subscriber fan-out completes. It typically
/// captures the `Signal<T>` handle + a backend reference and ships
/// the new value to the backend's JS bridge. Whatever it does is
/// opaque to the framework.
///
/// Cleanup: the notifier is dropped automatically when the
/// associated signal's slot is freed (see `take_signals_batched`).
/// Callers don't need to unregister manually unless they want to
/// detach a notifier from a still-live signal.
// =============================================================================
// Read/write capability halves (ReadSignal / WriteSignal)
// =============================================================================

/// The read half of a [`Signal`] — same arena slot, same generational
/// stale-safety, same `Copy` ergonomics, but the TYPE exposes only the
/// tracked-read surface. Use it in a signature to prove the holder
/// observes without mutating: a prop typed `ReadSignal<T>` cannot write
/// the caller's state, and [`memo`] returns one so a derivation's output
/// can't be injected over.
///
/// Obtained via [`Signal::read_only`] / [`Signal::split`] (or `.into()`).
/// Deliberately a newtype with NO `Deref` to `Signal` — deref would hand
/// the write half back and the capability split would be decorative.
pub struct ReadSignal<T>(Signal<T>);

/// The write half of a [`Signal`] — the mirror of [`ReadSignal`]: only
/// the write surface, so a child handed a `WriteSignal<T>` can report
/// values upward but can't read (and therefore can't accidentally
/// subscribe itself). Obtained via [`Signal::write_only`] /
/// [`Signal::split`].
pub struct WriteSignal<T>(Signal<T>);

// Manual Copy/Clone/Default mirroring `Signal`'s own impls — a derive
// would add a spurious `T: Copy`/`T: Clone`/`T: Default` bound (the
// handle is (id, gen); `T` is phantom).
impl<T> Copy for ReadSignal<T> {}
impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Default for ReadSignal<T> {
    fn default() -> Self {
        ReadSignal(Signal::default())
    }
}
impl<T> Copy for WriteSignal<T> {}
impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Default for WriteSignal<T> {
    fn default() -> Self {
        WriteSignal(Signal::default())
    }
}

impl<T> ReadSignal<T> {
    /// The underlying slot id — same value as the source signal's
    /// [`Signal::id`], so id-keyed integrations (f-string text bindings,
    /// robot watch) work with either handle.
    pub fn id(&self) -> u64 {
        self.0.id()
    }
}

impl<T: Clone + 'static> ReadSignal<T> {
    /// Tracked read — identical semantics to [`Signal::get`] (subscribes
    /// the running effect, stale-slot panic rules unchanged).
    pub fn get(&self) -> T {
        self.0.get()
    }

    /// Untracked read with declared intent — see [`Signal::get_untracked`].
    pub fn get_untracked(&self) -> T {
        self.0.get_untracked()
    }
}

impl<T> WriteSignal<T> {
    /// The underlying slot id (see [`ReadSignal::id`]).
    pub fn id(&self) -> u64 {
        self.0.id()
    }
}

impl<T: Clone + 'static> WriteSignal<T> {
    /// Identical to [`Signal::set`] (including the stale-slot no-op).
    pub fn set(&self, value: T) {
        self.0.set(value);
    }

    /// Identical to [`Signal::update`].
    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        self.0.update(f);
    }
}

impl<T: PartialEq + 'static> WriteSignal<T> {
    /// Identical to [`Signal::set_if_changed`].
    pub fn set_if_changed(&self, value: T) {
        self.0.set_if_changed(value);
    }
}

impl<T: PartialEq + Clone + 'static> WriteSignal<T> {
    /// Identical to [`Signal::update_if_changed`].
    pub fn update_if_changed<F: FnOnce(&mut T)>(&self, f: F) {
        self.0.update_if_changed(f);
    }
}

impl<T> Signal<T> {
    /// Split into read and write capability halves over the SAME slot —
    /// the Leptos-transliterable form: `let (count, set_count) =
    /// signal(0).split();`. The unified handle stays valid; the halves
    /// are additional views, not a transfer.
    pub fn split(self) -> (ReadSignal<T>, WriteSignal<T>) {
        (ReadSignal(self), WriteSignal(self))
    }

    /// The read-only view of this signal (see [`ReadSignal`]).
    pub fn read_only(self) -> ReadSignal<T> {
        ReadSignal(self)
    }

    /// The write-only view of this signal (see [`WriteSignal`]).
    pub fn write_only(self) -> WriteSignal<T> {
        WriteSignal(self)
    }
}

impl<T> From<Signal<T>> for ReadSignal<T> {
    fn from(s: Signal<T>) -> Self {
        ReadSignal(s)
    }
}

impl<T> From<Signal<T>> for WriteSignal<T> {
    fn from(s: Signal<T>) -> Self {
        WriteSignal(s)
    }
}

pub fn register_signal_js_notifier<F: Fn() + 'static>(signal_id_raw: u64, notifier: F) {
    ARENA.with(|a| {
        a.borrow_mut()
            .signal_js_notifiers
            .insert(signal_id_raw, std::rc::Rc::new(notifier));
    });
}

/// Drop the JS-side notifier for `signal_id_raw`. No-op if none
/// was registered. Use when the JS-side subscription pool empties
/// for a still-live signal (e.g. the last text binding on `global`
/// unmounted but `global` itself is still in use).
pub fn unregister_signal_js_notifier(signal_id_raw: u64) {
    ARENA.with(|a| {
        a.borrow_mut()
            .signal_js_notifiers
            .remove(&signal_id_raw);
    });
}

/// `true` if `signal_id_raw` has a JS-side notifier registered.
/// Useful for the variant / backend to gate its own per-binding
/// setup: if the framework doesn't have a notifier slot for this
/// signal, the JS-side updates would never fire.
pub fn signal_has_js_notifier(signal_id_raw: u64) -> bool {
    ARENA.with(|a| {
        a.borrow()
            .signal_js_notifiers
            .contains_key(&signal_id_raw)
    })
}

/// RAII guard that marks the enclosing block as a `memo` compute. While
/// any guard is live on the current thread, [`Signal::set`] and
/// [`Signal::update`] panic — preventing the bug where a memo's
/// supposed-to-be-pure derivation has a side effect that re-enters the
/// reactive graph during its own read.
struct MemoComputeGuard;

impl MemoComputeGuard {
    fn enter() -> Self {
        MEMO_COMPUTE_DEPTH.with(|d| d.set(d.get() + 1));
        MemoComputeGuard
    }
}

impl Drop for MemoComputeGuard {
    fn drop(&mut self) {
        MEMO_COMPUTE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Panics if called from inside a memo's compute closure. Invoked at
/// the top of `Signal::set` / `Signal::update` so the failure points at
/// the offending write, not at the downstream cascade it would have
/// produced.
fn assert_not_in_memo_compute() {
    if MEMO_COMPUTE_DEPTH.with(|d| d.get()) > 0 {
        panic!(
            "Signal::set / Signal::update called inside a memo's compute closure. \
             Memos must be pure derivations of their input signals. \
             For side effects use an `Effect` or `on(deps, ..)`; \
             for derived values use additional memos."
        );
    }
}

/// A signal dirtied during an open `batch(..)`. The fan-out decision is
/// deferred to the flush: `force` short-circuits to "always notify"
/// (a plain `set`/`update`, or a `set_if_changed` that ran in a window
/// already tainted by a force-write); otherwise `check` — captured by
/// the *first* `set_if_changed` of the window — compares the
/// window-initial value against the final one and notifies only on a
/// real net change.
struct DirtyEntry {
    /// Any always-notify write (`set`/`update`) seen this window taints
    /// the entry: at flush we notify regardless of `check`.
    force: bool,
    /// `Some` once the first `set_if_changed` of the window captured the
    /// pre-window value. Called once at flush with the signal's *current*
    /// `SignalInner<T>` (type-erased); returns `true` iff the value
    /// changed. Carries the typed original by move, so it needs no
    /// `PartialEq` bound at this erased call site — the bound lives on
    /// `set_if_changed`, which built the closure.
    check: Option<Box<dyn FnOnce(&dyn Any) -> bool>>,
}

/// The set of signals dirtied since the outermost `batch(..)` opened,
/// in first-dirty order. `order` drives a deterministic flush (signals
/// written earliest wake their effects first); `entries` holds the
/// per-signal change-detection state.
#[derive(Default)]
struct DirtyWindow {
    order: Vec<SignalId>,
    entries: FxHashMap<SignalId, DirtyEntry>,
}

/// `true` while an outermost `batch(..)` window is open on this thread.
fn is_batching() -> bool {
    BATCH_PENDING.with(|b| b.borrow().is_some())
}

/// Record `sid` as dirtied in the open batch window. `force` ORs into
/// the entry's force flag; `check` is installed only on the signal's
/// *first* appearance this window (so the captured original is the
/// window-initial value, never a later intermediate). Caller guarantees
/// a window is open.
fn window_record(sid: SignalId, force: bool, check: Option<Box<dyn FnOnce(&dyn Any) -> bool>>) {
    BATCH_PENDING.with(|b| {
        let mut b = b.borrow_mut();
        let w = b.as_mut().expect("window_record called with no open batch");
        if let Some(e) = w.entries.get_mut(&sid) {
            e.force |= force;
            // Keep the first-dirty `check`; a later write's snapshot
            // would be of an intermediate value, not the window start.
        } else {
            w.order.push(sid);
            w.entries.insert(sid, DirtyEntry { force, check });
        }
    });
}

/// Type-erased read of a signal's `SignalInner<T>` box as `&dyn Any`,
/// for the flush-time `check`. `None` if the slot is absent (freed). We
/// don't generation-check here: a synchronous batch can't free a slot
/// between writes and flush (no user code runs in between), and a freed
/// slot has no subscribers to wake anyway.
fn with_signal_any<R>(sid: SignalId, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
    ARENA.with(|a| {
        let a = a.borrow();
        a.signals
            .get(sid.0 as usize)
            .and_then(|o| o.as_ref())
            .map(|boxed| f(&**boxed))
    })
}

/// Immediate (non-batched) fan-out for one signal write: wake its
/// subscribers, then fire its JS notifier. Matches the historical
/// ordering (effects before JS notify).
fn fan_out_now(sid: SignalId) {
    let subs = collect_subscribers(sid);
    // Reactive-profile: this write is one transaction; `run_effects` emits the
    // per-effect and commit events between these markers. `txn_report` folds
    // the bracketed span into a record keyed by this signal.
    #[cfg(feature = "debug-stats")]
    crate::debug::record_txn_enter(vec![sid.0 as u64], subs.len());
    run_effects(&subs);
    #[cfg(feature = "debug-stats")]
    crate::debug::record_txn_exit();
    notify_js_subscriber(sid);
}

/// Runs `f` with effect fan-out deferred until `f` returns. Multiple
/// signal writes inside the closure coalesce into one re-run per
/// subscribing effect, in first-write order. Nested calls reuse the
/// outermost batch's queue and don't flush early.
///
/// Returns whatever `f` returns. The result of effects fired during the
/// flush is not exposed — effects don't return values to their
/// triggering write.
///
/// ```ignore
/// // Without batch: three subscriber fan-outs, intermediate states
/// // visible.
/// first.set("Jane");
/// last.set("Doe");
/// age.set(34);
///
/// // With batch: one fan-out per subscriber, intermediate states
/// // are not observed by any effect.
/// batch(|| {
///     first.set("Jane");
///     last.set("Doe");
///     age.set(34);
/// });
/// ```
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    // Only the outermost batch owns the window. Nested batches see
    // `Some(_)` already in place and skip the install — when the outer
    // returns, it flushes everything written across all nested batches
    // in one pass.
    let is_outer = BATCH_PENDING.with(|b| {
        let mut b = b.borrow_mut();
        if b.is_none() {
            *b = Some(DirtyWindow::default());
            true
        } else {
            false
        }
    });

    let result = f();

    if is_outer {
        // Take the window out and clear the slot *before* running
        // effects. An effect's body can call set() — that write should
        // see `BATCH_PENDING = None` (the batch is over) and fan out
        // synchronously, not record into a window we're already flushing.
        let window = BATCH_PENDING.with(|b| b.borrow_mut().take()).unwrap_or_default();

        let DirtyWindow { order, mut entries } = window;
        // Resolve each dirty signal's change decision in first-dirty
        // order, collecting the subscribers to wake (deduped, first-seen
        // order preserved) and the signals whose JS notifiers should
        // fire. A net-zero `set_if_changed` window contributes neither.
        let mut ordered: Vec<EffectId> = Vec::new();
        let mut changed_sids: Vec<SignalId> = Vec::with_capacity(order.len());
        for sid in order {
            let entry = entries.remove(&sid).expect("dirty order/entries out of sync");
            let changed = entry.force
                || match entry.check {
                    // Compare window-initial vs current. Missing slot →
                    // not changed (no subscribers to wake regardless).
                    Some(check) => with_signal_any(sid, check).unwrap_or(false),
                    // No force and no snapshot can't happen (force-writes
                    // set `force`, `set_if_changed` sets `check`); treat
                    // defensively as changed.
                    None => true,
                };
            if changed {
                changed_sids.push(sid);
                for eid in collect_subscribers(sid) {
                    // For typical batch sizes (a handful of writes) the
                    // linear `contains` beats allocating a HashSet.
                    if !ordered.contains(&eid) {
                        ordered.push(eid);
                    }
                }
            }
        }

        if !ordered.is_empty() {
            // Reactive-profile: a batch flush is one transaction whose triggers
            // are all the signals that net-changed in the window.
            #[cfg(feature = "debug-stats")]
            crate::debug::record_txn_enter(
                changed_sids.iter().map(|s| s.0 as u64).collect(),
                ordered.len(),
            );
            run_effects(&ordered);
            #[cfg(feature = "debug-stats")]
            crate::debug::record_txn_exit();
        }
        // JS notifiers fire after the Rust fan-out, matching the
        // non-batched path, and once per net-changed signal.
        for sid in changed_sids {
            notify_js_subscriber(sid);
        }
    }

    result
}

/// Run `f` as one **reactive cycle** (a "turn"): every signal write it
/// performs is queued and the subscriber fan-out + coalesced layout pass
/// happen **once**, at the end, instead of synchronously per write.
///
/// This is the framework's turn boundary. The runtime wraps every
/// *cycle entry point* in it automatically — input event handlers (at
/// the point a handler is attached, so every backend inherits it without
/// per-platform code), async task completions, timer/animation-frame
/// callbacks, and reducer dispatch — so author code never has to think
/// about batching: a handler that writes five signals re-renders its
/// subscribers once, not five times.
///
/// Mechanically identical to [`batch`] (it *is* `batch`); the separate
/// name documents intent at the framework's automatic call sites and
/// keeps them greppable. `batch` remains the name author code uses to
/// group writes manually. Nesting composes — an inner `cycle`/`batch`
/// joins the outer window and only the outermost flushes — so wrapping a
/// handler that a backend *also* happens to wrap is a harmless no-op.
///
/// Note: the value cell is still written synchronously (a `get()` on the
/// next line sees the new value); only the *notification* is queued. The
/// flush runs before paint within the same synchronous turn, so there is
/// no added frame latency — this is end-of-turn coalescing, not async
/// deferral.
#[inline]
pub fn cycle<R>(f: impl FnOnce() -> R) -> R {
    batch(f)
}

/// Debug-stats introspection: how many effects are currently subscribed
/// to `sid`. Used by theme-toggle diagnostics to distinguish "the token
/// set was a no-op" from "the set fanned out to nobody".
#[cfg(feature = "debug-stats")]
pub(crate) fn debug_subscriber_count_by_raw(raw: u64) -> usize {
    ARENA.with(|a| {
        a.borrow()
            .signal_subscribers
            .get(raw as usize)
            .map(|s| s.len())
            .unwrap_or(0)
    })
}

/// Snapshot the current subscribers of `sid` into a `Vec` so we can
/// release the arena borrow before running effects (each effect run
/// re-borrows the arena to read/write its own state).
fn collect_subscribers(sid: SignalId) -> Vec<EffectId> {
    ARENA.with(|a| {
        a.borrow()
            .signal_subscribers
            .get(sid.0 as usize)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    })
}

/// Generation-checked read access. Returns `None` when the handle is
/// STALE — its slot was freed (and possibly recycled) since the handle
/// was minted, so the generation no longer matches (or the id is the
/// detached-`Default` sentinel / out of range). A matching generation
/// guarantees the slot still holds the original `SignalInner<T>`, so
/// the downcast below is an invariant, not a fallible cast.
fn with_signal<T: 'static, R>(
    id: SignalId,
    gen: u32,
    f: impl FnOnce(&SignalInner<T>) -> R,
) -> Option<R> {
    ARENA.with(|arena| {
        let arena = arena.borrow();
        if arena.signal_gen.get(id.0 as usize).copied() != Some(gen) {
            return None; // stale handle or detached sentinel
        }
        let slot = arena.signals.get(id.0 as usize).and_then(|o| o.as_ref())?;
        let inner = slot
            .downcast_ref::<SignalInner<T>>()
            .expect("internal: signal type mismatch (generation matched but type differs)");
        Some(f(inner))
    })
}

/// Generation-checked mutable access. Returns `None` (no-op) on a stale
/// handle — see [`with_signal`]. The take/run/restore dance is
/// unchanged from the live path.
fn with_signal_mut<T: 'static, R>(
    id: SignalId,
    gen: u32,
    f: impl FnOnce(&mut SignalInner<T>) -> R,
) -> Option<R> {
    // Bail before taking the slot if the handle is stale. Single-
    // threaded with no user code between this check and the take below,
    // so the generation can't change underneath us.
    if ARENA.with(|a| a.borrow().signal_gen.get(id.0 as usize).copied()) != Some(gen) {
        return None;
    }
    // `f` is a user closure (e.g. `Signal::update`'s) that may create or
    // touch OTHER signals — each of which re-enters the arena RefCell.
    // Holding the arena borrow across `f` would panic ("RefCell already
    // borrowed"), so we TAKE the signal's box out of the arena, drop the
    // borrow, run `f`, then restore the box.
    //
    // Safe against aliasing: the taken slot is left `None` but is NOT
    // added to `signal_free`, and `insert_signal` only recycles slots
    // popped from that free-list — so a signal created inside `f` can
    // never grab this slot. (Re-entrant access to *this same* signal
    // inside `f` is the one unsupported case; the slot reads as `None`.)
    // Mark the arena as mid-mutation for the take/run/restore window.
    // The signal's slot reads `None` until we restore it; a deferred
    // scope-anchored callback that fires during this window must NOT
    // touch a signal (its slot may be the one we took, or another
    // effect's dep recording may be half-done). `is_reactive_busy`
    // exposes this so those callbacks skip + re-arm. The guard's Drop
    // runs even if `f` panics, so the busy count can't get stuck.
    let _busy = ReactiveBusyGuard::enter();
    // Generation already matched above, so the slot is occupied — the
    // only way `take()` yields `None` here is the documented unsupported
    // case of re-entrant mutation of *this same* signal inside `f`,
    // which stays a panic (a real logic bug, distinct from a stale
    // handle, which `None`-ed out before this point).
    let mut boxed = ARENA.with(|a| {
        a.borrow_mut()
            .signals
            .get_mut(id.0 as usize)
            .and_then(|o| o.take())
            .unwrap_or_else(|| {
                // Actionable diagnostic for the one genuinely-unsupported
                // re-entrancy case. Without this the read of the taken slot
                // surfaces downstream as the opaque "RefCell already mutably
                // borrowed", which gives no hint that a `set`/`update`/reducer
                // apply touching its own signal is the culprit. `batch`/`cycle`
                // can't rescue this — the box is moved out regardless of
                // batching — so the fix is to restructure, not to wrap.
                panic!(
                    "re-entrant mutation of signal {:?}: it was written from \
                     inside its own `set`/`update` closure (or an \
                     `async_reducer`/`reducer` apply that targets this same \
                     signal). The signal's storage is moved out of the arena \
                     for the duration of the closure, so it cannot be touched \
                     re-entrantly. Fix: mutate only the `&mut` value the \
                     closure is given; perform any other write to THIS signal \
                     from a separate `effect!` that reacts to it.",
                    id
                )
            })
    });
    let inner = boxed
        .downcast_mut::<SignalInner<T>>()
        .expect("internal: signal type mismatch (generation matched but type differs)");
    let result = f(inner);
    ARENA.with(|a| {
        a.borrow_mut().signals[id.0 as usize] = Some(boxed);
    });
    Some(result)
}

/// Drop every dependency link the effect currently holds. Called right
/// before a re-run so the new dep set reflects only the signals read on
/// this pass. Same operation `Arena::unsubscribe_effect` does internally,
/// exposed via a thread-local helper because `run_effect` already holds
/// the arena once and we want to keep the touch minimal.
fn clear_effect_dependencies(eid: EffectId) {
    ARENA.with(|a| a.borrow_mut().unsubscribe_effect(eid));
}

// =============================================================================
// Effect
// =============================================================================

/// Handle to a reactive effect. Drop it to stop the effect from re-running.
///
/// The handle owns the effect's slot in the arena; dropping the handle
/// frees the slot and immediately removes the effect from every
/// signal's subscriber set via `Arena::unsubscribe_effect`, so no stale
/// entries are left behind for later sweeps to clean up.
pub struct Effect {
    id: EffectId,
    /// If true, dropping this handle should free the effect slot. The
    /// renderer's `Scope` takes ownership by setting this to false on the
    /// handle it received; the scope then frees the slot at its own drop.
    owns: bool,
}

impl Drop for Effect {
    fn drop(&mut self) {
        if self.owns {
            ARENA.with(|a| a.borrow_mut().free_effect(self.id));
        }
    }
}

impl Effect {
    /// Shared constructor for every effect handle. `register` decides
    /// whether the active scope (if any) adopts the slot:
    ///
    /// - `register == true` — the active scope takes ownership (`owns ==
    ///   false`) and frees the slot on its own teardown; with no active
    ///   scope the returned handle owns the slot. This is the `effect!`
    ///   path (via [`Effect::scoped`]) and the legacy [`Effect::new`].
    /// - `register == false` — the slot is **never** adopted by a scope;
    ///   the returned handle always owns it. This is the [`watch`] path:
    ///   the caller owns the [`Subscription`], independent of the tree.
    ///
    /// Either way the effect runs once immediately and re-runs when a
    /// signal it read changes.
    #[track_caller]
    pub(crate) fn create<F: FnMut() + 'static>(f: F, register: bool) -> Self {
        // Reactive-profile identity: capture the author-code creation site
        // AND the innermost `#[component]` building right now, BEFORE
        // `run_effect` runs the closure (which could itself create nested
        // effects and shift the build stack). `#[track_caller]` on this and
        // the `new`/`scoped`/`new_with_stable_deps` wrappers propagates the
        // location past them to author code; zero cost when `debug-stats` off.
        let location = std::panic::Location::caller();
        let component = current_build_component();
        // Capture the owner chain at creation time so re-runs can
        // restore it. `with_scope` keeps these pointers valid for as
        // long as each scope is held by an outer call frame.
        let owning_stack: Vec<*mut Scope> =
            ACTIVE_SCOPE.with(|s| s.borrow().clone());
        let id = ARENA.with(|a| {
            a.borrow_mut().insert_effect(EffectInner {
                run: Some(Box::new(f)),
                cleanups: Vec::new(),
                owning_stack,
                stable_deps: false,
            })
        });
        crate::debug::record_effect_created(id.0, location, component);
        let registered = if register { register_effect(id) } else { false };
        run_effect(id);
        Effect { id, owns: !registered }
    }

    /// Creates an effect and runs it once. Any signals read during the run
    /// re-fire the effect on change.
    ///
    /// If a `Scope` is active (via `with_scope`), the effect's slot is
    /// owned by that scope — the returned `Effect` handle's drop is a
    /// no-op and the slot is freed when the scope drops. If no scope is
    /// active, the returned handle owns the slot directly.
    ///
    /// Crate-internal adopt-or-own constructor used throughout the framework
    /// (the render walker, `resource`, `memo_with`, …). It is **not** part of
    /// the public API — author code uses `effect! { … }` (scope-owned) or
    /// [`watch`] (caller-owned). Writing `Effect::new` in author code is a
    /// privacy error, which is the point: the tempting React/Solid-style name
    /// is unreachable.
    #[track_caller]
    pub(crate) fn new<F: FnMut() + 'static>(f: F) -> Self {
        Self::create(f, true)
    }

    /// Raw arena slot index of this effect. Crate-internal, for the walker
    /// to attach a reactive-profile binding label via `debug::label_effect`
    /// (e.g. naming the node a text effect drives). Only needed under
    /// `debug-stats`; gated to avoid a dead-code warning otherwise.
    #[cfg(feature = "debug-stats")]
    pub(crate) fn raw_id(&self) -> u32 {
        self.id.0
    }

    /// Creates a **scope-owned** effect — the form behind `effect! { … }`.
    ///
    /// Debug-asserts that a reactive scope is active, because a reactive
    /// effect only makes sense inside the component tree: the owning scope
    /// frees it on teardown. To react to a signal from *outside* the tree
    /// (app init, an async callback, a platform/service install), use
    /// [`watch`] and hold the returned [`Subscription`] instead.
    ///
    /// Returns `()` — there is no handle to manage. The active scope owns
    /// the effect; it lives exactly as long as that scope.
    #[track_caller]
    pub fn scoped<F: FnMut() + 'static>(f: F) {
        debug_assert!(
            ACTIVE_SCOPE.with(|s| !s.borrow().is_empty()),
            "effect! {{ … }} used with no active reactive scope. A reactive \
             effect must be created inside a component body (or other \
             reactive scope) so its owning scope can free it. To react to a \
             signal from outside the tree, use `watch(…)` and store the \
             returned `Subscription`."
        );
        // The active scope adopts the slot (`owns == false`); dropping the
        // returned no-op handle here is intentional — the scope owns it.
        let _adopted = Self::create(f, true);
    }

    /// Like [`Effect::new`] but flips the effect into a fast-path
    /// re-run mode after the initial tracking pass:
    ///
    /// - The initial run records dependencies normally (the closure
    ///   reads signals via `Signal::get`, tracking populates the
    ///   subscriber + dep sets exactly as for `Effect::new`).
    /// - Every subsequent fire **skips** `clear_effect_dependencies`
    ///   and runs the closure with tracking suppressed (CURRENT
    ///   temporarily `None`), so the matching `signal.get` re-track
    ///   inside the body becomes a no-op too.
    ///
    /// Net per-fire savings: one HashSet remove + one Vec alloc on
    /// the clear side, plus one HashSet insert on the re-track
    /// side. Material at fan-outs of thousands.
    ///
    /// # When to use
    ///
    /// Only when the closure provably reads the **same** set of
    /// signals on every fire. Reactive text bindings created by
    /// the framework's `text(closure)` factory are the canonical
    /// fit — their closure body is a pure value computation whose
    /// dep set is fixed at construction time.
    ///
    /// # When NOT to use
    ///
    /// Closures with conditional reads (e.g. `if a.get() { b.get() }
    /// else { c.get() }` where `a`'s value flips between fires) —
    /// the second branch's reads would no-op against the frozen
    /// subscriber set, and the original branch's signal would keep
    /// firing this effect even after no longer being read. Use
    /// [`Effect::new`] for those.
    ///
    /// Crate-internal perf fast-path used only by the framework's reactive
    /// text bindings — not part of the public API.
    #[track_caller]
    pub(crate) fn new_with_stable_deps<F: FnMut() + 'static>(f: F) -> Self {
        // Reactive-profile identity, same as `create`. This constructor is the
        // walker's node-binding path (text/style effects), so the entry
        // recorded here is what `debug::label_effect` later attaches
        // "text#<id>" / "style#<id>" to — without it the label would have no
        // slot and be silently dropped.
        let location = std::panic::Location::caller();
        let component = current_build_component();
        let owning_stack: Vec<*mut Scope> =
            ACTIVE_SCOPE.with(|s| s.borrow().clone());
        // Insert with `stable_deps: false` so the first `run_effect`
        // takes the full tracking path and the dep set gets recorded.
        // Flip the flag right after — every subsequent fire then
        // sees `stable_deps: true` and short-circuits.
        let id = ARENA.with(|a| {
            a.borrow_mut().insert_effect(EffectInner {
                run: Some(Box::new(f)),
                cleanups: Vec::new(),
                owning_stack,
                stable_deps: false,
            })
        });
        crate::debug::record_effect_created(id.0, location, component);
        let registered = register_effect(id);
        run_effect(id);
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(slot)) = a.effects.get_mut(id.0 as usize) {
                if let Some(inner) = slot.downcast_mut::<EffectInner>() {
                    inner.stable_deps = true;
                }
            }
        });
        Effect { id, owns: !registered }
    }

    /// Hand the effect to whoever owns it and stop tracking the handle —
    /// keeping the effect alive past the current statement without holding
    /// the returned handle yourself.
    ///
    /// - If a reactive scope was active at creation, that scope already
    ///   owns the slot (`owns == false`); this just drops the no-op handle
    ///   and the scope frees the effect on teardown.
    /// - If no scope was active, dropping the handle would otherwise cancel
    ///   the effect at end-of-statement; `persist` pins it for the process
    ///   lifetime instead.
    ///
    /// This is the named form of the `mem::forget(effect)` idiom used
    /// internally by `memo_with` / `resource` / animation bindings. Library
    /// and app code should call `persist()` rather than reaching for
    /// `mem::forget` — the adopt-or-pin behaviour is identical, but the
    /// intent is explicit and greppable.
    ///
    /// Crate-internal (`memo_with` / `resource` / animation bindings). Not
    /// public — author code uses [`watch`] + holding the [`Subscription`]
    /// (or [`Subscription::leak`] for a process-lifetime pin).
    pub(crate) fn persist(self) {
        // `owns == false` (scope-adopted): forget drops a no-op handle.
        // `owns == true` (no scope): forget skips the cancelling Drop,
        // pinning the slot for the process lifetime. Both are exactly the
        // behaviour the prior `mem::forget(effect)` call sites relied on
        // (see `memo_in_scope_releases_signal_and_effect_on_scope_drop`).
        std::mem::forget(self);
    }
}

/// Internal entry point for the `#[method]` codegen's component-registration
/// keepalive — **not** an author API (the `#[component]` macro emits it).
///
/// It exists because proc-macro output lands in user crates and can only
/// call `pub` items of `runtime_core`, yet [`Effect::new`] is `pub(crate)`.
/// This is the one sanctioned `pub` effect constructor, deliberately given a
/// `__`-prefixed, purpose-specific name so no author reaches for it.
///
/// Semantics: adopt-or-own with **no** scope assertion. The build walker
/// always runs components inside a `Scope` (so the registration guard lives
/// exactly as long as the component's mounted subtree), but a component
/// constructor invoked directly outside `mount` — as some tests do — must
/// not panic. That rules out [`Effect::scoped`]; this mirrors the historical
/// `let _ = Effect::new(...)` exactly.
#[doc(hidden)]
pub fn __component_keepalive_effect<F: FnMut() + 'static>(f: F) {
    let _ = Effect::create(f, true);
}

/// A caller-owned reactive subscription, created by [`watch`].
///
/// The watched closure runs once immediately and re-runs whenever any
/// signal it read changes — for exactly as long as this handle is alive.
/// Dropping the `Subscription` disposes the effect and runs its
/// `on_cleanup` callbacks; [`Subscription::leak`] keeps it for the process
/// lifetime.
///
/// Unlike `effect! { … }` — which is owned by the surrounding component
/// scope and needs no handle — a `Subscription` is owned by **you**. Store
/// it where its lifetime should match: a struct field, a thread-local, the
/// owning service. This is the right tool for reactive wiring created
/// *outside* the component tree (app init, async callbacks, platform
/// integrations), where there is no scope to own the effect.
#[must_use = "a Subscription disposes its effect when dropped — store it (or call \
              `.leak()`) to keep the effect running"]
pub struct Subscription {
    effect: Effect,
}

impl Subscription {
    /// Keep the subscription alive for the rest of the process, giving up
    /// the handle. The honest, greppable replacement for the old
    /// `Effect::persist()` *pin* semantics — e.g. a global theme observer
    /// installed once at app boot that should never be torn down.
    pub fn leak(self) {
        // The inner effect `owns` its slot (created with `register ==
        // false`); forgetting it skips the cancelling Drop, pinning the
        // slot for the process lifetime.
        std::mem::forget(self.effect);
    }
}

/// Create a caller-owned reactive [`Subscription`]: run `f` now and re-run
/// it whenever a signal it read changes, until the returned handle is
/// dropped.
///
/// This is the out-of-tree counterpart to `effect! { … }`. Reactivity
/// inside the component tree belongs in `effect!` (the scope owns it);
/// reactivity wired up *outside* the tree — at app init, in an async
/// callback, in a platform/service install — belongs here, where the
/// lifetime is explicit and yours to hold.
///
/// The slot is **never** adopted by an active scope: the returned
/// `Subscription` always owns it, so `watch` behaves identically whether
/// or not a scope happens to be active at the call site.
///
/// ```ignore
/// // Stored on the owning struct; dropped (and disposed) with `self`.
/// self.insets_sub = Some(super::watch(move || apply_insets(safe_area_insets().get())));
/// ```
#[must_use = "a Subscription disposes its effect when dropped — store it (or call \
              `.leak()`) to keep the effect running"]
pub fn watch<F: FnMut() + 'static>(f: F) -> Subscription {
    Subscription { effect: Effect::create(f, false) }
}

/// Transitive run-stack depth above which `run_effect` panics. Catches
/// the mutual-loop case (A writes B, B writes A, …) before it
/// stack-overflows. Tuned high enough that legitimately deep dependency
/// graphs don't trip it, low enough that the offending stack frames are
/// still recognizable in a panic backtrace.
const MAX_EFFECT_DEPTH: u32 = 256;

/// RAII guard that increments [`EFFECT_DEPTH`] on creation and
/// decrements on drop. Drop runs on unwind too, so a user-code panic
/// inside an effect doesn't leave the counter stuck high.
struct DepthGuard;

impl DepthGuard {
    /// Enter a new effect-run frame. Returns the post-increment depth so
    /// the caller can compare against [`MAX_EFFECT_DEPTH`]. The guard is
    /// returned regardless — if the caller decides to panic, dropping
    /// the guard during unwind still restores the counter.
    fn enter() -> (Self, u32) {
        let depth = EFFECT_DEPTH.with(|d| {
            let mut d = d.borrow_mut();
            *d += 1;
            *d
        });
        (DepthGuard, depth)
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        EFFECT_DEPTH.with(|d| {
            let mut d = d.borrow_mut();
            *d = d.saturating_sub(1);
        });
    }
}

/// Run the effect with `id`. The closure is temporarily moved out of the
/// arena slot during execution so signal callbacks can re-borrow the arena
/// without conflict. Restored on completion.
fn run_effect(id: EffectId) {
    // Re-entry guard. If a signal write *inside* this effect's body
    // fires the effect's own subscribers, the same id will be in the
    // about-to-run list. Running it now would call
    // `clear_effect_dependencies(id)`, wiping the dep set the outer
    // run had partially recorded — and since the inner run executes
    // through the no-op stub installed below, it never re-records
    // them. The outer run resumes with no subscriptions and will
    // never fire again on future signal changes.
    //
    // The fix: skip the re-entrant invocation entirely. The outer
    // run is already executing; it will pick up whatever fresh value
    // the signal write produced on its next `.get()`. This matches
    // how Solid / Reactively / MobX handle the same pattern (a
    // self-writing effect doesn't loop on itself).
    let reenters = RUNNING.with(|r| r.borrow().contains(&id));
    if reenters {
        return;
    }

    // Transitive-depth guard. Different-id reentry is legitimate (effect
    // A's write triggers effect B, which reads other signals), so the
    // same-id `RUNNING` set above doesn't catch mutual loops. Count the
    // nesting depth here and panic loudly above a threshold so an
    // unintentional A↔B cycle produces a useful error instead of a stack
    // overflow.
    let (_depth_guard, depth) = DepthGuard::enter();
    // Effect bodies mutate the arena (dep recording, signal writes); a
    // deferred scope-anchored callback dispatched during this window
    // must skip rather than re-enter. See `is_reactive_busy`.
    let _busy = ReactiveBusyGuard::enter();
    if depth > MAX_EFFECT_DEPTH {
        panic!(
            "effect run depth exceeded {} — likely a mutual signal/effect cycle. \
             Check for two or more effects that read and write each other's signals.",
            MAX_EFFECT_DEPTH
        );
    }

    // Take the closure out AND clone the owning-scope snapshot AND
    // drain cleanups AND read the `stable_deps` flag under a single
    // arena borrow. Folding these into one borrow saves three
    // RefCell + ARENA round-trips per fire — material at fan-outs
    // of thousands. `prev_cleanups` is typically empty for both
    // general and stable-deps effects (most effects don't register
    // `on_cleanup`); taking it out via `mem::take` is a no-op
    // memory move when empty.
    let (mut run_fn, owning_stack, prev_cleanups, stable_deps): (
        Option<Box<dyn FnMut()>>,
        Vec<*mut Scope>,
        Vec<Box<dyn FnOnce()>>,
        bool,
    ) = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let Some(Some(slot)) = a.effects.get_mut(id.0 as usize) else {
            return (None, Vec::new(), Vec::new(), false);
        };
        let Some(inner) = slot.downcast_mut::<EffectInner>() else {
            return (None, Vec::new(), Vec::new(), false);
        };
        // `take()` leaves `inner.run = None` for the duration of
        // the run — re-entry is already short-circuited by the
        // RUNNING check above, so no path observes the None.
        let run = inner.run.take();
        let stack = inner.owning_stack.clone();
        let cleanups = std::mem::take(&mut inner.cleanups);
        let stable = inner.stable_deps;
        (run, stack, cleanups, stable)
    });

    // Fire any cleanup callbacks registered during the previous
    // run before recording fresh deps. They run in LIFO order to
    // mirror typical resource-acquisition order. Outside the
    // arena borrow so callbacks can re-borrow it.
    for cb in prev_cleanups.into_iter().rev() {
        cb();
    }

    // Drop any subscriptions recorded by the previous run before we
    // collect this run's set. Skip for `stable_deps` effects: the
    // caller has asserted the dep set is identical across re-runs,
    // so clearing and re-inserting against (in the worst case)
    // a 2 k-entry subscriber HashSet on every fire is pure waste.
    // Without `stable_deps`, a re-run that reads a *different* set
    // of signals would leave stale `eid` entries in the no-longer-
    // read signals' subscriber sets — they'd be cleaned up at
    // effect drop, but in the meantime the signal would re-fire an
    // effect that doesn't care about it.
    if !stable_deps {
        clear_effect_dependencies(id);
    }

    if let Some(f) = run_fn.as_mut() {
        RUNNING.with(|r| {
            r.borrow_mut().insert(id);
        });
        // Restore the owner chain so `inject` etc. walk the scopes
        // active when this effect was created — not whatever scopes
        // happen to be on the stack when the triggering signal write
        // fired. Reversed by the matching pop below.
        let pushed = owning_stack.len();
        if pushed > 0 {
            ACTIVE_SCOPE.with(|s| s.borrow_mut().extend_from_slice(&owning_stack));
        }
        // For `stable_deps` effects, set CURRENT to None for the
        // duration of f() so `signal.get` inside the body doesn't
        // re-insert into the subscriber HashSet (the eid is already
        // there from the initial-tracking run). For the general
        // path, set CURRENT to `Some(id)` so reads track normally.
        let prev = if stable_deps {
            CURRENT.with(|c| c.replace(None))
        } else {
            CURRENT.with(|c| c.replace(Some(id)))
        };
        // Reactive-profile: time the closure body only (compute + backend
        // mutation calls). A nested flush this body triggers records its own
        // effects/commit BEFORE this `record_effect_run`, so `us` here is
        // inclusive of the nested work — the report's nested record gives the
        // finer split. Not timing `Signal::get` individually is deliberate
        // (it fires 10k+×/flush; the timer reads would swamp the measurement).
        #[cfg(feature = "debug-stats")]
        let _run_start = crate::debug::now_micros();
        f();
        #[cfg(feature = "debug-stats")]
        crate::debug::record_effect_run(
            id.0,
            crate::debug::now_micros().saturating_sub(_run_start),
        );
        CURRENT.with(|c| *c.borrow_mut() = prev);
        if pushed > 0 {
            ACTIVE_SCOPE.with(|s| {
                let mut s = s.borrow_mut();
                let new_len = s.len() - pushed;
                s.truncate(new_len);
            });
        }
        RUNNING.with(|r| {
            r.borrow_mut().remove(&id);
        });
        // Restore the actual function. If the slot has been freed during
        // the run (effect disposed by its own action), do nothing.
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(slot)) = a.effects.get_mut(id.0 as usize) {
                if let Some(inner) = slot.downcast_mut::<EffectInner>() {
                    inner.run = run_fn.take();
                }
            }
        });

    }
}

fn run_effects(ids: &[EffectId]) {
    // Hold ONE busy guard across the whole fan-out. Each `run_effect` enters
    // its own inner guard, but `REACTIVE_BUSY` must not drop to 0 *between*
    // sibling effects: the reactive-idle hook (a backend's synchronous layout
    // flush) fires at depth→0, and one `Signal::set` that wakes N subscriber
    // effects is a SINGLE mutation window, not N. Without this outer guard,
    // selecting a row in a list where all N rows subscribe to the selection
    // signal would re-run N style effects, each bouncing depth to 0 and each
    // triggering a full layout pass — O(N²) per selection (the regression this
    // guards against). With it, the hook fires once, after the last effect, and
    // the coalesced pass runs exactly once. The guard's Drop runs the hook.
    let _busy = ReactiveBusyGuard::enter();
    for &id in ids {
        // Skip freed effects gracefully.
        let alive = ARENA.with(|a| {
            a.borrow()
                .effects
                .get(id.0 as usize)
                .and_then(|o| o.as_ref())
                .is_some()
        });
        if alive {
            run_effect(id);
        }
    }
    // Reactive-profile: the backend's idle hook (a synchronous layout/paint
    // pass) fires from `_busy`'s Drop iff this drop closes the OUTERMOST
    // window (`REACTIVE_BUSY == 1` right now → drop takes it to 0). Time that
    // drop and attribute the cost to the enclosing transaction as its
    // `Commit`. Only outermost so we don't emit ~0µs commits for nested
    // flushes (and initial effect runs, which never come through here). The
    // non-debug path just lets `_busy` drop at scope end, unchanged.
    #[cfg(feature = "debug-stats")]
    {
        if REACTIVE_BUSY.with(|c| c.get()) == 1 {
            let commit_start = crate::debug::now_micros();
            drop(_busy);
            crate::debug::record_commit(crate::debug::now_micros().saturating_sub(commit_start));
        }
    }
}

// =============================================================================
// Ref<H>
// =============================================================================

/// A copy-handle pointing at an arena slot that holds an `H` once a
/// component has mounted. The parent of a component owns the `Ref<H>`
/// (typically inside its own reactive scope); the child component's
/// mount path calls [`Ref::fill`] to populate the slot, and unmount
/// calls [`Ref::clear`]. Reading via [`Ref::with`] returns `None` if
/// the slot has not been filled yet — pre-mount calls are silently
/// skipped, the same way `ref.current` is `null` in React before mount.
///
/// `Ref<H>` is `Copy`, so it can be captured into multiple closures
/// without explicit `.clone()` calls — matching `Signal<T>`'s ergonomics.
/// The slot itself is owned by the active `Scope` at creation time, so
/// it's freed deterministically when the surrounding `Owner` (or
/// `when()` branch scope) drops.
pub struct Ref<H> {
    id: RefId,
    _phantom: PhantomData<H>,
}

impl<H> Copy for Ref<H> {}
impl<H> Clone for Ref<H> {
    fn clone(&self) -> Self { *self }
}

impl<H> Default for Ref<H> {
    /// A *detached* ref: a sentinel id that aliases no arena slot, so it
    /// allocates nothing (unlike [`Ref::new`]). This lets a component with
    /// a required `Ref` prop derive `Default` for `ui!` dispatch (whose
    /// `..Default::default()` base is evaluated every render). `fill`/`clear`
    /// are no-ops on it; the real ref supplied as a prop overwrites it
    /// before mount, which is the normal path.
    fn default() -> Self {
        Self { id: RefId(u32::MAX), _phantom: PhantomData }
    }
}

impl<H: 'static> Ref<H> {
    /// Allocates a fresh ref slot. The slot's lifetime is bound to the
    /// active `Scope` (set by `render()` or by a `when()` rebuild). If
    /// no scope is active, the slot leaks until the thread exits — same
    /// rules as `Signal::new`.
    pub fn new() -> Self {
        let id = ARENA.with(|a| a.borrow_mut().insert_ref());
        register_ref(id);
        Self { id, _phantom: PhantomData }
    }

    /// Populates the slot with `handle`. The framework's mount path
    /// calls this; user code typically does not. Overwrite is legal
    /// (a `when()` rebuild may remount a component bearing the same
    /// ref) and replaces the previous handle cleanly.
    pub fn fill(&self, handle: H) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(inner)) = a.refs.get_mut(self.id.0 as usize) {
                *inner = Some(Box::new(handle));
            }
        });
    }

    /// Clears the slot, leaving the ref un-mounted. Called by the
    /// framework when the component bearing this ref unmounts (e.g.
    /// because a `when()` branch flipped away from it).
    pub fn clear(&self) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(inner)) = a.refs.get_mut(self.id.0 as usize) {
                *inner = None;
            }
        });
    }

    /// Runs `f` against the filled handle, if any. Returns `None` if
    /// the component hasn't mounted yet (or has been torn down).
    ///
    /// The handle is held by `&` inside `f`, so methods on `H` must
    /// take `&self`. Since handles mutate via Signals (which use
    /// interior mutability) or via backend dispatch, this restriction
    /// is what we want anyway.
    ///
    /// Most call sites should prefer [`Ref::get`] — same semantics but
    /// returns an owned `Option<H>`, so chaining reads like
    /// `r.get().map(|h| h.foo())` without the explicit closure.
    /// `with` is the right tool only when you specifically need to
    /// avoid cloning the handle (e.g. inside a hot path).
    pub fn with<R>(&self, f: impl FnOnce(&H) -> R) -> Option<R> {
        ARENA.with(|arena| {
            let arena = arena.borrow();
            let slot = arena.refs.get(self.id.0 as usize)?.as_ref()?;
            let inner = slot.as_ref()?;
            let handle = inner.downcast_ref::<H>()
                .expect("internal: ref handle type mismatch");
            Some(f(handle))
        })
    }

    /// True if the slot has been filled and not subsequently cleared.
    pub fn is_mounted(&self) -> bool {
        ARENA.with(|arena| {
            arena.borrow()
                .refs
                .get(self.id.0 as usize)
                .and_then(|s| s.as_ref())
                .map(|inner| inner.is_some())
                .unwrap_or(false)
        })
    }
}

impl<H: Clone + 'static> Ref<H> {
    /// Returns an owned clone of the filled handle, or `None` if the
    /// component hasn't mounted yet (or has been torn down).
    ///
    /// Cheap: handle types are designed so `Clone` is at most an `Rc`
    /// bump plus copying small pointers. The owned clone lets call
    /// sites read naturally:
    ///
    /// ```ignore
    /// pad_plus_ref.get().map(|h| h.click());
    /// // or
    /// if let Some(h) = pad_plus_ref.get() { h.click(); }
    /// ```
    ///
    /// Pre-mount calls return `None` — matching React's
    /// `ref.current === null` semantics but without nullable-by-default.
    pub fn get(&self) -> Option<H> {
        ARENA.with(|arena| {
            let arena = arena.borrow();
            let slot = arena.refs.get(self.id.0 as usize)?.as_ref()?;
            let inner = slot.as_ref()?;
            let handle = inner.downcast_ref::<H>()
                .expect("internal: ref handle type mismatch");
            Some(handle.clone())
        })
    }
}

// =============================================================================
// Scope
// =============================================================================

/// Lifetime container for arena slots created within it. Drop the scope
/// to free its signals, effects, and refs.
///
/// Scopes are typically owned by the renderer's `Owner` or by a reactive
/// subtree (e.g. inside a `when()`). User code rarely constructs scopes
/// directly — instead, signals/effects/refs created in a render call
/// register themselves with the active scope via the thread-local
/// ACTIVE_SCOPE.
pub(crate) struct Scope {
    signals: Vec<SignalId>,
    effects: Vec<EffectId>,
    refs: Vec<RefId>,
    /// Callbacks registered via `on_cleanup` from inside the scope
    /// but outside any active effect. Fired (LIFO) at the very top of
    /// `Scope::drop`, before signals/effects/refs/guards are torn
    /// down, so a callback can still read or write into the scope's
    /// reactive state.
    pub(crate) cleanups: Vec<Box<dyn FnOnce()>>,
    /// Ambient context values provided via `provide(value)`, keyed by
    /// the value's Rust type. Descendant scopes inherit lookups via
    /// `inject<T>` walking the active scope stack. Stored as a `Vec`
    /// rather than a `HashMap` because typical scopes provide 0–3
    /// values and linear search wins at that size — also lets `provide`
    /// push without rehashing.
    pub(crate) contexts: Vec<(std::any::TypeId, Box<dyn Any>)>,
    /// Boxed RAII guards adopted by the scope. Used by the
    /// static-style path so a styled node can register a cleanup
    /// (cohort unregister + backend on_node_unstyled) without
    /// allocating an `Effect` slot per node — a 10k-row scope keeps
    /// 10k guards in a tight `Vec<Box<dyn Drop>>` instead of 10k
    /// arena effect slots + 10k subscriber-set entries.
    guards: Vec<Box<dyn Any>>,
}

impl Scope {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self {
            signals: Vec::new(),
            effects: Vec::new(),
            refs: Vec::new(),
            cleanups: Vec::new(),
            contexts: Vec::new(),
            guards: Vec::new(),
        }
    }

    /// Adopt an arbitrary RAII guard into the scope. The guard's
    /// `Drop` impl fires when the scope drops, in the same batch as
    /// the effect/signal drops. Used by `attach_style_static` to
    /// hold a `StyleHandle` without allocating an Effect.
    pub(crate) fn adopt_guard<G: 'static>(&mut self, guard: G) {
        self.guards.push(Box::new(guard));
    }

    /// Adopts the given effect into this scope. The original `Effect`
    /// handle has its `owns` flag cleared so drop becomes a no-op; the
    /// scope is now responsible for freeing the slot. Reserved for the
    /// future integration where the renderer's `Owner` directly wraps a
    /// `Scope` instead of a `Vec<Effect>`.
    #[allow(dead_code)]
    pub(crate) fn adopt_effect(&mut self, mut e: Effect) {
        self.effects.push(e.id);
        e.owns = false;
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        // Thread-death guard: if the arena TLS is already destroyed,
        // this Scope is being dropped FROM the arena's own TLS
        // destructor — a leaked-at-exit scope reached transitively
        // through a still-registered effect's captures (e.g. a NESTED
        // navigator's keepalive effect whose owning screen scope was
        // caught in a backend↔handler `Rc` cycle). Running author
        // cleanups or arena bookkeeping here would re-enter destroyed
        // TLS and hard-abort the process ("cannot access a TLS value
        // during destruction", non-unwinding panic in a dtor). The
        // thread is exiting: semantic teardown is moot, so drop the raw
        // storage without running anything. Runtime drops (arena alive)
        // take the full path below unchanged. Regression: mock-backend
        // `nested_teardown_repro`.
        if ARENA.try_with(|_| ()).is_err() {
            self.cleanups.clear();
            self.signals.clear();
            self.effects.clear();
            self.refs.clear();
            self.guards.clear();
            return;
        }
        // Fire scope-level cleanups first, while every signal/effect
        // owned by this scope is still live — the callbacks may
        // legitimately read or write into them. Same reason effects
        // drain before signals later in this function: cleanup work
        // gets to assume the scope's reactive state still exists.
        // Effect-level cleanups fire separately, from EffectInner's
        // own Drop impl during the effect-drain below.
        let scope_cleanups: Vec<Box<dyn FnOnce()>> = self.cleanups.drain(..).collect();
        for cb in scope_cleanups.into_iter().rev() {
            cb();
        }

        // Take each slot's contents out under the ARENA borrow, then
        // drop them after releasing the borrow. The contents of an
        // EffectInner can transitively own *nested* Scopes (via
        // `Rc<RefCell<Option<Box<Scope>>>>` captured by an inner
        // `when`/`switch` effect closure). Those nested Scopes' Drop
        // also re-enters ARENA — and would panic "RefCell already
        // borrowed" if we drop them while still holding our own
        // borrow.
        //
        // Signals/refs follow the same pattern for symmetry, even
        // though in practice their stored values rarely own Scopes.
        // Drain owned ids into local Vecs first so we can pass slices
        // to the batched takers — they need to iterate twice (once to
        // dedupe deps, once to take slots) and can't borrow `self.*`
        // through the ARENA closure.
        let signal_ids: Vec<SignalId> = self.signals.drain(..).collect();
        let effect_ids: Vec<EffectId> = self.effects.drain(..).collect();
        let ref_ids: Vec<RefId> = self.refs.drain(..).collect();
        let guards: Vec<Box<dyn Any>> = self.guards.drain(..).collect();

        let mut taken_signals: Vec<Box<dyn Any>> = Vec::new();
        let mut taken_effects: Vec<Box<dyn Any>> = Vec::new();
        let mut taken_refs: Vec<Option<Box<dyn Any>>> = Vec::with_capacity(ref_ids.len());

        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            // Batched takers collapse the per-effect `unsubscribe`
            // hits — at 10k rows on one branch, all effects share
            // the same `theme` dep, so this turns ~10k
            // `HashSet::remove` calls into one `retain`. Same idea
            // for signals on the symmetric path.
            taken_signals = a.take_signals_batched(&signal_ids);
            taken_effects = a.take_effects_batched(&effect_ids);
            for id in ref_ids {
                if let Some(inner) = a.take_ref(id) {
                    taken_refs.push(inner);
                }
            }
        });

        // Borrow released; safe to drop the captured contents now.
        //
        // Drop order matters: **effects first, signals second**.
        // Backend cleanup hooks (`release_virtualizer`,
        // `release_graphics`, etc.) run from inside an
        // EffectInner's drop — they tear down JS-side listeners
        // and drop the wasm-bindgen Closures that JS was holding.
        // During that teardown, a queued browser event (scroll,
        // ResizeObserver, microtask-deferred refresh) can fire
        // synchronously into a Rust callback that reads a user
        // signal. If we'd already dropped the signal, that read
        // panics with "signal used after its scope was dropped".
        //
        // By draining effects first, every cleanup hook runs
        // while the surrounding scope's signals are still live.
        // Once all effects are gone, no Rust code holds a
        // `Signal<T>` reference into this scope — the framework's
        // own `data_changed` effect that captured `data` is
        // among the effects we just dropped — so the signal drop
        // is now harmless.
        // Heavy boxes (effect closures, scope guards holding
        // `StyleHandle`s) are routed through the backend-installable
        // `DROP_DEFERRAL` policy. The web backend installs a policy
        // that parks them on an rAF-sliced drain so teardown cost
        // lands outside the synchronous `apply` window — the
        // framework-purity refactor that removed the wasm-only
        // `PENDING_DROPS` thread-local + scheduler from here. Native
        // backends never install a policy; `defer_or_drop` falls
        // through to a synchronous `drop`, which is the right choice
        // when teardown is cheap.
        //
        // Signals and refs stay synchronous unconditionally — they
        // don't hold JS-side closures, and any deferred drain
        // touching effect closures may legitimately need to read
        // them.
        defer_or_drop(taken_effects);
        defer_or_drop(guards);

        drop(taken_signals);
        drop(taken_refs);
    }
}

// =============================================================================
// Active-scope registration
// =============================================================================

thread_local! {
    /// The currently-active scope, if any. `Signal::new` and `Effect::new`
    /// register their IDs here so the scope can free them on drop.
    static ACTIVE_SCOPE: RefCell<Vec<*mut Scope>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` with `scope` as the active scope. While active, any signals or
/// effects created inside `f` are registered to `scope`. The scope is
/// removed from the active list after `f` returns.
pub(crate) fn with_scope<R>(scope: &mut Scope, f: impl FnOnce() -> R) -> R {
    let ptr = scope as *mut Scope;
    ACTIVE_SCOPE.with(|s| s.borrow_mut().push(ptr));
    let result = f();
    ACTIVE_SCOPE.with(|s| {
        let last = s.borrow_mut().pop();
        debug_assert_eq!(last, Some(ptr), "scope stack imbalance");
    });
    result
}

/// Registers a signal ID with the topmost active scope, if any. Returns
/// true if a scope took ownership.
fn register_signal(id: SignalId) -> bool {
    ACTIVE_SCOPE.with(|s| {
        if let Some(&top) = s.borrow().last() {
            // SAFETY: ACTIVE_SCOPE only holds pointers to Scope values that
            // are currently borrowed by `with_scope`. The borrow extends for
            // the entire `f()` call, during which `register_signal` is the
            // only path that touches the pointer, and only mutably for a
            // brief push to the Vec. No aliasing.
            unsafe { (*top).signals.push(id); }
            true
        } else {
            false
        }
    })
}

/// Registers an effect ID with the topmost active scope. Returns true if
/// a scope took ownership.
fn register_effect(id: EffectId) -> bool {
    ACTIVE_SCOPE.with(|s| {
        if let Some(&top) = s.borrow().last() {
            unsafe { (*top).effects.push(id); }
            true
        } else {
            false
        }
    })
}

/// Registers a ref ID with the topmost active scope. Returns true if a
/// scope took ownership.
fn register_ref(id: RefId) -> bool {
    ACTIVE_SCOPE.with(|s| {
        if let Some(&top) = s.borrow().last() {
            unsafe { (*top).refs.push(id); }
            true
        } else {
            false
        }
    })
}

/// Snapshot of the reactive registration context — active-scope
/// stack plus current Effect. Used by the deferred-scheduling helpers
/// ([`crate::after_ms_scoped`], [`crate::raf_loop_scoped`]) so a
/// callback that fires later can re-enter the scope/effect it was
/// registered under — otherwise nested `*_scoped` calls inside the
/// callback see an empty stack with no CURRENT effect, their
/// `on_cleanup`-anchored handles drop instantly, and the inner
/// timer/loop is cancelled before it can fire even once.
///
/// Pair with [`with_reactive_ctx`] to re-enter + auto-restore.
///
/// Safety: the returned scope pointers are only valid for as long as
/// the originating `with_scope` / Effect frame keeps the Scope alive.
/// The scope-anchored helpers register an `on_cleanup` against that
/// same context, which guarantees the deferred callback is cancelled
/// before the Scope/Effect drops — so by the time the callback fires,
/// the pointers are still pointing at live storage.
pub(crate) struct ReactiveCtx {
    owning_stack: Vec<*mut Scope>,
    current_eid: Option<EffectId>,
}

pub(crate) fn capture_reactive_ctx() -> ReactiveCtx {
    ReactiveCtx {
        owning_stack: ACTIVE_SCOPE.with(|s| s.borrow().clone()),
        current_eid: CURRENT.with(|c| *c.borrow()),
    }
}

/// Re-enter a captured reactive context for the duration of `f`.
/// Mirrors the way [`Effect`] re-runs restore their `owning_stack` +
/// CURRENT pointer.
pub(crate) fn with_reactive_ctx<R>(ctx: &ReactiveCtx, f: impl FnOnce() -> R) -> R {
    let pushed = ctx.owning_stack.len();
    if pushed > 0 {
        ACTIVE_SCOPE.with(|s| s.borrow_mut().extend_from_slice(&ctx.owning_stack));
    }
    let prev_eid = CURRENT.with(|c| c.replace(ctx.current_eid));
    let result = f();
    CURRENT.with(|c| *c.borrow_mut() = prev_eid);
    if pushed > 0 {
        ACTIVE_SCOPE.with(|s| {
            let mut s = s.borrow_mut();
            let new_len = s.len() - pushed;
            s.truncate(new_len);
        });
    }
    result
}

/// Hands a guard to the topmost active scope. Used by the
/// static-style path so a styled node can attach its
/// `on_node_unstyled` + cohort-unregister cleanup without burning
/// an arena effect slot. Returns `true` if a scope adopted the
/// guard; `false` if there's no active scope, in which case the
/// caller is responsible for holding the guard themselves (or
/// dropping it immediately, which is fine for `StyleHandle` since
/// the apply work already happened inline).
pub(crate) fn adopt_guard_into_active_scope<G: 'static>(guard: G) -> bool {
    ACTIVE_SCOPE.with(|s| {
        if let Some(&top) = s.borrow().last() {
            unsafe { (*top).adopt_guard(guard); }
            true
        } else {
            false
        }
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests;
