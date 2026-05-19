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
use std::collections::HashSet;
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

    /// On wasm, `Scope::drop` parks its drained effect boxes here and
    /// schedules a single microtask to drain them. The arena slots
    /// are nulled synchronously (so the rebuild that follows can use
    /// fresh slot ids without conflict), but the actual `Drop` of
    /// each closure — which decrefs wasm-bindgen JS handles and runs
    /// `on_node_unstyled` per styled node — is heavy enough to push
    /// outside the apply window.
    ///
    /// Why a single microtask (not a sliced setTimeout chain): the
    /// suite measures `apply` as the synchronous JS cost of
    /// `set_rows(...)`. A microtask scheduled *during* the rebuild
    /// runs immediately after the rebuild's awaiting Promise
    /// resolves, so the drain runs in the same event-loop turn as
    /// the rebuild but doesn't count against `apply`. A
    /// `setTimeout(0)`-chained drain would yield to the suite's own
    /// macrotasks between slices, letting the next iteration's
    /// `set_rows(...)` queue more boxes faster than they drain —
    /// PENDING_DROPS would grow unbounded across iterations and JS
    /// heap pressure would slow subsequent builds. The single-
    /// microtask shape eats jank inside the 250ms transition window
    /// instead, which is the right trade.
    #[cfg(target_arch = "wasm32")]
    static PENDING_DROPS: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };

    /// Has a drain microtask been scheduled this turn? Many nested
    /// scopes can drop in quick succession; we want a single drain
    /// at the end, not one per scope.
    #[cfg(target_arch = "wasm32")]
    static PENDING_DRAIN_SCHEDULED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Arena {
    signals: Vec<Option<Box<dyn Any>>>,
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
            effects: Vec::new(),
            refs: Vec::new(),
            signal_subscribers: Vec::new(),
            effect_dependencies: Vec::new(),
            signal_free: Vec::new(),
            effect_free: Vec::new(),
            ref_free: Vec::new(),
        }
    }

    fn insert_signal<T: 'static>(&mut self, inner: SignalInner<T>) -> SignalId {
        if let Some(idx) = self.signal_free.pop() {
            // Recycle a previously-freed slot. The slot itself is
            // `None` and `signal_subscribers[idx]` is empty (cleared
            // by `take_signals_batched`), so we just stash the new
            // value.
            self.signals[idx as usize] = Some(Box::new(inner));
            // Defensive: in case a stale entry made it past cleanup.
            self.signal_subscribers[idx as usize].clear();
            SignalId(idx)
        } else {
            let id = SignalId(self.signals.len() as u32);
            self.signals.push(Some(Box::new(inner)));
            self.signal_subscribers.push(HashSet::new());
            id
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
            if let Some(slot) = self.signals.get_mut(sid.0 as usize) {
                if let Some(boxed) = slot.take() {
                    out.push(boxed);
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
    run: Box<dyn FnMut()>,
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
    // the scope already adopted the slot (`e.owns == false`) and
    // forgetting is a no-op. Outside any scope, the local binding's
    // Drop would free the slot — `forget` prevents that, leaving the
    // memo's update logic live for the lifetime of the thread, the
    // same way a bare `Signal::new` outside a scope is never reclaimed
    // (the returned handle is `Copy` with no `Drop`).
    std::mem::forget(e);

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
    _phantom: PhantomData<T>,
}

impl<T> Copy for Signal<T> {}
impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self { *self }
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
        let id = ARENA.with(|a| {
            a.borrow_mut().insert_signal(SignalInner { value })
        });
        register_signal(id);
        Self { id, _phantom: PhantomData }
    }

    pub fn get(&self) -> T {
        // Record subscription if an effect is currently running. The
        // arena holds the inverse map (`signal_subscribers` +
        // `effect_dependencies`) so each link is recorded under a
        // single mutable borrow.
        let sid = self.id;
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
        with_signal::<T, _>(sid, |inner| inner.value.clone())
    }

    pub fn set(&self, value: T) {
        assert_not_in_memo_compute();
        with_signal_mut::<T, _>(self.id, |inner| {
            inner.value = value;
        });
        #[cfg(debug_assertions)]
        record_write_in_running_effect(self.id);
        // Subscriber lists are kept tight on the cleanup side (effect
        // drop / effect re-run), so no pruning pass needed here.
        let to_run = collect_subscribers(self.id);
        notify_or_queue(&to_run);
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        assert_not_in_memo_compute();
        with_signal_mut::<T, _>(self.id, |inner| {
            f(&mut inner.value);
        });
        #[cfg(debug_assertions)]
        record_write_in_running_effect(self.id);
        let to_run = collect_subscribers(self.id);
        notify_or_queue(&to_run);
    }
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

#[cfg(debug_assertions)]
mod read_write_audit {
    //! Debug-only detection of effects that both read AND write the
    //! same signal. The reentry guard in `run_effect` makes the pattern
    //! safe (no infinite loop), but it's almost always a sign the
    //! author wanted `memo` or a guarded write — useful to flag at
    //! development time.
    //!
    //! Implementation: a per-running-effect write list (keyed in a
    //! thread-local map so nested runs don't collide), checked against
    //! the effect's recorded read deps at the end of each run. First
    //! occurrence per `(EffectId, SignalId)` pair on this thread emits
    //! to stderr; subsequent hits are deduped via `WARNED_PAIRS` so a
    //! steady-state bridge pattern only logs once.

    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    thread_local! {
        pub(super) static EFFECT_WRITES: RefCell<HashMap<EffectId, Vec<SignalId>>> =
            RefCell::new(HashMap::new());
        pub(super) static WARNED_PAIRS: RefCell<HashSet<(EffectId, SignalId)>> =
            RefCell::new(HashSet::new());
    }
}

/// Records a write to `sid` as having happened during the currently-
/// running effect (if any). Used by [`run_effect`] to detect the
/// "effect reads and writes the same signal" smell after the run
/// completes. Debug builds only — release builds carry zero overhead.
#[cfg(debug_assertions)]
fn record_write_in_running_effect(sid: SignalId) {
    let Some(eid) = CURRENT.with(|c| *c.borrow()) else {
        return;
    };
    read_write_audit::EFFECT_WRITES.with(|w| {
        w.borrow_mut().entry(eid).or_default().push(sid);
    });
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

fn with_signal<T: 'static, R>(id: SignalId, f: impl FnOnce(&SignalInner<T>) -> R) -> R {
    ARENA.with(|arena| {
        let arena = arena.borrow();
        let slot = arena
            .signals
            .get(id.0 as usize)
            .and_then(|o| o.as_ref())
            .unwrap_or_else(|| panic!("signal used after its scope was dropped (id {:?})", id));
        let inner = slot
            .downcast_ref::<SignalInner<T>>()
            .expect("internal: signal type mismatch");
        f(inner)
    })
}

fn with_signal_mut<T: 'static, R>(id: SignalId, f: impl FnOnce(&mut SignalInner<T>) -> R) -> R {
    ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let slot = arena
            .signals
            .get_mut(id.0 as usize)
            .and_then(|o| o.as_mut())
            .unwrap_or_else(|| panic!("signal used after its scope was dropped (id {:?})", id));
        let inner = slot
            .downcast_mut::<SignalInner<T>>()
            .expect("internal: signal type mismatch");
        f(inner)
    })
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
                run: Box::new(f),
                cleanups: Vec::new(),
                owning_stack,
            })
        });
        let registered = register_effect(id);
        run_effect(id);
        Effect { id, owns: !registered }
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
    if depth > MAX_EFFECT_DEPTH {
        panic!(
            "effect run depth exceeded {} — likely a mutual signal/effect cycle. \
             Check for two or more effects that read and write each other's signals.",
            MAX_EFFECT_DEPTH
        );
    }

    // Fire any cleanup callbacks registered during the previous run
    // before recording fresh deps. They run in LIFO order to mirror
    // typical resource-acquisition order. Drained out under the arena
    // borrow so the callbacks themselves can re-borrow the arena to
    // read or write signals.
    let prev_cleanups: Vec<Box<dyn FnOnce()>> = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        a.effects
            .get_mut(id.0 as usize)
            .and_then(|o| o.as_mut())
            .and_then(|slot| slot.downcast_mut::<EffectInner>())
            .map(|inner| std::mem::take(&mut inner.cleanups))
            .unwrap_or_default()
    });
    for cb in prev_cleanups.into_iter().rev() {
        cb();
    }

    // Drop any subscriptions recorded by the previous run before we
    // collect this run's set. Without this, a re-run that reads a
    // *different* set of signals would leave stale `eid` entries in
    // the no-longer-read signals' subscriber sets — they'd be cleaned
    // up at effect drop, but in the meantime the signal would re-fire
    // an effect that doesn't care about it.
    clear_effect_dependencies(id);

    // Take the closure out AND clone the owning-scope snapshot under
    // a single arena borrow. Two reasons: (a) we need the snapshot to
    // restore the scope stack before f() runs, and reading it under
    // the same borrow keeps the arena access cheap; (b) f() will
    // re-enter the arena to read/write signals, so we can't hold the
    // borrow across the call.
    let (mut run_fn, owning_stack): (Option<Box<dyn FnMut()>>, Vec<*mut Scope>) =
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let Some(Some(slot)) = a.effects.get_mut(id.0 as usize) else {
                return (None, Vec::new());
            };
            let Some(inner) = slot.downcast_mut::<EffectInner>() else {
                return (None, Vec::new());
            };
            let run = std::mem::replace(&mut inner.run, Box::new(|| {}));
            let stack = inner.owning_stack.clone();
            (Some(run), stack)
        });
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
        let prev = CURRENT.with(|c| c.replace(Some(id)));
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
                    inner.run = run_fn.take().unwrap();
                }
            }
        });

        #[cfg(debug_assertions)]
        audit_effect_read_write_overlap(id);
    }
}

