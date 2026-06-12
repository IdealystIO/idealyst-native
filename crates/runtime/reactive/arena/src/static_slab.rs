#![forbid(unsafe_code)] // machine-proof: this whole module compiles with no `unsafe`.
//! Hypothetical: a **downcast-free**, `Copy`-handle signal arena.
//!
//! This is a sibling prototype to [`crate`] (the type-erased arena). It proves
//! the production arena's per-access `downcast_ref::<SignalInner<T>>()` can be
//! eliminated *without* giving up `Signal<T>: Copy`, *without* a viral
//! lifetime, and — the surprising part — *without any `unsafe`*.
//!
//! # The idea
//!
//! Storage never deallocates. Each signal's slot is `Box::leak`ed once to
//! obtain a `&'static SignalSlot<T>`. That reference is itself `Copy`, is
//! statically typed (so reading the value is a plain field access, never a
//! downcast), and carries no *generic* lifetime — `'static` is a concrete
//! lifetime, not a parameter on `Signal<T>`, so the handle stays `Copy` and
//! un-parameterised exactly like today's. Slots are recycled through per-type
//! free lists, so memory is bounded by *peak-concurrent* signals (same profile
//! as the production `Vec`-arena, which also never shrinks). A generation tag
//! on each slot turns a stale handle into a clean no-op.
//!
//! # Why the `unsafe`-free version is sound
//!
//! The only thing that makes `&'static` legal here is that the backing
//! allocation is **never freed**. Three invariants, by construction:
//!
//! 1. **Never freed.** A leaked `Box` lives for the whole thread; its address
//!    is valid forever. Use-after-free is impossible, period.
//! 2. **Type-stable.** A slot allocated as `SignalSlot<T>` is only ever reused
//!    for another `T` (free lists are keyed by `TypeId::of::<T>()`). A
//!    `Signal<T>` therefore always references a `SignalSlot<T>`. There is no
//!    type confusion to guard against — *this is what replaces the downcast*.
//! 3. **Shared-only + interior mutability.** Handles hold `&'static` *shared*
//!    refs; all mutation goes through the `Cell`/`RefCell` inside the slot.
//!    Many `Copy` handles aliasing one slot is just many shared refs — always
//!    legal. We never form a `&mut SignalSlot<T>`.
//!
//! Note what is *not* in that list: the generation tag. Memory safety holds
//! regardless of the generation — the worst a bug in the generation logic can
//! do is return a stale *value*, never UB. The generation is pure correctness,
//! not a safety mechanism. That separation is the whole reason this is
//! comfortable to reason about.
//!
//! # The downcast that remains (and why it doesn't matter)
//!
//! Allocating/disposing a signal still looks up its per-type [`Store`] in a
//! `TypeId`-keyed map and downcasts the *bucket* once. That is the **cold
//! path** — it happens on `new`/`dispose`, which already touch a map. The
//! **hot path** (`get`/`set`/`update`, run on every reactive read and write)
//! is now a pointer-deref + generation compare + interior-mutability access,
//! with no `dyn Any` and no `downcast` anywhere in sight.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

// ----------------------------------------------------------------------------
// Storage
// ----------------------------------------------------------------------------

/// One signal's backing storage. Lives at a stable `'static` address (leaked
/// `Box`), reused via the per-type free list. `value` is `Option<T>` so a
/// disposed slot drops its `T` *promptly* (at dispose, not at the next reuse).
struct SignalSlot<T: 'static> {
    /// Bumped on every dispose. A handle records the generation it was minted
    /// with; a mismatch means the slot was disposed (and possibly recycled),
    /// so the handle is stale.
    generation: Cell<u32>,
    /// Lazily-assigned dense `u32` identity, minted from the global
    /// [`IdAllocator`] the first time this signal is *externalized* (handed to
    /// a JS/Roku binding via [`Signal::id`]). `0` = "no external id yet". Freed
    /// back to the allocator at dispose, *after* the client binding is released
    /// — so a recycled id can never resolve to a stale binding. Lives on the
    /// slot, not in a side map, so the two identities cannot drift.
    id: Cell<u32>,
    value: RefCell<Option<T>>,
}

/// Per-type recycle pool. Holds `'static` slots whose previous occupant was
/// disposed, ready to be re-initialised for a new `Signal<T>`.
struct Store<T: 'static> {
    free: Vec<&'static SignalSlot<T>>,
}

