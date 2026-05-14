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
//! - Subscriber lists may contain `EffectId`s pointing at freed slots;
//!   these are silently skipped during `notify`.

use std::any::Any;
use std::cell::RefCell;
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
}

struct Arena {
    signals: Vec<Option<Box<dyn Any>>>,
    effects: Vec<Option<Box<dyn Any>>>,
    /// Outer `Option`: `None` once the slot is freed by its owning scope.
    /// Inner `Option<Box<dyn Any>>`: `None` while the ref exists but hasn't
    /// been filled by a mount yet; `Some` once mounted.
    refs: Vec<Option<Option<Box<dyn Any>>>>,
}

impl Arena {
    fn new() -> Self {
        Self {
            signals: Vec::new(),
            effects: Vec::new(),
            refs: Vec::new(),
        }
    }

    fn insert_signal<T: 'static>(&mut self, inner: SignalInner<T>) -> SignalId {
        let id = SignalId(self.signals.len() as u32);
        self.signals.push(Some(Box::new(inner)));
        id
    }

    fn insert_effect(&mut self, inner: EffectInner) -> EffectId {
        let id = EffectId(self.effects.len() as u32);
        self.effects.push(Some(Box::new(inner)));
        id
    }

    fn insert_ref(&mut self) -> RefId {
        let id = RefId(self.refs.len() as u32);
        self.refs.push(Some(None));
        id
    }

    fn free_signal(&mut self, id: SignalId) {
        if let Some(slot) = self.signals.get_mut(id.0 as usize) {
            *slot = None;
        }
    }

    fn free_effect(&mut self, id: EffectId) {
        if let Some(slot) = self.effects.get_mut(id.0 as usize) {
            *slot = None;
        }
    }

    fn free_ref(&mut self, id: RefId) {
        if let Some(slot) = self.refs.get_mut(id.0 as usize) {
            *slot = None;
        }
    }
}

struct SignalInner<T> {
    value: T,
    subscribers: Vec<EffectId>,
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
            a.borrow_mut().insert_signal(SignalInner {
                value,
                subscribers: Vec::new(),
            })
        });
        register_signal(id);
        Self { id, _phantom: PhantomData }
    }

    pub fn get(&self) -> T {
        // Record subscription if an effect is currently running.
        CURRENT.with(|c| {
            if let Some(eid) = *c.borrow() {
                with_signal_mut::<T, _>(self.id, |inner| {
                    if !inner.subscribers.contains(&eid) {
                        inner.subscribers.push(eid);
                    }
                });
            }
        });
        with_signal::<T, _>(self.id, |inner| inner.value.clone())
    }

    pub fn set(&self, value: T) {
        let to_run: Vec<EffectId> = with_signal_mut::<T, _>(self.id, |inner| {
            inner.value = value;
            inner.subscribers.clone()
        });
        run_effects(&to_run);
        prune_subscribers::<T>(self.id);
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        let to_run: Vec<EffectId> = with_signal_mut::<T, _>(self.id, |inner| {
            f(&mut inner.value);
            inner.subscribers.clone()
        });
        run_effects(&to_run);
        prune_subscribers::<T>(self.id);
    }
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

/// After running a signal's effects, prune any subscriber IDs that point
/// at freed effect slots. Keeps subscriber lists bounded over time.
fn prune_subscribers<T: 'static>(sig: SignalId) {
    let live: Vec<EffectId> = with_signal::<T, _>(sig, |inner| inner.subscribers.clone());
    let kept: Vec<EffectId> = live
        .into_iter()
        .filter(|eid| {
            ARENA.with(|a| {
                a.borrow()
                    .effects
                    .get(eid.0 as usize)
                    .and_then(|o| o.as_ref())
                    .is_some()
            })
        })
        .collect();
    with_signal_mut::<T, _>(sig, |inner| inner.subscribers = kept);
}

// =============================================================================
// Effect
// =============================================================================

/// Handle to a reactive effect. Drop it to stop the effect from re-running.
///
/// The handle owns the effect's slot in the arena; dropping the handle
/// frees the slot. Existing subscriber references in signals are
/// silently skipped on the next notify pass and pruned.
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
}

impl Scope {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self { signals: Vec::new(), effects: Vec::new(), refs: Vec::new() }
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
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            for id in self.signals.drain(..) {
                a.free_signal(id);
            }
            for id in self.effects.drain(..) {
                a.free_effect(id);
            }
            for id in self.refs.drain(..) {
                a.free_ref(id);
            }
        });
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
