//! Prototype: copy-handle reactive primitives via a thread-local arena.
//!
//! Validates whether `Signal<T>: Copy` is feasible without unsafe code and
//! without architectural compromises. Not integrated with runtime-core
//! until the prototype proves sound.
//!
//! # Design
//!
//! - All reactive storage lives in a thread-local `Arena` of slots. Each
//!   slot carries a *generation* counter. Slots are addressed by
//!   `{ index, generation }` IDs; freeing a slot bumps its generation and
//!   returns the index to a free-list for reuse. A stale handle whose
//!   generation no longer matches the slot is rejected (panic on read,
//!   silent skip on prune) — so a dead handle never aliases live storage
//!   even though indices are reused. Generational reuse keeps the arena
//!   bounded by *peak concurrent* slots, not total-ever-allocated.
//! - `Signal<T>` is a `Copy` handle: `(SignalId, PhantomData<T>)`. The type
//!   parameter is preserved at compile time; the runtime downcast on access
//!   is checking an invariant, not selecting behavior.
//! - `Effect` is also an arena-backed slot. The subscriber graph is stored
//!   *type-erased* on the slots: each signal slot holds the `EffectId`s that
//!   read it, and each effect slot holds the `SignalId`s it reads (a
//!   back-reference). Because the graph lives outside `SignalInner<T>`, it
//!   can be pruned without knowing `T`.
//! - `Scope` owns a list of signal/effect IDs. Dropping the scope frees each
//!   one, which eagerly removes the freed effect from every signal's
//!   subscriber list (and vice-versa). Pruning is therefore independent of
//!   whether the signal is ever written again — closing the accumulating
//!   dead-subscriber leak.
//! - Each effect re-run first clears its previous subscriptions and
//!   recaptures a fresh dependency set, so dynamic dependencies (a signal
//!   read only on some branches) don't accumulate stale edges.
//!
//! # Failure modes (intentional, diagnostic)
//!
//! - Reading from a `Signal<T>` whose scope has dropped → panic with a
//!   clear message. The signal's slot is empty / its generation has moved on.
//! - Dangling effect IDs are pruned eagerly on free; the notify path also
//!   skips any slot whose generation no longer matches, so a write that
//!   races a teardown is still safe.
//! - A reentrant re-run of an already-running effect is skipped (its closure
//!   is checked out while it runs), preventing unbounded self-recursion.
//!
//! # What's NOT used
//!
//! - No `unsafe`.
//! - No `transmute`.
//! - No raw pointers.
//! - No `Rc`/`Arc` for handles — only `RefCell` for arena interior mutability.

// Sibling prototype: a downcast-free, unsafe-free Copy-handle arena built on
// leaked `&'static` typed slots. See `static_slab.rs`.
pub mod static_slab;

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;

// ----------------------------------------------------------------------------
// IDs
// ----------------------------------------------------------------------------

/// Generational address of a signal slot. `generation` guards against a
/// reused index: a handle is only valid while the slot's live generation
/// matches.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SignalId {
    index: u32,
    generation: u32,
}

/// Generational address of an effect slot. See [`SignalId`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectId {
    index: u32,
    generation: u32,
}

// ----------------------------------------------------------------------------
// Arena
// ----------------------------------------------------------------------------

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
    /// The effect that is currently running, if any. Set during an effect's
    /// initial run and during re-runs from `Signal::set`. Reads via
    /// `Signal::get` consult this slot to register subscriptions.
    static CURRENT: RefCell<Option<EffectId>> = const { RefCell::new(None) };
}

/// A signal's backing storage. The value is type-erased (`Box<dyn Any>` of
/// the concrete `T`); `subscribers` is the type-erased forward edge of the
/// dependency graph so it can be pruned without knowing `T`.
struct SignalSlot {
    generation: u32,
    value: Option<Box<dyn Any>>,
    subscribers: Vec<EffectId>,
}

/// An effect's backing storage. `run` is the closure, checked out (`None`)
/// while the effect is executing. `subscribed` is the back-edge: the signals
/// this effect currently reads, used to detach it on free / re-run.
struct EffectSlot {
    generation: u32,
    alive: bool,
    run: Option<Box<dyn FnMut()>>,
    subscribed: Vec<SignalId>,
}