thread_local! {
    /// `TypeId::of::<T>()` -> `Box<Store<T>>`. Touched only on `new`/`dispose`
    /// (the cold path), never on value access.
    static STORES: RefCell<HashMap<TypeId, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// Runs `f` against the `Store<T>`, creating it on first use. The single
/// `downcast_mut` here is keyed by `TypeId::of::<T>()`, so it can never fail —
/// the bucket for that key is always a `Store<T>`.
fn with_store<T: 'static, R>(f: impl FnOnce(&mut Store<T>) -> R) -> R {
    STORES.with(|cell| {
        let mut map = cell.borrow_mut();
        let entry = map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Store::<T> { free: Vec::new() }) as Box<dyn Any>);
        let store = entry
            .downcast_mut::<Store<T>>()
            .expect("store bucket is keyed by TypeId::of::<T>(), so this downcast is infallible");
        f(store)
    })
}

// ----------------------------------------------------------------------------
// Externalized identity (the "id shim")
// ----------------------------------------------------------------------------
//
// Two backends (web's JS bindings, Roku's wire) need a *dense `u32`* signal id,
// not a 64-bit pointer. We mint one lazily — only for signals that are actually
// externalized — and store it ON the slot, so there is no parallel `id -> slot`
// map to keep in sync: the id is born and freed together with its slot.
//
// Nothing ever resolves an id *back* to a slot. The handle always carries the
// pointer, and a notifier closure captures the typed handle. So the only global
// state here is the dense-id *allocator* (a counter + freelist) and the notifier
// table keyed by id — both touched only on the externalized cold path. This is
// the same recycle+release contract production already ships; the only new
// discipline is that `dispose` frees two things (slot + id) instead of one.

/// Dense `u32` id allocator. `0` is reserved as the "no id yet" sentinel, so
/// fresh ids start at 1. Freed ids are recycled LIFO, keeping the space dense.
struct IdAllocator {
    next: u32,
    free: Vec<u32>,
}

impl IdAllocator {
    fn alloc(&mut self) -> u32 {
        self.free.pop().unwrap_or_else(|| {
            let id = self.next;
            self.next += 1;
            id
        })
    }
    fn free(&mut self, id: u32) {
        if id != 0 {
            self.free.push(id);
        }
    }
}

thread_local! {
    static IDS: RefCell<IdAllocator> = RefCell::new(IdAllocator { next: 1, free: Vec::new() });
    /// Client-side change notifiers, keyed by externalized signal id. Models
    /// the production `signal_js_notifiers` map.
    static NOTIFIERS: RefCell<HashMap<u32, Rc<dyn Fn()>>> = RefCell::new(HashMap::new());
}

// ----------------------------------------------------------------------------
// Signal<T>
// ----------------------------------------------------------------------------

/// A `Copy` handle to a reactive value. Holds a *typed* `'static` reference to
/// its slot, so reads and writes are direct field accesses — no `dyn Any`, no
/// downcast on the hot path.
pub struct Signal<T: 'static> {
    slot: &'static SignalSlot<T>,
    generation: u32,
}

