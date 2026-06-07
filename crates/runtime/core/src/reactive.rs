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
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

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
    static RUNNING: RefCell<HashSet<EffectId>> = RefCell::new(HashSet::new());

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

    /// When `Some`, signal writes append their subscriber ids to this
    /// queue instead of running them inline. Drained at the end of the
    /// outermost `batch(..)` call. `None` outside any batch — writes
    /// fan out synchronously as before.
    ///
    /// Nested `batch(..)` calls reuse the outer queue: only the
    /// outermost batch flushes. This keeps "set a, then set b" inside a
    /// nested batch from running effects between the two writes when
    /// the outer batch hasn't completed yet.
    static BATCH_PENDING: RefCell<Option<Vec<EffectId>>> = const { RefCell::new(None) };

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
        REACTIVE_BUSY.with(|c| c.set(c.get().saturating_sub(1)));
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
    signal_subscribers: Vec<HashSet<EffectId>>,

    /// Per-effect dependency set, indexed parallel to `effects`. An
    /// entry `sid` here means "this effect's last run read signal
    /// `sid`". Cleared at the start of every re-run so the dep set
    /// reflects the *latest* run, not the union of all runs (matches
    /// what every fine-grained reactivity lib does — Solid, Reactively,
    /// MobX). Drained on effect-free so dead `EffectId`s don't sit in
    /// any signal's subscriber set.
    effect_dependencies: Vec<HashSet<SignalId>>,

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
    signal_js_notifiers: HashMap<u64, std::rc::Rc<dyn Fn()>>,

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
            signal_js_notifiers: HashMap::new(),
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
            self.signal_subscribers.push(HashSet::new());
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
            self.effect_dependencies.push(HashSet::new());
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
        let mut dead: HashSet<EffectId> = HashSet::with_capacity(ids.len());
        let mut affected: HashSet<SignalId> = HashSet::new();
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
/// let first = signal!("Jane".to_string());
/// let last = signal!("Doe".to_string());
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
        // Untracked read so a `dispatch` call from inside an effect
        // doesn't subscribe that effect to `state` (it's the
        // dispatcher's job to *cause* state changes, not to react
        // to them).
        let current = untrack(|| state.get());
        let next = f(&current, action);
        state.set(next);
    };
    (state, dispatch)
}