struct Arena {
    signals: Vec<SignalSlot>,
    signal_free: Vec<u32>,
    effects: Vec<EffectSlot>,
    effect_free: Vec<u32>,
}

impl Arena {
    fn new() -> Self {
        Self {
            signals: Vec::new(),
            signal_free: Vec::new(),
            effects: Vec::new(),
            effect_free: Vec::new(),
        }
    }

    fn insert_signal<T: 'static>(&mut self, value: T) -> SignalId {
        let boxed: Box<dyn Any> = Box::new(value);
        if let Some(index) = self.signal_free.pop() {
            let slot = &mut self.signals[index as usize];
            debug_assert!(slot.value.is_none(), "free-listed signal slot was still live");
            slot.value = Some(boxed);
            slot.subscribers.clear();
            SignalId { index, generation: slot.generation }
        } else {
            let index = self.signals.len() as u32;
            self.signals.push(SignalSlot { generation: 0, value: Some(boxed), subscribers: Vec::new() });
            SignalId { index, generation: 0 }
        }
    }

    fn insert_effect(&mut self, run: Box<dyn FnMut()>) -> EffectId {
        if let Some(index) = self.effect_free.pop() {
            let slot = &mut self.effects[index as usize];
            debug_assert!(!slot.alive, "free-listed effect slot was still live");
            slot.alive = true;
            slot.run = Some(run);
            slot.subscribed.clear();
            EffectId { index, generation: slot.generation }
        } else {
            let index = self.effects.len() as u32;
            self.effects.push(EffectSlot {
                generation: 0,
                alive: true,
                run: Some(run),
                subscribed: Vec::new(),
            });
            EffectId { index, generation: 0 }
        }
    }

    fn signal_is_live(&self, id: SignalId) -> bool {
        self.signals
            .get(id.index as usize)
            .is_some_and(|s| s.generation == id.generation && s.value.is_some())
    }

    fn effect_is_live(&self, id: EffectId) -> bool {
        self.effects
            .get(id.index as usize)
            .is_some_and(|e| e.generation == id.generation && e.alive)
    }

    /// Records the dependency edge `signal -> effect` (and its back-edge)
    /// when both endpoints are live. Idempotent.
    fn record_subscription(&mut self, sid: SignalId, eid: EffectId) {
        if !(self.signal_is_live(sid) && self.effect_is_live(eid)) {
            return;
        }
        let sig = &mut self.signals[sid.index as usize];
        if !sig.subscribers.contains(&eid) {
            sig.subscribers.push(eid);
        }
        let eff = &mut self.effects[eid.index as usize];
        if !eff.subscribed.contains(&sid) {
            eff.subscribed.push(sid);
        }
    }

    /// Detaches `eid` from every signal it currently reads and clears its
    /// back-edge list. Used before a re-run (to recapture fresh deps) and on
    /// free (to stop a dead effect lingering in subscriber lists). This is
    /// the eager pruning that prevents the accumulating dead-subscriber leak.
    fn clear_effect_subscriptions(&mut self, eid: EffectId) {
        let subs = match self.effects.get_mut(eid.index as usize) {
            Some(slot) if slot.generation == eid.generation => std::mem::take(&mut slot.subscribed),
            _ => return,
        };
        for sid in subs {
            if let Some(sig) = self.signals.get_mut(sid.index as usize) {
                if sig.generation == sid.generation {
                    sig.subscribers.retain(|e| *e != eid);
                }
            }
        }
    }

    fn free_signal(&mut self, id: SignalId) {
        let Some(slot) = self.signals.get_mut(id.index as usize) else { return };
        if slot.generation != id.generation || slot.value.is_none() {
            return;
        }
        slot.value = None;
        slot.subscribers.clear();
        slot.generation = slot.generation.wrapping_add(1);
        self.signal_free.push(id.index);
    }

    fn free_effect(&mut self, id: EffectId) {
        // Detach from every signal it subscribes to first, then tear down.
        self.clear_effect_subscriptions(id);
        let Some(slot) = self.effects.get_mut(id.index as usize) else { return };
        if slot.generation != id.generation || !slot.alive {
            return;
        }
        slot.alive = false;
        slot.run = None;
        slot.subscribed.clear();
        slot.generation = slot.generation.wrapping_add(1);
        self.effect_free.push(id.index);
    }
}