impl<T: 'static> Copy for Signal<T> {}
impl<T: 'static> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Signal<T> {
    /// Creates a signal not owned by any scope (free it with [`Signal::dispose`]
    /// or hand to a [`Scope`]). Reuses a recycled slot if one is available,
    /// else leaks a fresh one.
    pub fn new(value: T) -> Self {
        with_store::<T, _>(|store| {
            if let Some(slot) = store.free.pop() {
                // Recycle: same address, same type. `generation` was already
                // bumped at dispose, so old handles to this slot stay stale.
                slot.id.set(0); // defensive: a recycled slot carries no external id
                *slot.value.borrow_mut() = Some(value);
                Signal { slot, generation: slot.generation.get() }
            } else {
                let slot: &'static SignalSlot<T> = Box::leak(Box::new(SignalSlot {
                    generation: Cell::new(0),
                    id: Cell::new(0),
                    value: RefCell::new(Some(value)),
                }));
                Signal { slot, generation: 0 }
            }
        })
    }

    /// The slot's address — a stable, type-erased identity used as the key in
    /// the dependency graph (effects subscribe by address).
    fn addr(&self) -> usize {
        self.slot as *const SignalSlot<T> as usize
    }

    /// True while this handle's generation still matches the slot.
    fn is_live(&self) -> bool {
        self.slot.generation.get() == self.generation
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        assert!(self.is_live(), "signal used after dispose (generation mismatch)");
        // Auto-subscribe the running effect, if any.
        if let Some(eid) = CURRENT.with(|c| c.get()) {
            let addr = self.addr();
            GRAPH.with(|g| g.borrow_mut().subscribe(addr, eid));
        }
        // The whole point: a direct, typed read. No downcast.
        self.slot
            .value
            .borrow()
            .as_ref()
            .expect("a live slot always holds Some(value)")
            .clone()
    }

    pub fn set(&self, value: T) {
        // Stale write (slot disposed/recycled) → no-op, never aliases the new
        // occupant. Mirrors production's generational guard.
        if !self.is_live() {
            return;
        }
        *self.slot.value.borrow_mut() = Some(value); // borrow released at `;`
        self.fan_out();
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        if !self.is_live() {
            return;
        }
        {
            let mut v = self.slot.value.borrow_mut();
            f(v.as_mut().expect("a live slot always holds Some(value)"));
        } // borrow released before fan-out
        self.fan_out();
    }

    fn fan_out(&self) {
        let addr = self.addr();
        // Clone the subscriber list out, drop the graph borrow, then run — so
        // an effect re-entering the graph (reading/writing signals) is safe.
        let to_run = GRAPH.with(|g| {
            g.borrow().subscribers.get(&addr).cloned().unwrap_or_default()
        });
        for eid in to_run {
            run_effect(eid);
        }
        // External (JS/Roku) notifier — fires AFTER the Rust subscriber
        // fan-out, mirroring production's `signal_js_notifiers` ordering. Only
        // an externalized signal (one with a minted id) pays the lookup.
        let id = self.slot.id.get();
        if id != 0 {
            if let Some(n) = NOTIFIERS.with(|n| n.borrow().get(&id).cloned()) {
                n();
            }
        }
    }

    /// The signal's **externalized dense `u32` identity**, minted lazily from
    /// the global [`IdAllocator`] on first call. This is the value that crosses
    /// to JS (as a `Uint32Array` element) or to the Roku wire. Signals that are
    /// never externalized never mint an id — the common case pays nothing.
    /// Returns `0` for a stale handle.
    pub fn id(&self) -> u32 {
        if !self.is_live() {
            return 0;
        }
        let cur = self.slot.id.get();
        if cur != 0 {
            return cur;
        }
        let fresh = IDS.with(|a| a.borrow_mut().alloc());
        self.slot.id.set(fresh);
        fresh
    }

    /// Registers a client-side change notifier (models the web backend's
    /// `register_signal_js_notifier`): mints an id if needed, then keys the
    /// notifier by that id. Fired after the Rust subscriber fan-out on every
    /// change, and removed at dispose *before* the id is recycled.
    pub fn register_external_notifier(&self, f: impl Fn() + 'static) {
        let id = self.id();
        if id == 0 {
            return; // stale handle — nothing to bind
        }
        NOTIFIERS.with(|n| {
            n.borrow_mut().insert(id, Rc::new(f) as Rc<dyn Fn()>);
        });
    }

    /// Disposes the slot: bumps the generation (invalidating every `Copy` of
    /// this handle), drops the stored `T` now, releases any external binding,
    /// and returns the slot + id to their free lists for reuse. Idempotent — a
    /// stale handle's dispose is a no-op.
    pub fn dispose(self) {
        if !self.is_live() {
            return;
        }
        self.slot.generation.set(self.generation.wrapping_add(1));
        *self.slot.value.borrow_mut() = None; // drop T promptly

        // Externalized-id teardown — ORDER MATTERS. Remove the client binding
        // (the notifier) BEFORE returning the id to the allocator, so a recycled
        // id can never be handed to a new signal while a stale binding still
        // keys it. This `remove` is the analogue of the backend's
        // `release_reactive_text_binding`.
        let id = self.slot.id.replace(0);
        if id != 0 {
            NOTIFIERS.with(|n| {
                n.borrow_mut().remove(&id);
            });
            IDS.with(|a| a.borrow_mut().free(id)); // recycle only AFTER release
        }

        let addr = self.addr();
        // Drop any lingering subscriber entry so a future signal that recycles
        // this address can't inherit this signal's subscribers.
        GRAPH.with(|g| {
            g.borrow_mut().subscribers.remove(&addr);
        });
        with_store::<T, _>(|store| store.free.push(self.slot));
    }
}