/// A cached derived signal: recomputes `f` whenever a signal it reads
/// changes, and notifies subscribers only when the new value differs
/// from the old (`T: PartialEq`). Use it for derived state that's read
/// in several places or is expensive to compute — the work runs once
/// per dependency change, not once per read. For a value without
/// `PartialEq`, or a custom "close enough" comparison, use
/// [`memo_with`]. The `memo!` macro is the terse call-site form.
pub fn memo<T>(f: impl Fn() -> T + 'static) -> Signal<T>
where
    T: Clone + PartialEq + 'static,
{
    memo_with(|a, b| a == b, f)
}

/// Like [`memo`] but with a caller-supplied equality function. Use this
/// for types that don't impl `PartialEq` (e.g. when `T` contains a
/// trait object) or when "equal enough to skip notification" doesn't
/// match `PartialEq` (e.g. tolerance-based float comparison).
pub fn memo_with<T, F, E>(eq: E, f: F) -> Signal<T>
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

    signal
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
    let prev = CURRENT.with(|c| c.borrow_mut().take());
    let result = f();
    CURRENT.with(|c| *c.borrow_mut() = prev);
    result
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
    /// `bind!` macro and the Roku backend to wire reactive bindings:
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
    pub fn new(value: T) -> Self {
        let (id, gen) = ARENA.with(|a| {
            a.borrow_mut().insert_signal(SignalInner { value })
        });
        register_signal(id);
        Self { id, gen, _phantom: PhantomData }
    }

    pub fn get(&self) -> T {
        let sid = self.id;
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
        // Subscriber lists are kept tight on the cleanup side (effect
        // drop / effect re-run), so no pruning pass needed here.
        let to_run = collect_subscribers(self.id);
        notify_or_queue(&to_run);
        // Fire any JS-side notifier registered for this signal.
        // No-op when no notifier exists (the common case) — single
        // HashMap lookup, ~10 ns.
        notify_js_subscriber(self.id);
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
        let to_run = collect_subscribers(self.id);
        notify_or_queue(&to_run);
        notify_js_subscriber(self.id);
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

/// Either runs the listed effects immediately (no batch active) or
/// appends them to the current batch's pending queue (batch active —
/// outermost batch drains and runs them when it returns). Called from
/// `Signal::set` / `Signal::update` instead of `run_effects` directly so
/// every signal write participates in batching automatically.
fn notify_or_queue(ids: &[EffectId]) {
    let batched = BATCH_PENDING.with(|b| {
        let mut b = b.borrow_mut();
        if let Some(pending) = b.as_mut() {
            pending.extend_from_slice(ids);
            true
        } else {
            false
        }
    });
    if !batched {
        run_effects(ids);
    }
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
    // Only the outermost batch owns the queue. Nested batches see
    // `Some(_)` already in place and skip the install — when the outer
    // returns, it drains everything written across all nested batches
    // in one pass.
    let is_outer = BATCH_PENDING.with(|b| {
        let mut b = b.borrow_mut();
        if b.is_none() {
            *b = Some(Vec::new());
            true
        } else {
            false
        }
    });

    let result = f();

    if is_outer {
        // Take the queue out and clear the slot *before* running
        // effects. An effect's body can call set() — that write should
        // see `BATCH_PENDING = None` (the batch is over) and fan out
        // synchronously, not append to a queue we're already draining.
        let mut pending = BATCH_PENDING
            .with(|b| b.borrow_mut().take())
            .unwrap_or_default();

        if !pending.is_empty() {
            // Dedupe while preserving first-seen order so the user can
            // reason about ordering (writes earliest in the batch run
            // their effects first). For typical batch sizes (a handful
            // of writes), the linear `contains` is cheaper than
            // allocating a HashSet.
            let mut ordered: Vec<EffectId> = Vec::with_capacity(pending.len());
            for eid in pending.drain(..) {
                if !ordered.contains(&eid) {
                    ordered.push(eid);
                }
            }
            run_effects(&ordered);
        }
    }

    result
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
                panic!("re-entrant mutation of signal {:?} inside its own update", id)
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
    /// Creates an effect and runs it once. Any signals read during the run
    /// re-fire the effect on change.
    ///
    /// If a `Scope` is active (via `with_scope`), the effect's slot is
    /// owned by that scope — the returned `Effect` handle's drop is a
    /// no-op and the slot is freed when the scope drops. If no scope is
    /// active, the returned handle owns the slot directly.
    pub fn new<F: FnMut() + 'static>(f: F) -> Self {
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
        let registered = register_effect(id);
        run_effect(id);
        Effect { id, owns: !registered }
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
    pub fn new_with_stable_deps<F: FnMut() + 'static>(f: F) -> Self {
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
    pub fn persist(self) {
        // `owns == false` (scope-adopted): forget drops a no-op handle.
        // `owns == true` (no scope): forget skips the cancelling Drop,
        // pinning the slot for the process lifetime. Both are exactly the
        // behaviour the prior `mem::forget(effect)` call sites relied on
        // (see `memo_in_scope_releases_signal_and_effect_on_scope_drop`).
        std::mem::forget(self);
    }
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
        f();
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
mod tests {
    use super::*;

    #[test]
    fn signal_is_copy_and_works() {
        let s = Signal::new(7i32);
        let s2 = s; // Copy: no .clone() needed.
        s.set(42);
        assert_eq!(s2.get(), 42);
    }

    #[test]
    fn effect_fires_on_change() {
        use std::cell::Cell;
        use std::rc::Rc;
        let count = Signal::new(0i32);
        let observed = Rc::new(Cell::new(0));
        let obs = observed.clone();
        let _e = Effect::new(move || {
            obs.set(count.get());
        });
        assert_eq!(observed.get(), 0);
        count.set(5);
        assert_eq!(observed.get(), 5);
        count.set(11);
        assert_eq!(observed.get(), 11);
    }

    /// `Effect::persist` outside any reactive scope must keep the effect
    /// reacting. A bare handle dropped at end-of-statement (no scope to
    /// adopt it) would cancel; `persist` pins it instead. This is the
    /// behaviour `doc_controls.rs` relies on when its controls are built
    /// ad-hoc / in tests outside a render scope.
    #[test]
    fn persist_keeps_effect_alive_outside_scope() {
        use std::cell::Cell;
        use std::rc::Rc;
        let src = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0u32));
        let runs_for_effect = runs.clone();
        Effect::new(move || {
            let _ = src.get();
            runs_for_effect.set(runs_for_effect.get() + 1);
        })
        .persist();
        assert_eq!(runs.get(), 1, "effect runs once at creation");
        src.set(1);
        assert_eq!(
            runs.get(),
            2,
            "persisted effect must re-fire on signal change (handle was not held)"
        );
    }

    /// Contrast for [`persist_keeps_effect_alive_outside_scope`]: WITHOUT
    /// `persist`, dropping the handle outside a scope cancels the effect —
    /// the exact regression `persist` (and the prior `mem::forget`) guards
    /// against.
    #[test]
    fn dropped_effect_outside_scope_does_not_refire() {
        use std::cell::Cell;
        use std::rc::Rc;
        let src = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0u32));
        let runs_for_effect = runs.clone();
        drop(Effect::new(move || {
            let _ = src.get();
            runs_for_effect.set(runs_for_effect.get() + 1);
        }));
        assert_eq!(runs.get(), 1);
        src.set(1);
        assert_eq!(runs.get(), 1, "dropped effect must not re-fire");
    }

    /// Regression test for the "self-writing effect breaks after first
    /// flip" bug. An effect that bridges two signals — reads from
    /// `value`, writes to `shadow` — used to corrupt its own
    /// subscription set on the recursive re-fire from `shadow.set`,
    /// since `run_effect` calls `clear_effect_dependencies` at the
    /// start of every (re-)entry. After fix: re-entrant invocations
    /// of the same effect are short-circuited so the outer run's
    /// dep recording isn't wiped.
    #[test]
    fn effect_with_self_write_keeps_firing() {
        use std::cell::Cell;
        use std::rc::Rc;
        let value = Signal::new(0i32);
        let shadow = Signal::new(0i32);
        let mirror_runs = Rc::new(Cell::new(0));
        let r = mirror_runs.clone();
        let _e = Effect::new(move || {
            let v = value.get();
            // Reads `shadow` AND writes it. Pre-fix, the second
            // value.set below leaves the effect dead because the
            // recursive shadow.set wiped its `value` subscription.
            if shadow.get() != v {
                shadow.set(v);
            }
            r.set(r.get() + 1);
        });
        assert_eq!(mirror_runs.get(), 1);
        assert_eq!(shadow.get(), 0);

        value.set(1);
        assert_eq!(shadow.get(), 1);
        let after_first = mirror_runs.get();
        assert!(after_first >= 2, "effect should re-run after first value.set");

        value.set(2);
        assert_eq!(
            shadow.get(),
            2,
            "shadow should track value after the second flip too"
        );
        assert!(
            mirror_runs.get() > after_first,
            "effect must fire again after the second value.set — before \
             the fix this was the broken case"
        );
    }

    // -----------------------------------------------------------------
    // Context (provide / inject)
    // -----------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq)]
    struct Theme(&'static str);

    #[derive(Clone, Debug, PartialEq)]
    struct Locale(&'static str);

    #[test]
    fn inject_returns_none_without_provider() {
        let mut scope = Scope::new();
        let result: Option<Theme> = with_scope(&mut scope, || inject::<Theme>());
        assert_eq!(result, None);
    }

    #[test]
    fn provide_then_inject_in_same_scope() {
        let mut scope = Scope::new();
        let result = with_scope(&mut scope, || {
            provide(Theme("dark"));
            inject::<Theme>()
        });
        assert_eq!(result, Some(Theme("dark")));
    }

    #[test]
    fn inject_finds_outer_provision_from_inner_scope() {
        let mut outer = Scope::new();
        let result = with_scope(&mut outer, || {
            provide(Theme("dark"));
            let mut inner = Scope::new();
            with_scope(&mut inner, || inject::<Theme>())
        });
        assert_eq!(result, Some(Theme("dark")));
    }

    #[test]
    fn inner_provision_shadows_outer() {
        let mut outer = Scope::new();
        let result = with_scope(&mut outer, || {
            provide(Theme("light"));
            let mut inner = Scope::new();
            let inner_result = with_scope(&mut inner, || {
                provide(Theme("dark"));
                inject::<Theme>()
            });
            // After inner scope drops, the inner provision is gone —
            // outer's "light" is visible again.
            let outer_after = inject::<Theme>();
            (inner_result, outer_after)
        });
        assert_eq!(result, (Some(Theme("dark")), Some(Theme("light"))));
    }

    #[test]
    fn different_types_coexist() {
        let mut scope = Scope::new();
        let (theme, locale) = with_scope(&mut scope, || {
            provide(Theme("dark"));
            provide(Locale("ja-JP"));
            (inject::<Theme>(), inject::<Locale>())
        });
        assert_eq!(theme, Some(Theme("dark")));
        assert_eq!(locale, Some(Locale("ja-JP")));
    }

    #[test]
    fn provision_dies_with_scope() {
        let mut scope = Scope::new();
        with_scope(&mut scope, || provide(Theme("dark")));
        drop(scope);
        // No active scope at all → inject returns None (also exercises
        // the no-active-scope branch in inject).
        assert_eq!(inject::<Theme>(), None);
    }

    #[test]
    fn inject_or_falls_back_to_default() {
        let mut scope = Scope::new();
        let value = with_scope(&mut scope, || inject_or(Theme("default")));
        assert_eq!(value, Theme("default"));
    }

    #[test]
    fn with_inject_reads_by_reference() {
        // Use a non-Clone type to prove `with_inject` doesn't need
        // Clone — only `inject` / `inject_or` do.
        struct NonClone(i32);
        let mut scope = Scope::new();
        let result: Option<i32> = with_scope(&mut scope, || {
            provide(NonClone(42));
            with_inject::<NonClone, _>(|v| v.0)
        });
        assert_eq!(result, Some(42));
    }

    #[test]
    fn provided_signal_is_reactive_for_descendants() {
        use std::cell::Cell;
        use std::rc::Rc;
        // The classic theme-switch pattern: provide a Signal<Theme>;
        // descendant effects subscribe by reading `.get()`.
        let mut scope = Scope::new();
        let observed = Rc::new(Cell::new(""));
        let theme_signal = with_scope(&mut scope, || {
            let theme = Signal::new("light");
            provide(theme);
            let obs = observed.clone();
            let _e = Effect::new(move || {
                let t = inject::<Signal<&'static str>>().expect("provided above");
                obs.set(t.get());
            });
            theme
        });
        assert_eq!(observed.get(), "light");
        theme_signal.set("dark");
        assert_eq!(observed.get(), "dark", "descendant must see signal updates");
    }

    #[test]
    #[should_panic(expected = "outside any active reactive scope")]
    fn provide_outside_scope_panics() {
        provide(Theme("nope"));
    }

    #[test]
    #[should_panic(expected = "memo's compute closure")]
    fn provide_inside_memo_compute_panics() {
        // `provide` is a side effect that would attach to the
        // memo-creation scope and accumulate duplicates on each
        // recompute. Same guard as `Signal::set`.
        let trigger = Signal::new(0i32);
        let _m = memo(move || {
            let _ = trigger.get();
            provide(Theme("dark")); // ← violation
            7
        });
    }

    // -----------------------------------------------------------------
    // Memo write-during-compute: hard panic
    // -----------------------------------------------------------------

    #[test]
    #[should_panic(expected = "memo's compute closure")]
    fn memo_write_during_compute_panics() {
        // A memo whose compute closure writes to a signal — the panic
        // points at the offending write, not the downstream cascade.
        let trigger = Signal::new(0i32);
        let side = Signal::new(0i32);
        let _m = memo(move || {
            let _ = trigger.get();
            side.set(42); // ← violation
            7
        });
    }

    #[test]
    #[should_panic(expected = "memo's compute closure")]
    fn memo_update_during_compute_panics() {
        // `update` goes through the same guard as `set`.
        let trigger = Signal::new(0i32);
        let side = Signal::new(0i32);
        let _m = memo(move || {
            let _ = trigger.get();
            side.update(|v| *v += 1);
            7
        });
    }

    #[test]
    fn memo_writing_to_own_output_signal_does_not_panic() {
        // Sanity: the memo's internal `signal.set(new)` (when the
        // computed value differs from `last`) must not be caught by the
        // guard. The guard scope is tight to the user's `f()` only.
        let source = Signal::new(1i32);
        let mut scope = Scope::new();
        let m = with_scope(&mut scope, || memo(move || source.get() * 2));
        assert_eq!(m.get(), 2);
        source.set(5);
        assert_eq!(m.get(), 10, "memo updates its output signal normally");
    }

    // -----------------------------------------------------------------
    // batch()
    // -----------------------------------------------------------------

    #[test]
    fn batch_coalesces_fan_out_to_one_run_per_effect() {
        use std::cell::Cell;
        use std::rc::Rc;
        let a = Signal::new(0i32);
        let b = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let _e = Effect::new(move || {
            let _ = a.get() + b.get();
            r.set(r.get() + 1);
        });
        assert_eq!(runs.get(), 1, "effect runs once on creation");

        batch(|| {
            a.set(5);
            b.set(7);
            a.set(8);
        });
        assert_eq!(
            runs.get(),
            2,
            "three writes inside a batch produce one re-run, not three"
        );
    }

    #[test]
    fn batch_nested_only_flushes_at_outermost() {
        use std::cell::Cell;
        use std::rc::Rc;
        let a = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let _e = Effect::new(move || {
            let _ = a.get();
            r.set(r.get() + 1);
        });
        assert_eq!(runs.get(), 1);

        batch(|| {
            a.set(1);
            // Inner batch must not flush — the outer should keep
            // collecting and fire `_e` exactly once at its own end.
            batch(|| {
                a.set(2);
            });
            assert_eq!(runs.get(), 1, "no flush during inner batch");
            a.set(3);
        });
        assert_eq!(runs.get(), 2, "outermost batch flushes once at exit");
    }

    #[test]
    fn batch_returns_inner_result() {
        let value = batch(|| 42);
        assert_eq!(value, 42);
    }

    // -----------------------------------------------------------------
    // Cycle / depth detection
    // -----------------------------------------------------------------

    #[test]
    #[should_panic(expected = "effect run depth exceeded")]
    fn deep_effect_chain_panics_at_depth_threshold() {
        // The same-id reentry guard already prevents an effect from
        // looping on itself, and incidentally catches small mutual
        // cycles (A↔B) because the cycle revisits an effect already on
        // the stack. The depth guard exists for cases reentry doesn't
        // cover: long synchronous *chains* of distinct effects, where
        // no single effect repeats but the cascade depth is unbounded.
        //
        // Construct N forwarding effects (read signals[i], write
        // signals[i+1]). Setting signals[0] cascades the full length;
        // past MAX_EFFECT_DEPTH (256) the depth guard panics with the
        // expected message instead of stack-overflowing.
        const N: usize = 280;
        let signals: Vec<Signal<i32>> = (0..N).map(|_| Signal::new(0i32)).collect();
        let mut effects: Vec<Effect> = Vec::with_capacity(N - 1);
        for i in 0..(N - 1) {
            let read = signals[i];
            let write = signals[i + 1];
            // Wrap each effect's first-run write so the initial fan-out
            // from setup doesn't trigger the cascade prematurely — only
            // the explicit set() below should kick it off.
            let mut first = true;
            let e = Effect::new(move || {
                let v = read.get();
                if first {
                    first = false;
                    return;
                }
                write.set(v + 1);
            });
            effects.push(e);
        }
        signals[0].set(1);
    }

    // -----------------------------------------------------------------
    // memo()
    // -----------------------------------------------------------------

    #[test]
    fn memo_caches_and_skips_equal_notifications() {
        use std::cell::Cell;
        use std::rc::Rc;
        let source = Signal::new(0i32);

        // Memo: count whether the input is over 10.
        let mut scope = Scope::new();
        let runs = Rc::new(Cell::new(0));
        let m = with_scope(&mut scope, || {
            let m = memo(move || source.get() > 10);
            let r = runs.clone();
            let _e = Effect::new(move || {
                let _ = m.get();
                r.set(r.get() + 1);
            });
            m
        });
        // Initial: subscriber ran once, memo value is `false`.
        assert_eq!(runs.get(), 1);
        assert_eq!(m.get(), false);

        // Bump source within "false" range — memo recomputes but value
        // stays `false`, so subscriber must NOT re-fire.
        source.set(5);
        assert_eq!(m.get(), false);
        assert_eq!(
            runs.get(),
            1,
            "memo gates equal results — subscriber must not re-run"
        );

        source.set(7);
        assert_eq!(runs.get(), 1, "still false → still gated");

        // Cross the threshold: memo flips, subscriber sees the change.
        source.set(11);
        assert_eq!(m.get(), true);
        assert_eq!(runs.get(), 2, "subscriber fires when memo's value actually changes");

        // Back below threshold: flips again, subscriber fires again.
        source.set(3);
        assert_eq!(m.get(), false);
        assert_eq!(runs.get(), 3);
    }

    #[test]
    fn memo_recomputes_once_per_dep_change_regardless_of_subscriber_count() {
        use std::cell::Cell;
        use std::rc::Rc;
        let source = Signal::new(1i32);
        let compute_count = Rc::new(Cell::new(0));
        let c = compute_count.clone();
        let m = memo(move || {
            c.set(c.get() + 1);
            source.get() * 2
        });
        // Three independent readers of the same memo.
        let _e1 = Effect::new(move || {
            let _ = m.get();
        });
        let _e2 = Effect::new(move || {
            let _ = m.get();
        });
        let _e3 = Effect::new(move || {
            let _ = m.get();
        });
        let after_setup = compute_count.get();

        source.set(5);
        assert_eq!(
            compute_count.get(),
            after_setup + 1,
            "memo recomputes once per dep change even when three subscribers exist"
        );
    }

    // -----------------------------------------------------------------
    // on() / on_defer()
    // -----------------------------------------------------------------

    #[test]
    fn on_passes_new_and_previous_values() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let count = Signal::new(0i32);
        let log: Rc<RefCell<Vec<(i32, Option<i32>)>>> = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        let _e = on(count, move |new, prev| {
            l.borrow_mut().push((*new, prev.copied()));
        });
        // Initial fire: prev is None.
        count.set(5);
        count.set(7);
        let recorded = log.borrow().clone();
        assert_eq!(
            recorded,
            vec![(0, None), (5, Some(0)), (7, Some(5))],
            "on() must thread (current, previous) across runs"
        );
    }

    #[test]
    fn on_tuple_subscribes_to_every_member() {
        use std::cell::Cell;
        use std::rc::Rc;
        let first = Signal::new("Jane".to_string());
        let last = Signal::new("Doe".to_string());
        let fires = Rc::new(Cell::new(0));
        let f = fires.clone();
        let _e = on((first, last), move |_new, _prev| {
            f.set(f.get() + 1);
        });
        assert_eq!(fires.get(), 1, "initial fire");
        first.set("Janet".to_string());
        assert_eq!(fires.get(), 2);
        last.set("Smith".to_string());
        assert_eq!(fires.get(), 3);
    }

    #[test]
    fn on_defer_skips_initial_run() {
        use std::cell::Cell;
        use std::rc::Rc;
        let count = Signal::new(0i32);
        let fires = Rc::new(Cell::new(0));
        let f = fires.clone();
        let _e = on_defer(count, move |_new, _prev| {
            f.set(f.get() + 1);
        });
        assert_eq!(fires.get(), 0, "on_defer must not fire on creation");
        count.set(1);
        assert_eq!(fires.get(), 1, "first change after creation fires");
        count.set(2);
        assert_eq!(fires.get(), 2);
    }

    #[test]
    fn on_body_reads_do_not_subscribe() {
        // Body reads `other` but `other` is not in the deps tuple — only
        // `trigger` should re-fire the effect.
        use std::cell::Cell;
        use std::rc::Rc;
        let trigger = Signal::new(0i32);
        let other = Signal::new(0i32);
        let fires = Rc::new(Cell::new(0));
        let f = fires.clone();
        let _e = on(trigger, move |_new, _prev| {
            let _shielded = other.get();
            f.set(f.get() + 1);
        });
        assert_eq!(fires.get(), 1, "initial");
        other.set(99);
        assert_eq!(
            fires.get(),
            1,
            "writes to a signal read inside the body but not in deps must not fire"
        );
        trigger.set(1);
        assert_eq!(fires.get(), 2, "writes to a dep do fire");
    }

    // -----------------------------------------------------------------
    // reducer()
    // -----------------------------------------------------------------

    #[test]
    fn reducer_applies_user_function_to_state() {
        enum Counter {
            Inc,
            Dec,
            Set(i32),
        }
        let (state, dispatch) = reducer(0i32, |&n, action| match action {
            Counter::Inc => n + 1,
            Counter::Dec => n - 1,
            Counter::Set(v) => v,
        });
        assert_eq!(state.get(), 0);
        dispatch(Counter::Inc);
        assert_eq!(state.get(), 1);
        dispatch(Counter::Inc);
        dispatch(Counter::Inc);
        assert_eq!(state.get(), 3);
        dispatch(Counter::Dec);
        assert_eq!(state.get(), 2);
        dispatch(Counter::Set(100));
        assert_eq!(state.get(), 100);
    }

    #[test]
    fn reducer_state_signal_notifies_subscribers() {
        use std::cell::Cell;
        use std::rc::Rc;
        let (state, dispatch) = reducer(0i32, |&n, delta: i32| n + delta);
        let observed = Rc::new(Cell::new(0i32));
        let o = observed.clone();
        let _e = Effect::new(move || {
            o.set(state.get());
        });
        assert_eq!(observed.get(), 0);
        dispatch(5);
        assert_eq!(observed.get(), 5, "subscriber sees the new state after dispatch");
        dispatch(7);
        assert_eq!(observed.get(), 12);
    }

    #[test]
    fn reducer_dispatch_does_not_subscribe_caller_effect() {
        // The dispatcher reads the current state to compute the next
        // one. That read is `untrack`ed so it doesn't accidentally
        // subscribe the surrounding effect to the reducer's state.
        // (Without that, calling `dispatch` from inside an effect
        // would make the effect re-fire on every state change it
        // caused — easy infinite-loop trap.)
        use std::cell::Cell;
        use std::rc::Rc;
        let trigger = Signal::new(0i32);
        let (state, dispatch) = reducer(0i32, |&n, _: ()| n + 1);
        let fires = Rc::new(Cell::new(0));
        let f = fires.clone();
        let _e = Effect::new(move || {
            // Effect's only declared dep is `trigger`. If `dispatch`
            // ends up subscribing us to `state`, the assertion below
            // catches it.
            let _ = trigger.get();
            f.set(f.get() + 1);
            dispatch(());
        });
        assert_eq!(fires.get(), 1, "initial run");
        let after_initial = state.get();
        assert_eq!(after_initial, 1, "state advanced once on the initial run");
        // External write to a signal we DO depend on triggers a re-run
        // and another dispatch.
        trigger.set(1);
        assert_eq!(fires.get(), 2, "re-fires on trigger");
        assert_eq!(state.get(), 2, "state advanced again");
        // Critically: no additional re-fires beyond the trigger-driven
        // one. If dispatch had subscribed us to `state`, fires would
        // be 3+ here (reentry guard would short-circuit re-entries,
        // but the count would still differ).
        assert_eq!(
            fires.get(),
            2,
            "dispatch must not subscribe caller effect to state"
        );
    }

    #[test]
    fn reducer_state_is_a_plain_signal() {
        // Sanity: the returned `state` is the same `Signal<S>` type
        // every other consumer accepts. This verifies that the
        // pattern composes without inventing a new type.
        let (state, dispatch) = reducer(0i32, |&n, a: i32| n + a);
        // Same Copy semantics as any other Signal.
        let alias: Signal<i32> = state;
        dispatch(10);
        assert_eq!(alias.get(), 10);
        // `.update` works on the same signal directly, bypassing the
        // reducer — useful escape hatch for migrations from
        // signal-based state.
        alias.update(|n| *n = -5);
        assert_eq!(state.get(), -5);
        dispatch(3);
        assert_eq!(state.get(), -2);
    }

    #[test]
    fn effect_macro_runs_and_rebinds_in_scope() {
        use std::cell::Cell;
        use std::rc::Rc;
        let count = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            crate::effect!({
                let _ = count.get();
                r.set(r.get() + 1);
            });
        });
        assert_eq!(runs.get(), 1);
        count.set(7);
        assert_eq!(runs.get(), 2, "macro-built effect should re-fire on signal change");
        // Scope drop disposes the effect.
        drop(scope);
        count.set(8);
        assert_eq!(runs.get(), 2, "effect should not fire after its scope drops");
    }

    #[test]
    fn on_cleanup_fires_before_effect_rerun() {
        use std::cell::Cell;
        use std::rc::Rc;
        let trigger = Signal::new(0i32);
        let cleanup_count = Rc::new(Cell::new(0));
        let run_count = Rc::new(Cell::new(0));
        let c = cleanup_count.clone();
        let r = run_count.clone();
        let _e = Effect::new(move || {
            let _ = trigger.get();
            r.set(r.get() + 1);
            let c2 = c.clone();
            on_cleanup(move || {
                c2.set(c2.get() + 1);
            });
        });
        // First run: 1 run, 0 cleanups so far.
        assert_eq!(run_count.get(), 1);
        assert_eq!(cleanup_count.get(), 0);

        // Re-run drains the previous cleanup and registers a new one.
        trigger.set(1);
        assert_eq!(run_count.get(), 2);
        assert_eq!(cleanup_count.get(), 1);

        trigger.set(2);
        assert_eq!(run_count.get(), 3);
        assert_eq!(cleanup_count.get(), 2);
    }

    #[test]
    fn on_cleanup_fires_on_effect_drop() {
        use std::cell::Cell;
        use std::rc::Rc;
        let cleanup_count = Rc::new(Cell::new(0));
        let c = cleanup_count.clone();
        let e = Effect::new(move || {
            let c2 = c.clone();
            on_cleanup(move || {
                c2.set(c2.get() + 1);
            });
        });
        assert_eq!(cleanup_count.get(), 0);
        drop(e);
        assert_eq!(cleanup_count.get(), 1);
    }

    #[test]
    fn on_cleanup_attaches_to_scope_outside_effect() {
        use std::cell::Cell;
        use std::rc::Rc;
        let cleanup_count = Rc::new(Cell::new(0));
        let c = cleanup_count.clone();
        let mut scope = Scope::new();
        with_scope(&mut scope, || {
            on_cleanup(move || {
                c.set(c.get() + 1);
            });
        });
        assert_eq!(cleanup_count.get(), 0);
        drop(scope);
        assert_eq!(cleanup_count.get(), 1);
    }

    #[test]
    fn on_cleanup_outside_any_context_is_noop() {
        // Just verify nothing panics. The callback is dropped silently;
        // any side effect from its destructor is the test signal.
        use std::cell::Cell;
        use std::rc::Rc;
        let dropped = Rc::new(Cell::new(false));
        let d = dropped.clone();
        on_cleanup(move || { /* unused */ });
        // The closure captures nothing observable; we just check this
        // didn't panic. For a second pass, register a closure that
        // *does* observe its drop:
        struct Witness(Rc<Cell<bool>>);
        impl Drop for Witness {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let w = Witness(d);
        on_cleanup(move || {
            let _hold = w;
        });
        // No context → callback dropped synchronously → Witness drops now.
        assert!(dropped.get());
    }

    #[test]
    fn untrack_blocks_subscription() {
        use std::cell::Cell;
        use std::rc::Rc;
        let s = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let _e = Effect::new(move || {
            untrack(|| {
                let _ = s.get();
            });
            r.set(r.get() + 1);
        });
        assert_eq!(runs.get(), 1);
        s.set(99); // should NOT re-fire effect
        assert_eq!(runs.get(), 1);
    }

    /// Returns (signals_in_use, effects_in_use) — counts of `Some` slots in
    /// the arena. Used by leak tests.
    fn arena_inuse_counts() -> (usize, usize) {
        ARENA.with(|a| {
            let a = a.borrow();
            (
                a.signals.iter().filter(|s| s.is_some()).count(),
                a.effects.iter().filter(|e| e.is_some()).count(),
            )
        })
    }

    #[test]
    fn scope_frees_signals_and_effects_on_drop() {
        let (s0, e0) = arena_inuse_counts();
        {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                let _a = Signal::new(1i32);
                let _b = Signal::new(2i32);
                let _e = Effect::new(|| {});
                let (s1, e1) = arena_inuse_counts();
                assert_eq!(s1, s0 + 2, "two new signal slots in use inside scope");
                assert_eq!(e1, e0 + 1, "one new effect slot in use inside scope");
            });
            // Scope still alive (just not active). Slots still in use.
            let (s_active, e_active) = arena_inuse_counts();
            assert_eq!(s_active, s0 + 2);
            assert_eq!(e_active, e0 + 1);
            // Scope drops here.
        }
        let (s_after, e_after) = arena_inuse_counts();
        assert_eq!(s_after, s0, "all signal slots returned to baseline");
        assert_eq!(e_after, e0, "all effect slots returned to baseline");
    }

    /// Regression: a write through a STALE signal handle — one whose
    /// owning scope unmounted and whose slot was recycled by a
    /// different-typed signal — must be a safe no-op, NOT a
    /// "signal type mismatch" panic. That panic, fired from a deferred
    /// `signal.set` inside a JNI scheduled callback, aborted the whole
    /// Android app (SIGABRT, non-unwinding FFI boundary). Generational
    /// handles make the stale write detect the bumped generation and do
    /// nothing. ARENA is thread-local, so this test thread's arena
    /// starts empty and the freed slot is the one `fresh` recycles.
    #[test]
    fn stale_signal_write_after_scope_drop_is_noop_not_panic() {
        let mut scope = Scope::new();
        let stale: Signal<bool> = with_scope(&mut scope, || Signal::new(false));
        drop(scope); // frees `stale`'s slot and bumps its generation

        // Recycle the just-freed slot with a DIFFERENT-typed signal —
        // the exact aliasing that used to make the stale write panic.
        let fresh: Signal<u64> = Signal::new(7);
        assert_eq!(
            fresh.id(),
            stale.id(),
            "fresh signal should reuse the freed slot (LIFO freelist)"
        );

        // The crash repro: deferred write through the stale handle.
        stale.set(true); // must NOT panic
        assert_eq!(
            fresh.get(),
            7,
            "stale write must not clobber the recycled signal"
        );

        // A stale `update` is likewise a no-op.
        stale.update(|v| *v = true);
        assert_eq!(fresh.get(), 7);

        // The recycled signal still works normally afterward.
        fresh.set(9);
        assert_eq!(fresh.get(), 9);
    }

    /// A stale write must not fire the recycled occupant's subscribers
    /// either — otherwise a disposed signal's deferred `set` could
    /// spuriously re-run effects subscribed to whatever took its slot.
    #[test]
    fn stale_signal_write_does_not_fire_recycled_subscribers() {
        use std::cell::Cell;
        use std::rc::Rc;

        let mut scope = Scope::new();
        let stale: Signal<bool> = with_scope(&mut scope, || Signal::new(false));
        drop(scope);

        let fresh: Signal<u64> = Signal::new(0);
        assert_eq!(fresh.id(), stale.id());

        // Subscribe an effect to the recycled signal.
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let _e = Effect::new(move || {
            let _ = fresh.get();
            r.set(r.get() + 1);
        });
        assert_eq!(runs.get(), 1, "effect runs once on creation");

        // Stale write to the same slot index must NOT re-run the effect.
        stale.set(true);
        assert_eq!(
            runs.get(),
            1,
            "stale write fired the recycled signal's subscribers"
        );

        // A real write to the recycled signal still re-runs it.
        fresh.set(1);
        assert_eq!(runs.get(), 2);
    }

    #[test]
    fn freelist_recycles_slot_ids_across_scopes() {
        // Repeatedly mount-then-drop a scope holding N signals + N
        // effects. Without the freelist, `arena_stats().effects_total`
        // would grow by N per iteration; with the freelist, it should
        // stay roughly bounded by the largest concurrent scope size.
        const N: usize = 64;
        let stats_before = super::arena_stats();
        for _ in 0..5 {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                for _ in 0..N {
                    let _ = Signal::new(0_i32);
                    let _ = Effect::new(|| {});
                }
            });
            // scope drops, ids recycle to the freelist
        }
        let stats_after = super::arena_stats();
        // Without recycling we'd see signals_total/effects_total grow
        // by ~5N. With recycling, growth is bounded by N (one cohort's
        // worth — the first iteration fills fresh ids, later iterations
        // pop them off the freelist).
        let growth = stats_after.effects_total - stats_before.effects_total;
        assert!(
            growth <= N + 2,
            "effects_total grew by {} (expected ≤ {} with freelist recycling)",
            growth,
            N + 2,
        );
        let sig_growth = stats_after.signals_total - stats_before.signals_total;
        assert!(
            sig_growth <= N + 2,
            "signals_total grew by {} (expected ≤ {} with freelist recycling)",
            sig_growth,
            N + 2,
        );
    }

    #[test]
    fn nested_scopes_drop_independently() {
        let (s0, e0) = arena_inuse_counts();
        let mut outer = Scope::new();
        with_scope(&mut outer, || {
            let _outer_sig = Signal::new("outer".to_string());
            {
                let mut inner = Scope::new();
                with_scope(&mut inner, || {
                    let _inner_sig = Signal::new("inner".to_string());
                    let _inner_eff = Effect::new(|| {});
                    let (s, e) = arena_inuse_counts();
                    assert_eq!(s, s0 + 2);
                    assert_eq!(e, e0 + 1);
                });
                // inner drops here
            }
            // After inner drops, only outer's signal remains.
            let (s, e) = arena_inuse_counts();
            assert_eq!(s, s0 + 1, "inner scope's signal freed");
            assert_eq!(e, e0, "inner scope's effect freed");
        });
        drop(outer);
        let (s, e) = arena_inuse_counts();
        assert_eq!(s, s0);
        assert_eq!(e, e0);
    }

    /// Regression test for the framework-purity refactor that moved the
    /// wasm-only `PENDING_DROPS` / rAF-sliced drain out of runtime-core
    /// and behind `install_drop_deferral`. The seam must:
    ///
    /// 1. Default to synchronous drop when no policy is installed (the
    ///    native-backend path).
    /// 2. Route effect closures + scope guards through an installed
    ///    policy when one exists (the web backend's rAF drain).
    /// 3. Still drop signals/refs synchronously (they don't go through
    ///    the policy — any deferred drain might need to read them).
    #[test]
    fn install_drop_deferral_routes_effects_and_guards_not_signals() {
        use std::cell::RefCell;
        use std::rc::Rc;

        // Capture every box the policy receives so we can introspect.
        thread_local! {
            static DEFERRED: RefCell<Vec<Box<dyn std::any::Any>>> =
                RefCell::new(Vec::new());
        }
        fn capturing_policy(mut boxes: Vec<Box<dyn std::any::Any>>) {
            DEFERRED.with(|q| q.borrow_mut().append(&mut boxes));
        }

        // Sentinel guard that marks `dropped` when its Drop fires. Clone
        // is required by `Signal<T>` (T: Clone). The clone target isn't
        // used in the test; we only care about the *last* Drop firing.
        #[derive(Clone)]
        struct Sentinel(Rc<RefCell<bool>>);
        impl Drop for Sentinel {
            fn drop(&mut self) {
                *self.0.borrow_mut() = true;
            }
        }

        // ----- 1) No policy installed (the default): everything drops
        // synchronously, including effects and guards. ------------------
        DROP_DEFERRAL.with(|c| c.set(None));
        let guard_dropped = Rc::new(RefCell::new(false));
        {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                let _e = Effect::new(|| {});
                scope_adopt_guard_for_test(Sentinel(guard_dropped.clone()));
            });
            // scope drops here → synchronous drop path
        }
        assert!(
            *guard_dropped.borrow(),
            "without an installed policy, scope guards must drop synchronously"
        );

        // ----- 2) Install a capturing policy: effects + guards go to
        // the policy, signals do NOT. -----------------------------------
        DEFERRED.with(|q| q.borrow_mut().clear());
        install_drop_deferral(capturing_policy);

        let signal_value_drop_observed = Rc::new(RefCell::new(false));
        let guard2_dropped = Rc::new(RefCell::new(false));
        {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                // Signal holding a Sentinel — its Drop runs synchronously
                // because signals don't go through the deferral policy.
                let _s: Signal<Sentinel> = Signal::new(Sentinel(signal_value_drop_observed.clone()));
                let _e = Effect::new(|| {});
                scope_adopt_guard_for_test(Sentinel(guard2_dropped.clone()));
            });
        }

        // Effect + guard are in the policy's queue, NOT dropped yet.
        assert!(
            !*guard2_dropped.borrow(),
            "with an installed policy, the scope guard must be parked in the \
             policy queue rather than dropping synchronously",
        );
        let queued = DEFERRED.with(|q| q.borrow().len());
        assert!(
            queued >= 2,
            "policy should have received at least the effect box + the guard box (got {queued})",
        );

        // Signal-held Sentinel dropped synchronously (signals stay
        // outside the deferral path).
        assert!(
            *signal_value_drop_observed.borrow(),
            "signals are not routed through the deferral policy; their \
             contained values must drop synchronously when the scope drops",
        );

        // Now manually drain the policy queue and observe the guard runs.
        DEFERRED.with(|q| q.borrow_mut().clear());
        assert!(
            *guard2_dropped.borrow(),
            "draining the policy queue must finally drop the guard"
        );

        // ----- 3) Reset to no-policy so we don't poison sibling tests. -
        DROP_DEFERRAL.with(|c| c.set(None));
    }

    /// Regression for the web history-pop abort traced to
    /// `idea-ui`'s `Collapsible::measured_body`: it `mem::forget`'d the
    /// `LayoutSubscription` (a `ResizeObserver`), so the observer was
    /// never disconnected. After the component's scope was disposed (a
    /// history-pop detaching the subtree) a late layout callback still
    /// fired and read the now-freed `natural_height: Signal<f32>` →
    /// "signal used after its scope was dropped" → abort.
    ///
    /// The contract the fix relies on: a `LayoutSubscription` anchored to
    /// the scope via [`on_cleanup`] has its drop (= observer disconnect)
    /// run when the scope drops. `mem::forget` would skip that drop — this
    /// test fails if the anchor regresses back to a leak. A tighter test
    /// against `measured_body` itself needs a layout-capable web backend
    /// (real `ResizeObserver`), which the headless test env lacks, so we
    /// assert the underlying subscription/scope contract instead.
    #[test]
    fn layout_subscription_via_on_cleanup_unsubscribes_on_scope_drop() {
        use std::cell::Cell;
        use std::rc::Rc;

        let disconnected = Rc::new(Cell::new(false));
        {
            let mut scope = Scope::new();
            let flag = disconnected.clone();
            with_scope(&mut scope, || {
                // Stands in for `ViewHandle::on_layout`'s return — its
                // drop is the observer disconnect.
                let sub = crate::handles::LayoutSubscription::new(move || flag.set(true));
                on_cleanup(move || drop(sub));
            });
            assert!(
                !disconnected.get(),
                "subscription must stay live until the scope drops"
            );
            // scope drops here → on_cleanup fires → sub drops → disconnect.
        }
        assert!(
            disconnected.get(),
            "scope drop must run the LayoutSubscription's drop (observer \
             disconnect); a `mem::forget` anchor would leak it"
        );
    }

    /// Helper that adopts a guard into the currently-active scope. The
    /// production code calls `Scope::adopt_guard` directly through its
    /// own crate-internal seams; for the test we just exercise the same
    /// path.
    fn scope_adopt_guard_for_test<G: 'static>(guard: G) {
        assert!(
            adopt_guard_into_active_scope(guard),
            "test invariant: scope must be active when adopting a guard"
        );
    }

    /// Regression test for the "memo / resource leak inside scope" audit
    /// finding. `memo_with` and `resource` both end with `mem::forget(e)`
    /// on their internal Effect. The audit claimed this caused arena
    /// growth even inside an active render scope.
    ///
    /// Verify that when a memo is created INSIDE a `with_scope`, the
    /// scope's drop frees both the memo's output Signal and its driving
    /// Effect — the `forget` is harmless in that path because the local
    /// handle's `owns` flag is already false (scope adopted the slot).
    #[test]
    fn memo_in_scope_releases_signal_and_effect_on_scope_drop() {
        let source = Signal::new(0i32);
        let (s0, e0) = arena_inuse_counts();

        {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                for _ in 0..16 {
                    let _m = memo(move || source.get() * 2);
                }
            });
            let (s_active, e_active) = arena_inuse_counts();
            // 16 memos × (1 output Signal + 1 driving Effect) inside the scope.
            // The internal Signal `last` rc-cell isn't an arena allocation,
            // so we only count one signal + one effect per memo.
            assert_eq!(
                s_active - s0,
                16,
                "expected 16 memo output signals in arena (was +{})",
                s_active - s0
            );
            assert_eq!(
                e_active - e0,
                16,
                "expected 16 memo driver effects in arena (was +{})",
                e_active - e0
            );
            // scope drops here.
        }

        let (s_after, e_after) = arena_inuse_counts();
        assert_eq!(
            s_after, s0,
            "memo output signals must be freed on scope drop \
             (the mem::forget on the Effect must not pin the Signal)"
        );
        assert_eq!(
            e_after, e0,
            "memo driver effects must be freed on scope drop \
             (mem::forget is harmless when scope owns the slot)"
        );
    }

    /// Regression test for the ACTIVE_THEME-style accumulating-subscriber concern.
    ///
    /// A hot, thread-lifetime signal that many short-lived scopes read inside
    /// effects must not accumulate dead `EffectId`s in its subscriber set across
    /// mount/unmount cycles. The fix path (`take_effects_batched` → `retain`)
    /// runs at every `Scope::drop`; this test asserts that property end-to-end.
    #[test]
    fn hot_signal_subscribers_pruned_on_scope_drop() {
        // Thread-lifetime "active theme" analogue: a signal that outlives every
        // render scope and that every component subscribes to.
        let hot = Signal::new(0i32);

        let base_subs = ARENA.with(|a| {
            a.borrow()
                .signal_subscribers
                .get(hot.id.0 as usize)
                .map(|s| s.len())
                .unwrap_or(0)
        });
        assert_eq!(base_subs, 0, "fresh signal has no subscribers");

        // Mount-and-drop many scopes, each running an effect that reads `hot`.
        // Without subscriber pruning on Scope::drop, `hot`'s subscriber set
        // would grow to ~ROUNDS * EFFECTS_PER_SCOPE.
        const ROUNDS: usize = 32;
        const EFFECTS_PER_SCOPE: usize = 16;
        for _ in 0..ROUNDS {
            let mut scope = Scope::new();
            with_scope(&mut scope, || {
                for _ in 0..EFFECTS_PER_SCOPE {
                    let _e = Effect::new(move || {
                        let _ = hot.get();
                    });
                }
            });
            // scope drops here → take_effects_batched must remove every
            // effect's subscription from `hot`.
        }

        let subs_after = ARENA.with(|a| {
            a.borrow()
                .signal_subscribers
                .get(hot.id.0 as usize)
                .map(|s| s.len())
                .unwrap_or(0)
        });
        assert_eq!(
            subs_after, 0,
            "hot signal must have zero subscribers after all reading scopes drop; \
             accumulating dead EffectIds here is the LEAK_REPORT bug",
        );

        // And the framework must still deliver writes to a freshly-subscribed
        // effect after all that churn — the prune must not have damaged the
        // signal's internal state.
        use std::cell::Cell;
        use std::rc::Rc;
        let observed = Rc::new(Cell::new(-1));
        let o = observed.clone();
        let _e = Effect::new(move || o.set(hot.get()));
        hot.set(42);
        assert_eq!(observed.get(), 42);
    }

    fn arena_refs_inuse() -> usize {
        ARENA.with(|a| a.borrow().refs.iter().filter(|r| r.is_some()).count())
    }

    /// Stand-in for a component-defined handle. Closes over a Cell so we
    /// can assert that `with(|h| h.method())` reaches the body. Clone
    /// is required so `Ref::get()` can hand back an owned copy.
    #[derive(Clone)]
    struct DummyHandle {
        counter: std::rc::Rc<std::cell::Cell<u32>>,
    }
    impl DummyHandle {
        fn bump(&self) { self.counter.set(self.counter.get() + 1); }
    }

    #[test]
    fn ref_fills_and_clears() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut scope = Scope::new();
        let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
        let counter = Rc::new(Cell::new(0));

        // Pre-mount: with() is None, bump never reaches handle.
        assert!(!r.is_mounted());
        assert!(r.with(|h| h.bump()).is_none());
        assert_eq!(counter.get(), 0);

        r.fill(DummyHandle { counter: counter.clone() });
        assert!(r.is_mounted());
        r.with(|h| h.bump());
        assert_eq!(counter.get(), 1);

        r.clear();
        assert!(!r.is_mounted());
        assert!(r.with(|h| h.bump()).is_none());
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn scope_drop_frees_ref_slot() {
        let baseline = arena_refs_inuse();
        {
            let mut scope = Scope::new();
            let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
            r.fill(DummyHandle { counter: std::rc::Rc::new(std::cell::Cell::new(0)) });
            assert_eq!(arena_refs_inuse(), baseline + 1, "ref slot in use inside scope");
            // scope drops here
        }
        assert_eq!(arena_refs_inuse(), baseline, "ref slot freed at scope drop");
    }

    #[test]
    fn ref_get_returns_owned_clone() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut scope = Scope::new();
        let r: Ref<DummyHandle> = with_scope(&mut scope, Ref::new);
        let counter = Rc::new(Cell::new(0));

        // Pre-mount: get() returns None.
        assert!(r.get().is_none());

        r.fill(DummyHandle { counter: counter.clone() });

        // The ergonomic call site: get a handle, call a method on it,
        // no closure needed.
        r.get().map(|h| h.bump());
        assert_eq!(counter.get(), 1);

        // Cloned handle outlives the temporary inside get(): the Rc
        // bump means the underlying counter is still reachable.
        let owned = r.get().unwrap();
        owned.bump();
        owned.bump();
        assert_eq!(counter.get(), 3);

        r.clear();
        assert!(r.get().is_none(), "post-unmount get() returns None");
    }
}