/// Compares the set of signals this effect just *wrote* against the set
/// of signals it just *read* (its dep set). Any signal appearing in
/// both is logged once per `(EffectId, SignalId)` pair to stderr — it's
/// almost always a sign the author wanted `memo` for a derived value
/// or a guarded write. The reentry guard makes the pattern safe at
/// runtime, so this is advisory, not fatal.
///
/// Debug builds only — release builds compile out the write tracking
/// and this function. No allocation on the happy path (effect didn't
/// write anything).
#[cfg(debug_assertions)]
fn audit_effect_read_write_overlap(id: EffectId) {
    use read_write_audit::{EFFECT_WRITES, WARNED_PAIRS};

    // Pop the write list for this effect's just-completed run. If the
    // effect didn't write anything, there's nothing to check.
    let writes: Vec<SignalId> = EFFECT_WRITES
        .with(|w| w.borrow_mut().remove(&id))
        .unwrap_or_default();
    if writes.is_empty() {
        return;
    }

    // Snapshot the post-run dep set. The arena borrow is held briefly;
    // we collect into a Vec before releasing it because the eprintln
    // calls below shouldn't run under an arena borrow.
    let deps: Vec<SignalId> = ARENA.with(|a| {
        let a = a.borrow();
        a.effect_dependencies
            .get(id.0 as usize)
            .map(|d| d.iter().copied().collect())
            .unwrap_or_default()
    });
    if deps.is_empty() {
        return;
    }

    for sid in writes {
        if !deps.contains(&sid) {
            continue;
        }
        let is_new = WARNED_PAIRS.with(|w| w.borrow_mut().insert((id, sid)));
        if is_new {
            eprintln!(
                "[framework-core] effect {:?} both reads AND writes signal {:?}. \
                 This terminates safely (reentry guard) but is usually a bug — \
                 consider `memo` for derived values, or guard the write with an \
                 inequality check if it's an intentional bridge.",
                id, sid
            );
        }
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
        // On wasm: park the heavy boxes (effect closures) for a
        // microtask drain so their teardown cost lands outside the
        // synchronous `apply` window. Signals and refs stay
        // synchronous — they don't hold JS-side closures and any
        // queued microtask draining boxes might need them.
        #[cfg(target_arch = "wasm32")]
        {
            if !taken_effects.is_empty() {
                PENDING_DROPS.with(|q| q.borrow_mut().extend(taken_effects));
                schedule_pending_drain();
            }
            // Same deferral applies to the scope's guards: they
            // typically hold `StyleHandle`s that decref a JS-side
            // Node on drop, which is the same kind of FFI-heavy
            // work we're trying to keep out of the apply window.
            if !guards.is_empty() {
                PENDING_DROPS.with(|q| q.borrow_mut().extend(guards));
                schedule_pending_drain();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            drop(taken_effects);
            drop(guards);
        }

        drop(taken_signals);
        drop(taken_refs);
    }
}

/// Schedule a sliced drain of `PENDING_DROPS` aligned to
/// `requestAnimationFrame`. Each rAF callback drops a budgeted
/// number of boxes (small enough to fit within a 16 ms frame
/// budget) and re-schedules itself if more remain. Idempotent:
/// repeated calls before the first slice fires coalesce into one
/// scheduled rAF.
///
/// Why rAF and not `setTimeout(0)`:
///
/// - Microtask drain would be included in the `apply` timing the
///   suite reads right after `await setRows(...)`. So that path
///   was ruled out.
/// - A single `setTimeout(0)` drain runs the whole queue in one
///   blob — fast on `apply` but blows one frame inside the 250 ms
///   transition window (the `worst frame: 200ms` we saw on 1k-
///   after-10k iters).
/// - Chained `setTimeout(0)` slices yield to the suite's own
///   macrotasks between slices, so a fast iteration loop can
///   queue drops faster than slices drain — backlog grows. This
///   is the failure mode the earlier sliced attempt hit.
/// - **rAF rate-limits naturally.** The browser fires it at most
///   once per display refresh (16.7 ms at 60 Hz). Inside the
///   250+50 ms transition window there are ~18 ticks, plenty to
///   drain 10k boxes in batches of ~1000 each. Between iterations
///   the browser pauses rAF until something paint-worthy happens,
///   so PENDING_DROPS naturally empties before the next iteration
///   starts.
///
/// The slice budget is intentionally large enough that an empty
/// queue is one tick away from start to finish — we don't want to
/// drag drops out over many frames if there's nothing to do.
#[cfg(target_arch = "wasm32")]
fn schedule_pending_drain() {
    let already = PENDING_DRAIN_SCHEDULED.with(|c| c.replace(true));
    if already {
        return;
    }
    request_drain_frame();
}

/// Request one rAF tick to drain a slice of `PENDING_DROPS`. The
/// callback re-arms itself via `request_drain_frame()` if the
/// queue still has work; otherwise it clears the
/// `PENDING_DRAIN_SCHEDULED` flag so the next scope drop can
/// re-kick the loop.
#[cfg(target_arch = "wasm32")]
fn request_drain_frame() {
    // Tunable. Larger = fewer rAFs needed to drain a big queue,
    // but more work per frame. 2000 fits comfortably inside a
    // 16 ms frame budget at our measured ~10 µs per box drop —
    // worst case ~20 ms which is one stutter but won't compound.
    const PER_FRAME_BUDGET: usize = 2000;
    let task = crate::scheduling::after_animation_frame(|| {
        // Take up to `PER_FRAME_BUDGET` boxes off the queue and
        // drop them. We `split_off` rather than `drain` so the
        // remaining boxes stay in their original allocation and
        // ordering — no per-call reallocations.
        let to_drop = PENDING_DROPS.with(|q| {
            let mut q = q.borrow_mut();
            let n = q.len().min(PER_FRAME_BUDGET);
            // Drain the tail (most recently parked entries —
            // typically the deepest-nested children) so each slice
            // touches a contiguous block. `split_off` from the
            // tail end is cheap (just truncate + return owned).
            let split_at = q.len() - n;
            q.split_off(split_at)
        });
        drop(to_drop);
        // If anything's left, re-arm. Otherwise mark idle.
        let remaining = PENDING_DROPS.with(|q| q.borrow().len());
        if remaining > 0 {
            request_drain_frame();
        } else {
            PENDING_DRAIN_SCHEDULED.with(|c| c.set(false));
        }
    });
    // Fire-and-forget: leak the task handle so its Closure stays
    // alive past the rAF dispatch. Dropping the task would cancel
    // the pending frame; we want the opposite. The browser fires
    // the callback once, then the task is unreachable garbage —
    // bounded by the number of slices needed to drain (~18 per
    // 250 ms transition window).
    std::mem::forget(task);
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
    // Read+write-same-signal audit (debug-only)
    // -----------------------------------------------------------------

    #[cfg(debug_assertions)]
    fn warned_pairs_count() -> usize {
        super::read_write_audit::WARNED_PAIRS.with(|w| w.borrow().len())
    }

    #[cfg(debug_assertions)]
    fn clear_warned_pairs() {
        super::read_write_audit::WARNED_PAIRS.with(|w| w.borrow_mut().clear());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn effect_reading_and_writing_same_signal_warns_once() {
        clear_warned_pairs();
        let before = warned_pairs_count();
        let s = Signal::new(0i32);
        let _e = Effect::new(move || {
            // Read then unconditionally write — the canonical smell.
            let v = s.get();
            s.set(v + 1);
        });
        // First run logged exactly one new pair.
        assert_eq!(
            warned_pairs_count(),
            before + 1,
            "read-then-write produces one new warning on the first run"
        );
        // External writes re-fire the effect; the same pair must not
        // produce additional warnings (dedup).
        s.set(50);
        s.set(60);
        assert_eq!(
            warned_pairs_count(),
            before + 1,
            "subsequent runs of the same effect on the same signal must dedup"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn effect_only_reading_does_not_warn() {
        clear_warned_pairs();
        let before = warned_pairs_count();
        let s = Signal::new(0i32);
        let _e = Effect::new(move || {
            let _ = s.get();
        });
        s.set(5);
        assert_eq!(warned_pairs_count(), before, "read-only effect must not warn");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn effect_only_writing_does_not_warn() {
        clear_warned_pairs();
        let before = warned_pairs_count();
        let s = Signal::new(0i32);
        let _e = Effect::new(move || {
            // No read of `s` → no dep → no overlap.
            s.set(99);
        });
        assert_eq!(warned_pairs_count(), before, "write-only effect must not warn");
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
