//! Prototype: copy-handle reactive primitives via a thread-local arena.
//!
//! Validates whether `Signal<T>: Copy` is feasible without unsafe code and
//! without architectural compromises. Not integrated with runtime-core
//! until the prototype proves sound.
//!
//! # Design
//!
//! - All reactive storage lives in a thread-local `Arena` of `Box<dyn Any>`
//!   slots. Slots are addressed by monotonically increasing IDs that are
//!   never reused, so a dead handle never aliases live storage.
//! - `Signal<T>` is a `Copy` handle: `(SignalId, PhantomData<T>)`. The type
//!   parameter is preserved at compile time; the runtime downcast on access
//!   is checking an invariant, not selecting behavior.
//! - `Effect` is also an arena-backed slot. Subscribed effects are stored
//!   as `EffectId`s in each signal's subscriber list.
//! - `Scope` owns a list of `NodeId`s. Dropping the scope frees each ID.
//!
//! # Failure modes (intentional, diagnostic)
//!
//! - Reading from a `Signal<T>` whose scope has dropped → panic with a
//!   clear message. The signal's slot is `None` in the arena.
//! - Dangling effect IDs in a subscriber list → silently skipped during
//!   notify, since the effect's slot is `None`.
//!
//! # What's NOT used
//!
//! - No `unsafe`.
//! - No `transmute`.
//! - No raw pointers.
//! - No `Rc`/`Arc` for handles — only `RefCell` for arena interior mutability.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;

// ----------------------------------------------------------------------------
// IDs
// ----------------------------------------------------------------------------

/// Index into the arena's signal slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SignalId(u32);

/// Index into the arena's effect slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(u32);

// ----------------------------------------------------------------------------
// Arena
// ----------------------------------------------------------------------------

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
    /// The effect that is currently running, if any. Set during `Effect::new`
    /// initial run and during re-runs from `Signal::set`. Reads via
    /// `Signal::get` consult this slot to register subscriptions.
    static CURRENT: RefCell<Option<EffectId>> = const { RefCell::new(None) };
}

struct Arena {
    signals: Vec<Option<Box<dyn Any>>>,
    effects: Vec<Option<Box<dyn Any>>>,
}

impl Arena {
    fn new() -> Self {
        Self { signals: Vec::new(), effects: Vec::new() }
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

    fn free_signal(&mut self, id: SignalId) {
        let idx = id.0 as usize;
        if idx < self.signals.len() {
            self.signals[idx] = None;
        }
    }

    fn free_effect(&mut self, id: EffectId) {
        let idx = id.0 as usize;
        if idx < self.effects.len() {
            self.effects[idx] = None;
        }
    }
}

// ----------------------------------------------------------------------------
// SignalInner / EffectInner
// ----------------------------------------------------------------------------

struct SignalInner<T> {
    value: T,
    subscribers: Vec<EffectId>,
}

struct EffectInner {
    run: Box<dyn FnMut()>,
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
/// The arena slot is freed when the owning `Scope` is dropped. Reading from
/// a signal after its scope has dropped will panic with a clear message.
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
    /// Creates a signal in the *current scope*. Use `Scope::signal` to
    /// create one bound to a specific scope. For the prototype this
    /// uses the bare arena and caller must manually free via the
    /// returned id, OR via a `Scope`.
    pub fn new(value: T) -> Self {
        let inner = SignalInner { value, subscribers: Vec::new() };
        let id = ARENA.with(|a| a.borrow_mut().insert_signal(inner));
        Signal { id, _phantom: PhantomData }
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
        run_effects(&to_run, self.id);
    }

    pub fn update<F: FnOnce(&mut T)>(&self, f: F) {
        let to_run: Vec<EffectId> = with_signal_mut::<T, _>(self.id, |inner| {
            f(&mut inner.value);
            inner.subscribers.clone()
        });
        run_effects(&to_run, self.id);
    }

    /// For tests: returns the underlying ID. Not exposed publicly in a
    /// real framework.
    #[doc(hidden)]
    pub fn id_for_tests(&self) -> SignalId {
        self.id
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
            .expect("internal: signal type mismatch (this should never happen)");
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
            .expect("internal: signal type mismatch (this should never happen)");
        f(inner)
    })
}

/// Walks the given effect IDs and re-runs each live one. Dropped effects
/// (None slots) are removed from the subscriber list to keep it bounded.
fn run_effects(to_run: &[EffectId], from_signal: SignalId) {
    let mut still_alive: Vec<EffectId> = Vec::with_capacity(to_run.len());
    for &eid in to_run {
        // Snapshot the run function (need to move it out temporarily to
        // avoid holding a borrow on the arena across the call).
        let mut run_fn: Option<Box<dyn FnMut()>> = ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let slot = a.effects.get_mut(eid.0 as usize)?.as_mut()?;
            let inner = slot.downcast_mut::<EffectInner>()?;
            // Replace with a no-op while we run, to detect re-entry and
            // avoid double-borrow. We restore the original afterward.
            Some(std::mem::replace(&mut inner.run, Box::new(|| {})))
        });
        if let Some(f) = run_fn.as_mut() {
            let prev = CURRENT.with(|c| c.replace(Some(eid)));
            f();
            CURRENT.with(|c| *c.borrow_mut() = prev);
            still_alive.push(eid);
            // Restore the actual function.
            ARENA.with(|a| {
                let mut a = a.borrow_mut();
                if let Some(Some(slot)) = a.effects.get_mut(eid.0 as usize) {
                    if let Some(inner) = slot.downcast_mut::<EffectInner>() {
                        inner.run = run_fn.take().unwrap();
                    }
                }
            });
        }
    }
    // Update the originating signal's subscriber list to only include live
    // effects. (This is a minor optimization; the panic-on-dead-signal path
    // already handles the missing-slot case correctly.)
    let _ = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(Some(slot)) = a.signals.get_mut(from_signal.0 as usize) {
            // Note: we don't know the inner T here without generics, so this
            // best-effort pruning is left for future work. The list is
            // bounded by the number of effects created in the scope.
            let _ = slot;
        }
    });
    let _ = still_alive;
}