// ----------------------------------------------------------------------------
// Signal<T>
// ----------------------------------------------------------------------------

/// A copy-handle to a reactive value. The handle is plain data
/// (id + phantom); the actual storage lives in the thread-local arena.
///
/// `Signal<T>` is `Copy` because it contains no owning references. Multiple
/// copies of the same `Signal<T>` all address the same arena slot.
///
/// # Lifetime
///
/// The arena slot is freed when the owning `Scope` is dropped (or via
/// [`Signal::dispose`] for an unscoped signal). Reading from a signal after
/// its slot has been freed panics with a clear message; the generation guard
/// guarantees a freed handle can never observe a different signal that later
/// reused the same index.
pub struct Signal<T> {
    id: SignalId,
    _phantom: PhantomData<T>,
}

impl<T> Copy for Signal<T> {}
impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Clone + 'static> Signal<T> {
    /// Creates a signal *not* owned by any scope. The caller is responsible
    /// for its lifetime: either hand the handle to a [`Scope`] (prefer
    /// [`Scope::signal`]) or free it explicitly with [`Signal::dispose`].
    /// An undisposed unscoped signal occupies its arena slot for the thread
    /// lifetime.
    pub fn new(value: T) -> Self {
        let id = ARENA.with(|a| a.borrow_mut().insert_signal(value));
        Signal { id, _phantom: PhantomData }
    }

    pub fn get(&self) -> T {
        if let Some(eid) = CURRENT.with(|c| *c.borrow()) {
            ARENA.with(|a| a.borrow_mut().record_subscription(self.id, eid));
        }
        with_signal::<T, _>(self.id, |v| v.clone())
    }

    pub fn set(&self, value: T) {
        let to_run = ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let slot = a
                .signals
                .get_mut(self.id.index as usize)
                .filter(|s| s.generation == self.id.generation && s.value.is_some())
                .unwrap_or_else(|| panic!("signal used after its scope was dropped (id {:?})", self.id));
            *slot
                .value
                .as_mut()
                .unwrap()
                .downcast_mut::<T>()
                .expect("internal: signal type mismatch (this should never happen)") = value;
            slot.subscribers.clone()
        });
        for eid in to_run {
            run_effect(eid);
        }
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        let to_run = ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let slot = a
                .signals
                .get_mut(self.id.index as usize)
                .filter(|s| s.generation == self.id.generation && s.value.is_some())
                .unwrap_or_else(|| panic!("signal used after its scope was dropped (id {:?})", self.id));
            f(slot
                .value
                .as_mut()
                .unwrap()
                .downcast_mut::<T>()
                .expect("internal: signal type mismatch (this should never happen)"));
            slot.subscribers.clone()
        });
        for eid in to_run {
            run_effect(eid);
        }
    }

    /// Frees this signal's arena slot. Only needed for signals created via
    /// [`Signal::new`] outside a [`Scope`]; scope-owned signals free
    /// automatically when the scope drops. Calling `get`/`set` afterward
    /// panics.
    pub fn dispose(self) {
        ARENA.with(|a| a.borrow_mut().free_signal(self.id));
    }

    /// For tests: returns the underlying ID. Not exposed in a real framework.
    #[doc(hidden)]
    pub fn id_for_tests(&self) -> SignalId {
        self.id
    }
}

fn with_signal<T: 'static, R>(id: SignalId, f: impl FnOnce(&T) -> R) -> R {
    ARENA.with(|arena| {
        let arena = arena.borrow();
        let boxed = arena
            .signals
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation)
            .and_then(|s| s.value.as_ref())
            .unwrap_or_else(|| panic!("signal used after its scope was dropped (id {:?})", id));
        let value = boxed
            .downcast_ref::<T>()
            .expect("internal: signal type mismatch (this should never happen)");
        f(value)
    })
}