// ----------------------------------------------------------------------------
// Effects + dependency graph (type-erased by *address*, never by value type)
// ----------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct EffectId(u32);

struct EffectSlot {
    /// Checked out (`None`) while the effect runs — re-entry is skipped.
    run: Option<Box<dyn FnMut()>>,
    alive: bool,
    /// Signal addresses this effect read on its last run.
    deps: Vec<usize>,
}

struct Graph {
    effects: Vec<EffectSlot>,
    free: Vec<u32>,
    /// signal address -> effects that read it.
    subscribers: HashMap<usize, Vec<EffectId>>,
}

impl Graph {
    fn new() -> Self {
        Self { effects: Vec::new(), free: Vec::new(), subscribers: HashMap::new() }
    }

    fn subscribe(&mut self, addr: usize, eid: EffectId) {
        let subs = self.subscribers.entry(addr).or_default();
        if !subs.contains(&eid) {
            subs.push(eid);
        }
        if let Some(slot) = self.effects.get_mut(eid.0 as usize) {
            if !slot.deps.contains(&addr) {
                slot.deps.push(addr);
            }
        }
    }

    fn clear_deps(&mut self, eid: EffectId) {
        let deps = match self.effects.get_mut(eid.0 as usize) {
            Some(slot) => std::mem::take(&mut slot.deps),
            None => return,
        };
        for addr in deps {
            if let Some(subs) = self.subscribers.get_mut(&addr) {
                subs.retain(|e| *e != eid);
            }
        }
    }
}

thread_local! {
    static CURRENT: Cell<Option<EffectId>> = const { Cell::new(None) };
    static GRAPH: RefCell<Graph> = RefCell::new(Graph::new());
}

/// Creates an effect and runs it once, capturing its dependency set.
pub fn effect<F: FnMut() + 'static>(f: F) -> EffectId {
    let id = GRAPH.with(|g| {
        let mut g = g.borrow_mut();
        if let Some(idx) = g.free.pop() {
            g.effects[idx as usize] = EffectSlot { run: Some(Box::new(f)), alive: true, deps: Vec::new() };
            EffectId(idx)
        } else {
            let idx = g.effects.len() as u32;
            g.effects.push(EffectSlot { run: Some(Box::new(f)), alive: true, deps: Vec::new() });
            EffectId(idx)
        }
    });
    run_effect(id);
    id
}

/// Frees an effect, detaching it from every signal it read.
pub fn dispose_effect(id: EffectId) {
    GRAPH.with(|g| g.borrow_mut().clear_deps(id));
    GRAPH.with(|g| {
        let mut g = g.borrow_mut();
        let alive = g.effects.get(id.0 as usize).map(|s| s.alive).unwrap_or(false);
        if alive {
            let slot = &mut g.effects[id.0 as usize];
            slot.alive = false;
            slot.run = None;
            slot.deps.clear();
            g.free.push(id.0);
        }
    });
}

fn run_effect(eid: EffectId) {
    let taken = GRAPH.with(|g| {
        let mut g = g.borrow_mut();
        match g.effects.get_mut(eid.0 as usize) {
            Some(slot) if slot.alive => slot.run.take(), // None => already running => skip
            _ => None,
        }
    });
    let Some(run) = taken else { return };

    GRAPH.with(|g| g.borrow_mut().clear_deps(eid)); // recapture a fresh dep set
    let prev = CURRENT.with(|c| c.replace(Some(eid)));

    /// Restores `CURRENT` and returns the closure to the arena even on panic.
    struct Guard {
        eid: EffectId,
        run: Option<Box<dyn FnMut()>>,
        prev: Option<EffectId>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT.with(|c| c.set(self.prev));
            if let Some(run) = self.run.take() {
                GRAPH.with(|g| {
                    let mut g = g.borrow_mut();
                    if let Some(slot) = g.effects.get_mut(self.eid.0 as usize) {
                        if slot.alive {
                            slot.run = Some(run);
                        }
                    }
                });
            }
        }
    }

    let mut guard = Guard { eid, run: Some(run), prev };
    (guard.run.as_mut().expect("guard holds the closure during the run"))();
}

// ----------------------------------------------------------------------------
// Scope
// ----------------------------------------------------------------------------