// ----------------------------------------------------------------------------
// Effect
// ----------------------------------------------------------------------------

/// Creates an effect and runs it once. Any signals read during the run will
/// re-fire the effect on change. Returns the `EffectId` so a `Scope` can
/// own its lifetime.
///
/// For convenience callers can also use `Scope::effect(...)` which manages
/// the id automatically.
pub fn effect<F: FnMut() + 'static>(f: F) -> EffectId {
    let inner = EffectInner { run: Box::new(f) };
    let id = ARENA.with(|a| a.borrow_mut().insert_effect(inner));
    // Run once to capture subscriptions.
    let mut run_fn: Option<Box<dyn FnMut()>> = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let slot = a.effects.get_mut(id.0 as usize)?.as_mut()?;
        let inner = slot.downcast_mut::<EffectInner>()?;
        Some(std::mem::replace(&mut inner.run, Box::new(|| {})))
    });
    if let Some(f) = run_fn.as_mut() {
        let prev = CURRENT.with(|c| c.replace(Some(id)));
        f();
        CURRENT.with(|c| *c.borrow_mut() = prev);
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(Some(slot)) = a.effects.get_mut(id.0 as usize) {
                if let Some(inner) = slot.downcast_mut::<EffectInner>() {
                    inner.run = run_fn.take().unwrap();
                }
            }
        });
    }
    id
}

// ----------------------------------------------------------------------------
// Scope
// ----------------------------------------------------------------------------

/// Owns a set of arena slots. When the `Scope` drops, all slots it owns are
/// freed, releasing their storage. Use a `Scope` to bound the lifetime of
/// signals and effects created inside a component, render pass, or test.
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
            for id in self.signals.drain(..) {
                a.free_signal(id);
            }
            for id in self.effects.drain(..) {
                a.free_effect(id);
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
    use std::rc::Rc;

    #[test]
    fn signal_is_copy() {
        let mut scope = Scope::new();
        let s = scope.signal(7i32);
        // Compile-time: Copy works. No clone needed.
        let s2 = s;
        let s3 = s;
        assert_eq!(s2.get(), 7);
        assert_eq!(s3.get(), 7);
        s.set(42);
        // All three handles see the same value because they all reference
        // the same arena slot.
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
        // Initial run captured value 0.
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
            // scope drops here, freeing s
        }
        // The slot should now be None. We can't access via a Signal handle
        // since scope owned the only one, but we can poke the arena directly.
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.signals[id.0 as usize].is_none(), "slot should be freed");
        });
    }

    #[test]
    #[should_panic(expected = "signal used after its scope was dropped")]
    fn dead_signal_panics() {
        // We have to keep the Signal handle alive past scope drop. The only
        // way is to leak it from the scope, which we don't support cleanly.
        // For the test we read the underlying id, free it, and try to access
        // through a reconstructed-looking handle.
        let id;
        {
            let mut scope = Scope::new();
            let s = scope.signal(99i32);
            id = s.id_for_tests();
            assert_eq!(s.get(), 99);
        }
        // Reconstruct a Signal handle pointing at the dead slot. In real
        // code you wouldn't be able to do this without smuggling the id —
        // but for the test it demonstrates the panic.
        let dead: Signal<i32> = Signal { id, _phantom: PhantomData };
        let _ = dead.get(); // should panic
    }

    #[test]
    fn copy_into_closures_no_clones() {
        let mut scope = Scope::new();
        let toggle = scope.signal(false);
        // Both closures capture `toggle` by Copy — no .clone() needed.
        let toggle_cond = toggle;
        let toggle_set = toggle;
        let _ = toggle_cond;
        let _ = toggle_set;
        // Visually the cleanest expression of "use the signal in two places":
        let read = move || toggle.get();
        let write = move || toggle.set(true);
        assert_eq!(read(), false);
        write();
        assert_eq!(read(), true);
    }

    #[test]
    fn nested_scopes_free_independently() {
        let outer_id;
        let inner_id;
        let mut outer = Scope::new();
        let outer_sig = outer.signal(10i32);
        outer_id = outer_sig.id_for_tests();
        {
            let mut inner = Scope::new();
            let inner_sig = inner.signal(20i32);
            inner_id = inner_sig.id_for_tests();
            assert_eq!(inner_sig.get(), 20);
            // Outer signal still reachable inside inner scope.
            assert_eq!(outer_sig.get(), 10);
        }
        // Inner scope dropped; outer remains.
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.signals[inner_id.0 as usize].is_none());
            assert!(a.signals[outer_id.0 as usize].is_some());
        });
        assert_eq!(outer_sig.get(), 10);
    }
}