// ----------------------------------------------------------------------------
// Effect
// ----------------------------------------------------------------------------

/// Restores a checked-out effect closure on scope exit — including panic
/// unwind — so a panicking effect body never permanently disables the effect,
/// and restores the previous `CURRENT` so a panic doesn't leak the running-
/// effect context into later reads.
struct EffectRunGuard {
    id: EffectId,
    run: Option<Box<dyn FnMut()>>,
    prev_current: Option<EffectId>,
}

impl Drop for EffectRunGuard {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = self.prev_current);
        if let Some(run) = self.run.take() {
            ARENA.with(|a| {
                let mut a = a.borrow_mut();
                if let Some(slot) = a.effects.get_mut(self.id.index as usize) {
                    // Only restore if the slot is still the same live effect.
                    // If it was freed mid-run, the generation moved on and we
                    // drop the closure instead of resurrecting a dead effect.
                    if slot.generation == self.id.generation && slot.alive {
                        slot.run = Some(run);
                    }
                }
            });
        }
    }
}

/// Checks out an effect's closure, recaptures its dependency set, and runs it.
/// The closure is held by an RAII guard so it is always returned to the arena,
/// even if the body panics. Re-running an effect that is already executing
/// (its closure is checked out) is skipped to prevent unbounded recursion.
fn run_effect(eid: EffectId) {
    let run = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let slot = a.effects.get_mut(eid.index as usize)?;
        if slot.generation != eid.generation || !slot.alive {
            return None;
        }
        // `None` here means the effect is already running higher in the
        // stack (reentry) — skip rather than recurse.
        slot.run.take()
    });
    let Some(run) = run else { return };

    // Drop stale edges before the run; `get` recaptures the live set.
    ARENA.with(|a| a.borrow_mut().clear_effect_subscriptions(eid));

    let prev_current = CURRENT.with(|c| c.replace(Some(eid)));
    let mut guard = EffectRunGuard { id: eid, run: Some(run), prev_current };
    (guard.run.as_mut().expect("guard always holds the closure during run"))();
    // `guard` drops here: restores CURRENT and returns the closure to the arena.
}

/// Creates an effect and runs it once. Any signals read during the run will
/// re-fire the effect on change. Returns the `EffectId` so a `Scope` can own
/// its lifetime; an unscoped effect should be freed with [`dispose_effect`].
pub fn effect<F: FnMut() + 'static>(f: F) -> EffectId {
    let id = ARENA.with(|a| a.borrow_mut().insert_effect(Box::new(f)));
    run_effect(id);
    id
}

/// Frees an effect created via [`effect`] outside a [`Scope`], detaching it
/// from every signal it subscribes to. Scope-owned effects free automatically.
pub fn dispose_effect(id: EffectId) {
    ARENA.with(|a| a.borrow_mut().free_effect(id));
}

// ----------------------------------------------------------------------------
// Scope
// ----------------------------------------------------------------------------

/// Owns a set of arena slots. When the `Scope` drops, all slots it owns are
/// freed, releasing their storage *and* detaching them from the dependency
/// graph. Use a `Scope` to bound the lifetime of signals and effects created
/// inside a component, render pass, or test.
pub struct Scope {
    signals: Vec<SignalId>,
    effects: Vec<EffectId>,
}

impl Scope {
    pub fn new() -> Self {
        Self { signals: Vec::new(), effects: Vec::new() }
    }