/// Owns signals and effects; dropping it disposes them all.
#[derive(Default)]
pub struct Scope {
    cleanups: Vec<Box<dyn FnOnce()>>,
    effects: Vec<EffectId>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn signal<T: 'static>(&mut self, value: T) -> Signal<T> {
        let s = Signal::new(value);
        // A real framework would use a zero-alloc, fn-pointer reclaimer here;
        // a boxed closure keeps the prototype readable. `s` is Copy.
        self.cleanups.push(Box::new(move || s.dispose()));
        s
    }

    pub fn effect<F: FnMut() + 'static>(&mut self, f: F) -> EffectId {
        let id = effect(f);
        self.effects.push(id);
        id
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        // Effects first so their back-edges are pruned before the signals
        // they read are recycled.
        for id in self.effects.drain(..) {
            dispose_effect(id);
        }
        for c in self.cleanups.drain(..) {
            c();
        }
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;

    #[test]
    fn signal_is_copy_and_typed() {
        let mut scope = Scope::new();
        let s = scope.signal(7i32);
        let a = s;
        let b = s; // Copy, no clone ceremony
        assert_eq!(a.get(), 7);
        assert_eq!(b.get(), 7);
        s.set(42);
        assert_eq!(a.get(), 42);
        assert_eq!(b.get(), 42);
    }

    /// Heterogeneous value types coexist with zero downcast on access — each
    /// `Signal<T>` reaches a statically-typed `SignalSlot<T>`.
    #[test]
    fn heterogeneous_types_coexist() {
        let mut scope = Scope::new();
        let n = scope.signal(1u8);
        let s = scope.signal(String::from("hi"));
        #[derive(Clone, PartialEq, Debug)]
        struct Custom {
            x: i64,
        }
        let c = scope.signal(Custom { x: -5 });
        assert_eq!(n.get(), 1);
        assert_eq!(s.get(), "hi");
        assert_eq!(c.get(), Custom { x: -5 });
        s.set("bye".into());
        assert_eq!(s.get(), "bye");
    }

    #[test]
    fn effect_fires_on_change() {
        let mut scope = Scope::new();
        let count = scope.signal(0i32);
        let observed = Rc::new(Cell::new(-1));
        let o = observed.clone();
        scope.effect(move || o.set(count.get()));
        assert_eq!(observed.get(), 0);
        count.set(5);
        assert_eq!(observed.get(), 5);
        count.set(9);
        assert_eq!(observed.get(), 9);
    }

    /// A freed slot's *address* is reused, but its generation advances — so a
    /// stale handle reads as dead and can never alias the new occupant.
    #[test]
    fn stale_handle_never_aliases_recycled_slot() {
        let first_addr;
        let stale: Signal<i64>;
        {
            let mut scope = Scope::new();
            let s = scope.signal(100i64);
            first_addr = s.addr();
            stale = s; // Copy that will outlive the scope
        } // scope drop disposes `s`, recycling its slot

        let mut scope2 = Scope::new();
        let fresh = scope2.signal(200i64);
        assert_eq!(fresh.addr(), first_addr, "slot address should be recycled");
        assert!(!stale.is_live(), "stale handle must not be live");

        // Stale writes/reads do not touch the new occupant.
        stale.set(999);
        assert_eq!(fresh.get(), 200, "stale set must not reach the recycled slot");
    }

    #[test]
    fn scope_drop_disposes_signals_and_effects() {
        let count;
        {
            let mut scope = Scope::new();
            count = scope.signal(0i32);
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            scope.effect(move || {
                let _ = count.get();
                r.set(r.get() + 1);
            });
            assert!(count.is_live());
        }
        assert!(!count.is_live(), "scope drop must dispose its signals");
    }

    /// A self-writing effect must not recurse forever: the re-entrant run is
    /// skipped because the effect's closure is checked out while it runs.
    #[test]
    fn self_writing_effect_terminates() {
        let mut scope = Scope::new();
        let sig = scope.signal(0i32);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        scope.effect(move || {
            let v = sig.get();
            r.set(r.get() + 1);
            if v < 1 {
                sig.set(v + 1); // re-entrant write — must be skipped, not loop
            }
        });
        assert_eq!(sig.get(), 1);
        assert!(runs.get() >= 1);
    }

    /// Dynamic dependencies: an effect that reads `a` xor `b` depending on a
    /// flag must only re-run for the signal it actually read.
    #[test]
    fn dynamic_dependencies_recapture() {
        let mut scope = Scope::new();
        let flag = scope.signal(true);
        let a = scope.signal(10i32);
        let b = scope.signal(20i32);
        let seen = Rc::new(Cell::new(0));
        let s = seen.clone();
        scope.effect(move || {
            let v = if flag.get() { a.get() } else { b.get() };
            s.set(v);
        });
        assert_eq!(seen.get(), 10);
        b.set(99); // not a dep yet → no re-run
        assert_eq!(seen.get(), 10);
        flag.set(false); // re-runs, now reads b
        assert_eq!(seen.get(), 99);
        a.set(11); // a is no longer a dep → no re-run
        assert_eq!(seen.get(), 99);
        b.set(7);
        assert_eq!(seen.get(), 7);
    }

    /// A panic in an effect body restores the checked-out closure (RAII), so a
    /// later write re-runs the effect rather than finding it permanently dead.
    #[test]
    fn effect_closure_restored_after_panic() {
        let mut scope = Scope::new();
        let trigger = scope.signal(0i32);
        let runs = Rc::new(Cell::new(0));
        let boom = Rc::new(Cell::new(false));
        let r = runs.clone();
        let p = boom.clone();
        scope.effect(move || {
            let _ = trigger.get();
            r.set(r.get() + 1);
            if p.get() {
                panic!("boom");
            }
        });
        assert_eq!(runs.get(), 1);
        boom.set(true);
        let res = catch_unwind(AssertUnwindSafe(|| trigger.set(1)));
        assert!(res.is_err());
        assert_eq!(runs.get(), 2);
        boom.set(false);
        trigger.set(2);
        assert_eq!(runs.get(), 3, "closure was restored despite the panic");
    }

    /// The full externalized-id lifecycle: mint a dense id, fire the client
    /// notifier on change, then dispose — which releases the binding *and*
    /// recycles the id. A new signal reuses that id, and the OLD binding must
    /// never fire for it: the release brackets the reuse.
    #[test]
    fn externalized_id_lifecycle_brackets_reuse() {
        let fired_a = Rc::new(Cell::new(0));
        let id_a;
        {
            let sig = Signal::new(0i32);
            let fa = fired_a.clone();
            sig.register_external_notifier(move || fa.set(fa.get() + 1));
            id_a = sig.id();
            assert!(id_a >= 1, "an externalized signal mints a dense id");
            sig.set(1);
            assert_eq!(fired_a.get(), 1, "client notifier fires on change");
            sig.dispose(); // releases the binding, THEN recycles the id
        }

        // The freed id is densely recycled by the next externalized signal.
        let fired_b = Rc::new(Cell::new(0));
        let sig2 = Signal::new(100i32);
        let fb = fired_b.clone();
        sig2.register_external_notifier(move || fb.set(fb.get() + 1));
        let id_b = sig2.id();
        assert_eq!(id_b, id_a, "freed id is recycled (the space stays dense)");

        sig2.set(2);
        assert_eq!(fired_b.get(), 1, "the new binding fires");
        assert_eq!(
            fired_a.get(),
            1,
            "the OLD binding never fires for the recycled id — release bracketed the reuse"
        );
        sig2.dispose();
    }

    /// A signal that is never externalized never mints an id; a stale handle
    /// reports id `0`.
    #[test]
    fn non_externalized_signal_mints_no_id() {
        let mut scope = Scope::new();
        let s = scope.signal(5i32);
        assert_eq!(s.slot.id.get(), 0, "no id until externalized");
        let _ = s.get();
        s.set(6);
        assert_eq!(s.slot.id.get(), 0, "plain reads/writes don't externalize");

        let dangling = Signal::new(1i32);
        let real_id = dangling.id();
        assert!(real_id >= 1);
        dangling.dispose();
        assert_eq!(dangling.id(), 0, "stale handle reports id 0");
    }

    /// Recycling keeps the value type stable, so reuse never type-confuses a
    /// slot. (Disposing many i32s then allocating i32s reuses addresses.)
    #[test]
    fn recycle_is_type_stable_and_bounded() {
        let mut addrs = Vec::new();
        for _ in 0..3 {
            let s = Signal::new(1i32);
            addrs.push(s.addr());
            s.dispose();
        }
        // All three reused the same recycled slot (bounded by peak = 1).
        assert!(addrs.windows(2).all(|w| w[0] == w[1]), "one signal at a time reuses one slot");
    }
}
