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
}

// =============================================================================
// untrack
// =============================================================================

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
        with_signal_mut::<T, _>(self.id, |inner| {
            inner.value = value;
        });
        // Subscriber lists are kept tight on the cleanup side (effect
        // drop / effect re-run), so no pruning pass needed here.
        let to_run = collect_subscribers(self.id);
        run_effects(&to_run);
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        with_signal_mut::<T, _>(self.id, |inner| {
            f(&mut inner.value);
        });
        let to_run = collect_subscribers(self.id);
        run_effects(&to_run);
    }
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
        let id = ARENA.with(|a| {
            a.borrow_mut().insert_effect(EffectInner { run: Box::new(f) })
        });
        let registered = register_effect(id);
        run_effect(id);
        Effect { id, owns: !registered }
    }
}

/// Run the effect with `id`. The closure is temporarily moved out of the
/// arena slot during execution so signal callbacks can re-borrow the arena
/// without conflict. Restored on completion.
fn run_effect(id: EffectId) {
    // Drop any subscriptions recorded by the previous run before we
    // collect this run's set. Without this, a re-run that reads a
    // *different* set of signals would leave stale `eid` entries in
    // the no-longer-read signals' subscriber sets — they'd be cleaned
    // up at effect drop, but in the meantime the signal would re-fire
    // an effect that doesn't care about it.
    clear_effect_dependencies(id);

    let mut run_fn: Option<Box<dyn FnMut()>> = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let slot = a.effects.get_mut(id.0 as usize)?.as_mut()?;
        let inner = slot.downcast_mut::<EffectInner>()?;
        // Replace with a no-op while we run, to detect re-entry and avoid
        // a double-borrow of the arena. We restore the original afterward.
        Some(std::mem::replace(&mut inner.run, Box::new(|| {})))
    });
    if let Some(f) = run_fn.as_mut() {
        let prev = CURRENT.with(|c| c.replace(Some(id)));
        f();
        CURRENT.with(|c| *c.borrow_mut() = prev);
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
        Self { signals: Vec::new(), effects: Vec::new(), refs: Vec::new(), guards: Vec::new() }
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

/// Schedule a macrotask (`setTimeout(0)`) that drops every box in
/// `PENDING_DROPS` in a single pass. Idempotent: repeated calls
/// within the same turn coalesce into one drain.
///
/// Why a macrotask and not a microtask: microtasks all drain before
/// `await someAsync()` resolves, so a microtask drain would be
/// included in the `apply` timing the suite reads right after
/// `await setRows(...)`. `setTimeout(0)` runs on the next event-loop
/// turn — after `apply` is recorded — but before the next iteration
/// (the suite sleeps 50ms between iters via `setTimeout`, which the
/// drain races, plus the 250ms transition window). Net: drain
/// completes inside the transition window of the iteration that
/// triggered it, not as part of `apply`. The cost shows up as a
/// `worst frame` spike during the transition, not as `apply` time.
#[cfg(target_arch = "wasm32")]
fn schedule_pending_drain() {
    let already = PENDING_DRAIN_SCHEDULED.with(|c| c.replace(true));
    if already {
        return;
    }
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    let cb: Closure<dyn FnMut()> = Closure::new(|| {
        PENDING_DRAIN_SCHEDULED.with(|c| c.set(false));
        let drops = PENDING_DROPS.with(|q| std::mem::take(&mut *q.borrow_mut()));
        drop(drops);
    });
    if let Some(w) = web_sys::window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            0,
        );
    }
    cb.forget();
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