    pub fn signal<T: Clone + 'static>(&mut self, value: T) -> Signal<T> {
        let s = Signal::new(value);
        self.signals.push(s.id);
        s
    }

    pub fn effect<F: FnMut() + 'static>(&mut self, f: F) -> EffectId {
        let id = effect(f);
        self.effects.push(id);
        id
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            // Free effects first so their back-edges are pruned before the
            // signals they read disappear (order isn't required for safety —
            // the generation guard handles either order — but it keeps the
            // graph consistent at each step).
            for id in self.effects.drain(..) {
                a.free_effect(id);
            }
            for id in self.signals.drain(..) {
                a.free_signal(id);
            }
        });
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;

    #[test]
    fn signal_is_copy() {
        let mut scope = Scope::new();
        let s = scope.signal(7i32);
        let s2 = s;
        let s3 = s;
        assert_eq!(s2.get(), 7);
        assert_eq!(s3.get(), 7);
        s.set(42);
        assert_eq!(s.get(), 42);
        assert_eq!(s2.get(), 42);
        assert_eq!(s3.get(), 42);
    }

    #[test]
    fn effect_fires_on_signal_change() {
        let mut scope = Scope::new();
        let count = scope.signal(0i32);
        let observed = Rc::new(Cell::new(0));
        let observed_clone = observed.clone();
        scope.effect(move || {
            observed_clone.set(count.get());
        });
        assert_eq!(observed.get(), 0);
        count.set(5);
        assert_eq!(observed.get(), 5);
        count.set(11);
        assert_eq!(observed.get(), 11);
    }

    #[test]
    fn scope_drop_frees_signal() {
        let id;
        {
            let mut scope = Scope::new();
            let s = scope.signal(0i32);
            id = s.id_for_tests();
            assert_eq!(s.get(), 0);
        }
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.signals[id.index as usize].value.is_none(), "slot should be freed");
        });
    }

    #[test]
    #[should_panic(expected = "signal used after its scope was dropped")]
    fn dead_signal_panics() {
        let id;
        {
            let mut scope = Scope::new();
            let s = scope.signal(99i32);
            id = s.id_for_tests();
            assert_eq!(s.get(), 99);
        }
        // Smuggle the dead id into a reconstructed handle (not possible in
        // real code) to demonstrate the panic.
        let dead: Signal<i32> = Signal { id, _phantom: PhantomData };
        let _ = dead.get();
    }

    #[test]
    fn copy_into_closures_no_clones() {
        let mut scope = Scope::new();
        let toggle = scope.signal(false);
        let read = move || toggle.get();
        let write = move || toggle.set(true);
        assert_eq!(read(), false);
        write();
        assert_eq!(read(), true);
    }

    #[test]
    fn nested_scopes_free_independently() {
        let mut outer = Scope::new();
        let outer_sig = outer.signal(10i32);
        let outer_id = outer_sig.id_for_tests();
        let inner_id;
        {
            let mut inner = Scope::new();
            let inner_sig = inner.signal(20i32);
            inner_id = inner_sig.id_for_tests();
            assert_eq!(inner_sig.get(), 20);
            assert_eq!(outer_sig.get(), 10);
        }
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.signals[inner_id.index as usize].value.is_none());
            assert!(a.signals[outer_id.index as usize].value.is_some());
        });
        assert_eq!(outer_sig.get(), 10);
    }

    /// Regression: dead effects must be pruned from a signal's subscriber
    /// list on scope drop, not only on the next write. Without eager pruning
    /// the list accumulates dead `EffectId`s across rebuilds of a hot,
    /// rarely-written signal (the documented LEAK_REPORT.md leak).
    #[test]
    fn dead_effect_pruned_from_signal_on_scope_drop() {
        let mut outer = Scope::new();
        let sig = outer.signal(0i32);
        let sid = sig.id_for_tests();
        {
            let mut inner = Scope::new();
            inner.effect(move || {
                let _ = sig.get();
            });
            ARENA.with(|a| {
                let a = a.borrow();
                assert_eq!(
                    a.signals[sid.index as usize].subscribers.len(),
                    1,
                    "effect should be subscribed while inner scope is live"
                );
            });
        }
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(
                a.signals[sid.index as usize].subscribers.is_empty(),
                "dead effect must be pruned from subscriber list on scope drop, \
                 with no intervening write"
            );
        });
        // Signal still works after the prune.
        sig.set(1);
        assert_eq!(sig.get(), 1);
    }

    /// Regression: subscriber lists must not grow without bound when an
    /// effect re-runs. Each run recaptures a fresh dependency set rather than
    /// appending duplicate edges.
    #[test]
    fn resubscription_does_not_accumulate_edges() {
        let mut scope = Scope::new();
        let sig = scope.signal(0i32);
        let sid = sig.id_for_tests();
        scope.effect(move || {
            let _ = sig.get();
        });
        for i in 1..=10 {
            sig.set(i);
        }
        ARENA.with(|a| {
            let a = a.borrow();
            assert_eq!(
                a.signals[sid.index as usize].subscribers.len(),
                1,
                "re-runs must not duplicate subscriber edges"
            );
        });
    }

    /// Regression: a freed index is reused, but a stale handle to the old
    /// generation must NOT alias the new occupant — it must panic. This is
    /// the guarantee that lets the arena reuse indices (bounded growth)
    /// without the aliasing hazard the original non-reuse design avoided.
    #[test]
    fn freed_index_is_reused_without_aliasing() {
        let first_id;
        {
            let mut scope = Scope::new();
            let s = scope.signal(1i32);
            first_id = s.id_for_tests();
        }
        let mut scope2 = Scope::new();
        let s2 = scope2.signal(2i32);
        let second_id = s2.id_for_tests();
        assert_eq!(first_id.index, second_id.index, "index should be reused from the free-list");
        assert_ne!(first_id.generation, second_id.generation, "generation must advance on reuse");

        let stale: Signal<i32> = Signal { id: first_id, _phantom: PhantomData };
        let result = catch_unwind(AssertUnwindSafe(|| stale.get()));
        assert!(result.is_err(), "stale handle must panic, never alias the reused slot");
        assert_eq!(s2.get(), 2);
    }

    /// Regression: a panic inside an effect body must not permanently disable
    /// the effect. The RAII guard returns the closure to the arena even on
    /// unwind, so the next relevant write re-runs it.
    #[test]
    fn effect_closure_restored_after_panic() {
        let mut scope = Scope::new();
        let trigger = scope.signal(0i32);
        let runs = Rc::new(Cell::new(0));
        let should_panic = Rc::new(Cell::new(false));
        let r = runs.clone();
        let p = should_panic.clone();
        scope.effect(move || {
            let _ = trigger.get();
            r.set(r.get() + 1);
            if p.get() {
                panic!("boom");
            }
        });
        assert_eq!(runs.get(), 1, "initial run");

        should_panic.set(true);
        let result = catch_unwind(AssertUnwindSafe(|| trigger.set(1)));
        assert!(result.is_err(), "the panic should propagate to the writer");
        assert_eq!(runs.get(), 2, "effect ran before panicking");

        // Closure must have been restored despite the panic.
        should_panic.set(false);
        trigger.set(2);
        assert_eq!(runs.get(), 3, "effect re-ran after the panic — closure was restored");
    }

    /// A signal that writes the very signal it reads must not recurse
    /// infinitely: the reentrant re-run is skipped because the effect's
    /// closure is checked out while it runs.
    #[test]
    fn self_writing_effect_does_not_recurse() {
        let mut scope = Scope::new();
        let sig = scope.signal(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        scope.effect(move || {
            let v = sig.get();
            r.set(r.get() + 1);
            if v < 1 {
                sig.set(v + 1); // reentrant write — must not loop forever
            }
        });
        // Initial run reads 0, writes 1 (reentrant run skipped). A real
        // re-run happens for the write, reads 1, does not write again.
        assert!(runs.get() >= 1, "effect ran at least once without hanging");
        assert_eq!(sig.get(), 1);
    }

    #[test]
    fn dispose_frees_unscoped_signal() {
        let s = Signal::new(5i32);
        let id = s.id_for_tests();
        assert_eq!(s.get(), 5);
        s.dispose();
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.signals[id.index as usize].value.is_none(), "dispose should free the slot");
        });
    }

    #[test]
    fn dispose_frees_unscoped_effect() {
        let mut scope = Scope::new();
        let sig = scope.signal(0i32);
        let sid = sig.id_for_tests();
        let eid = effect(move || {
            let _ = sig.get();
        });
        ARENA.with(|a| {
            assert_eq!(a.borrow().signals[sid.index as usize].subscribers.len(), 1);
        });
        dispose_effect(eid);
        ARENA.with(|a| {
            assert!(
                a.borrow().signals[sid.index as usize].subscribers.is_empty(),
                "disposing an unscoped effect must detach it from its signals"
            );
        });
    }
}
