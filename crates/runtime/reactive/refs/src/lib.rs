//! Prototype: first-class component refs.
//!
//! Validates the `Ref<H>` design — a Copy-handle that addresses an arena
//! slot which is filled at component mount and cleared at unmount. The
//! parent of a component stashes the ref before mount and calls methods
//! on the filled handle afterward.
//!
//! ## Why a separate crate
//!
//! Same reason `reactive-arena` exists: the production `Signal<T>` arena
//! and the planned `Ref<H>` arena could share machinery, but until the
//! API is stable we want to iterate without breaking runtime-core.
//!
//! ## What we're proving
//!
//! 1. A single `Ref<H>` API works for *both* primitive handles
//!    (`Ref<InputHandle>` with `.focus()`) and custom-component handles
//!    (`Ref<DatePickerHandle>` with `.jump_to_date(d)`).
//! 2. Custom-component handles can close over the component's own Signals
//!    (modelled here with a `Cell`) so methods mutate component-local
//!    state.
//! 3. Pre-mount calls are a clean no-op via `Option<H>` semantics — the
//!    parent's `.with(|h| h.method())` is skipped when the slot is empty.
//! 4. The arena slot is freed deterministically when its owning scope
//!    drops, matching `reactive-arena`'s lifetime model.
//!
//! ## Lifetime & aliasing
//!
//! Slots are addressed by generational `RefId`s (`{ index, generation }`)
//! and freed indices are returned to a free-list for reuse — so the arena
//! stays bounded by *peak concurrent* refs rather than total-ever-allocated.
//! Reuse is made safe by the generation guard: freeing a slot bumps its
//! generation, so a stale `Ref` copy that outlives its scope can never alias
//! the next occupant of the same index. A stale `Ref::with` reads as a clean
//! `None` (matching React's "ref.current may be null"), exactly as a
//! never-mounted ref does — it never reaches an unrelated handle.
//!
//! ## What's intentionally excluded
//!
//! - Macros. The `methods!` block expansion is left for runtime-macros.
//!   Custom handles in this prototype are hand-written to prove the
//!   underlying API supports them.
//! - Backend integration. Element handles here use a trivial dispatch
//!   trait; the real backend wiring lives in runtime-core.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;

// ----------------------------------------------------------------------------
// IDs and arena
// ----------------------------------------------------------------------------

/// Generational address of a ref slot. `generation` guards against a reused
/// index: a `Ref` is only valid while the slot's live generation matches.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefId {
    index: u32,
    generation: u32,
}

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
}

/// One ref slot. `alive` is the outer liveness (false once the owning scope
/// frees it); `handle` is the inner mount state (`None` until a component
/// fills it, `Some` once mounted). `generation` advances every time the slot
/// is freed so reused indices can't be aliased by stale handles.
struct RefSlot {
    generation: u32,
    alive: bool,
    handle: Option<Box<dyn Any>>,
}

struct Arena {
    refs: Vec<RefSlot>,
    free: Vec<u32>,
}

impl Arena {
    fn new() -> Self {
        Self { refs: Vec::new(), free: Vec::new() }
    }

    fn insert(&mut self) -> RefId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.refs[index as usize];
            debug_assert!(!slot.alive, "free-listed ref slot was still live");
            slot.alive = true;
            slot.handle = None;
            RefId { index, generation: slot.generation }
        } else {
            let index = self.refs.len() as u32;
            self.refs.push(RefSlot { generation: 0, alive: true, handle: None });
            RefId { index, generation: 0 }
        }
    }

    /// Returns the slot for `id` only if it is the same live generation.
    fn live(&self, id: RefId) -> Option<&RefSlot> {
        self.refs
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation && s.alive)
    }

    fn fill<H: 'static>(&mut self, id: RefId, handle: H) {
        match self.refs.get_mut(id.index as usize) {
            Some(slot) if slot.generation == id.generation && slot.alive => {
                // Overwrite is legal — happens on remount.
                slot.handle = Some(Box::new(handle));
            }
            _ => {
                // Filling a freed slot means a component mounted after its
                // owning scope was torn down — a mount/unmount ordering bug.
                // Surface it loudly in dev rather than silently dropping the
                // handle (which would leave the ref permanently dead).
                debug_assert!(
                    false,
                    "Ref::fill on a freed slot (id {:?}) — a component filled its ref \
                     after the owning scope was dropped; check mount/unmount ordering",
                    id
                );
            }
        }
    }

    fn clear(&mut self, id: RefId) {
        if let Some(slot) = self.refs.get_mut(id.index as usize) {
            if slot.generation == id.generation && slot.alive {
                slot.handle = None;
            }
        }
    }

    fn free(&mut self, id: RefId) {
        if let Some(slot) = self.refs.get_mut(id.index as usize) {
            if slot.generation == id.generation && slot.alive {
                slot.alive = false;
                slot.handle = None;
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(id.index);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Ref<H>
// ----------------------------------------------------------------------------

/// A copy-handle pointing at an arena slot that holds an `H` once a
/// component has mounted. Multiple copies of the same `Ref<H>` all
/// address the same slot.
///
/// The parent of a component owns the `Ref<H>` (typically inside its own
/// reactive scope). The child component mounts and calls
/// [`Ref::fill`] to populate the slot; unmount calls [`Ref::clear`].
///
/// Reading via [`Ref::with`] is a no-op if the slot has not been filled
/// yet *or* the owning scope has been dropped — pre-mount and post-teardown
/// calls are silently skipped, matching React's "ref.current may be null"
/// semantics but without the boilerplate. The generation guard ensures a
/// post-teardown `Ref` never observes an unrelated handle in a reused slot.
pub struct Ref<H> {
    id: RefId,
    _phantom: PhantomData<H>,
}

impl<H> Copy for Ref<H> {}
impl<H> Clone for Ref<H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H: 'static> Ref<H> {
    /// Allocates a fresh ref slot in the arena. Use [`Scope::ref_`] to
    /// bind the slot's lifetime to a scope.
    pub fn new() -> Self {
        let id = ARENA.with(|a| a.borrow_mut().insert());
        Self { id, _phantom: PhantomData }
    }

    /// Populates the slot. Called by the framework at component mount.
    /// In production code this is *not* user-callable; it lives behind
    /// the framework's mount path. We expose it here so the prototype
    /// can stand in for the framework.
    pub fn fill(&self, handle: H) {
        ARENA.with(|a| a.borrow_mut().fill(self.id, handle));
    }

    /// Clears the slot. Called by the framework at component unmount.
    pub fn clear(&self) {
        ARENA.with(|a| a.borrow_mut().clear(self.id));
    }

    /// Runs `f` against the filled handle, if any. Returns `None` if
    /// the component hasn't mounted yet, has been torn down, or the owning
    /// scope has been freed.
    ///
    /// The handle is held by `&` reference inside `f`, so method calls
    /// on it must take `&self`. Since handles only mutate via Signals
    /// (which use interior mutability), this is sufficient — and avoids
    /// any borrow-checker friction with multiple refs.
    pub fn with<R>(&self, f: impl FnOnce(&H) -> R) -> Option<R> {
        ARENA.with(|arena| {
            let arena = arena.borrow();
            let slot = arena.live(self.id)?;
            let handle = slot.handle.as_ref()?;
            let handle = handle
                .downcast_ref::<H>()
                .expect("internal: ref handle type mismatch");
            Some(f(handle))
        })
    }

    /// True if the slot has been filled and not subsequently cleared (and
    /// its owning scope is still alive).
    pub fn is_mounted(&self) -> bool {
        ARENA.with(|arena| {
            arena
                .borrow()
                .live(self.id)
                .map(|s| s.handle.is_some())
                .unwrap_or(false)
        })
    }
}

// ----------------------------------------------------------------------------
// Scope
// ----------------------------------------------------------------------------

/// Owns ref slots. Drop the scope to free every slot it created.
pub struct Scope {
    refs: Vec<RefId>,
}

impl Scope {
    pub fn new() -> Self {
        Self { refs: Vec::new() }
    }

    pub fn ref_<H: 'static>(&mut self) -> Ref<H> {
        let r = Ref::<H>::new();
        self.refs.push(r.id);
        r
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
            for id in self.refs.drain(..) {
                a.free(id);
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

    // --- Test 1: primitive-style handle -------------------------------------

    /// Stand-in for what a real primitive handle would look like. In the
    /// framework, this struct would carry a `BackendNode` plus a
    /// `&'static dyn InputOps` trait object so `.focus()` reaches the
    /// backend. Here the "backend" is just a counter for assertions.
    #[derive(Clone)]
    struct InputHandle {
        focus_count: Rc<Cell<u32>>,
    }

    impl InputHandle {
        fn focus(&self) {
            self.focus_count.set(self.focus_count.get() + 1);
        }
    }

    #[test]
    fn primitive_handle_focus_after_mount() {
        let focus_count = Rc::new(Cell::new(0));
        let mut scope = Scope::new();
        let r: Ref<InputHandle> = scope.ref_();

        // Pre-mount: with() returns None.
        assert!(!r.is_mounted());
        let pre = r.with(|h| h.focus());
        assert!(pre.is_none(), "with() must be a no-op before mount");
        assert_eq!(focus_count.get(), 0, "no focus call before mount");

        // Framework mounts the component; ref gets filled.
        r.fill(InputHandle { focus_count: focus_count.clone() });
        assert!(r.is_mounted());

        // Parent calls .focus() — the dispatch reaches the handle's
        // method, which mutates the shared counter.
        r.with(|h| h.focus());
        assert_eq!(focus_count.get(), 1);
        r.with(|h| h.focus());
        assert_eq!(focus_count.get(), 2);

        // Unmount: subsequent calls are no-ops again.
        r.clear();
        assert!(!r.is_mounted());
        let post = r.with(|h| h.focus());
        assert!(post.is_none());
        assert_eq!(focus_count.get(), 2, "no focus call after unmount");
    }

    // --- Test 2: custom-component-style handle ------------------------------

    /// Models what a `#[component] fn date_picker(...)` would produce.
    /// The handle closes over the component's internal state (here,
    /// `Cell<i32>` standing in for a `Signal<Date>`). A real expansion
    /// would close over Copy-handle Signals instead of Rc<Cell>, but the
    /// closure structure is the same.
    struct DatePickerHandle {
        value: Rc<Cell<i32>>,
        open: Rc<Cell<bool>>,
    }

    impl DatePickerHandle {
        fn jump_to_date(&self, d: i32) {
            self.value.set(d);
        }
        fn open_picker(&self) {
            self.open.set(true);
        }
    }

    /// Models a "component" — owns its state, exposes a handle. The
    /// real `#[component]` macro will generate this shape from a
    /// `methods!` block.
    fn mount_date_picker(r: Ref<DatePickerHandle>) -> (Rc<Cell<i32>>, Rc<Cell<bool>>) {
        // Component-local state. In a real component these are Signals.
        let value = Rc::new(Cell::new(0));
        let open = Rc::new(Cell::new(false));

        // Mount: fill the ref with a handle that closes over the state.
        r.fill(DatePickerHandle { value: value.clone(), open: open.clone() });
        (value, open)
    }

    #[test]
    fn custom_handle_methods_mutate_component_state() {
        let mut scope = Scope::new();
        let picker_ref: Ref<DatePickerHandle> = scope.ref_();

        let (value, open) = mount_date_picker(picker_ref);

        // Parent calls the custom method.
        picker_ref.with(|p| p.jump_to_date(20260514));
        assert_eq!(value.get(), 20260514, "method closure mutated component-local state");

        picker_ref.with(|p| p.open_picker());
        assert!(open.get());
    }

    // --- Test 3: scope drop frees slot --------------------------------------

    #[test]
    fn scope_drop_frees_ref_slot() {
        let id;
        {
            let mut scope = Scope::new();
            let r: Ref<InputHandle> = scope.ref_();
            r.fill(InputHandle { focus_count: Rc::new(Cell::new(0)) });
            id = r.id;
            assert!(r.is_mounted());
        }
        // Scope dropped: slot is no longer live for this generation.
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.live(id).is_none(), "scope drop must free the ref slot");
        });
    }

    // --- Test 4: Copy semantics — passing into closures without clone -------

    #[test]
    fn ref_is_copy_into_closures() {
        let mut scope = Scope::new();
        let r: Ref<InputHandle> = scope.ref_();
        let focus_count = Rc::new(Cell::new(0));
        r.fill(InputHandle { focus_count: focus_count.clone() });

        // Two independent closures, both capturing `r` by Copy — no
        // .clone() ceremony.
        let call_focus = move || {
            r.with(|h| h.focus());
        };
        let check_mounted = move || r.is_mounted();

        assert!(check_mounted());
        call_focus();
        assert_eq!(focus_count.get(), 1);
    }

    // --- Test 5: remount overwrites the slot --------------------------------

    /// When a `when()` branch flips and the same ref-bearing component
    /// is remounted in the new subtree, the second `fill` must replace
    /// the first cleanly — no leaks, no stale handle.
    #[test]
    fn remount_overwrites_slot() {
        let mut scope = Scope::new();
        let r: Ref<InputHandle> = scope.ref_();

        let first = Rc::new(Cell::new(0));
        r.fill(InputHandle { focus_count: first.clone() });
        r.with(|h| h.focus());
        assert_eq!(first.get(), 1);

        // Remount with a fresh handle.
        let second = Rc::new(Cell::new(0));
        r.fill(InputHandle { focus_count: second.clone() });
        r.with(|h| h.focus());
        assert_eq!(second.get(), 1);
        assert_eq!(first.get(), 1, "old handle no longer reachable");
    }

    // --- Test 6: type mismatch panics with a clear message ------------------

    /// `Ref<T>` carries `T` at compile time so type errors are usually
    /// caught at the type checker. The only way to get a mismatch is to
    /// `fill` a slot via one type and reinterpret the id as another —
    /// which is impossible from safe user code. The downcast inside
    /// `with` still asserts the invariant, panicking if it ever fails.
    #[test]
    fn handle_type_is_compile_time_safe() {
        let mut scope = Scope::new();
        let input_ref: Ref<InputHandle> = scope.ref_();
        let picker_ref: Ref<DatePickerHandle> = scope.ref_();
        let _ = input_ref;
        let _ = picker_ref;
    }

    // --- Test 7: generational reuse never aliases ---------------------------

    /// Regression: a freed slot's index is reused, but a stale `Ref` copy
    /// that outlived its scope must NOT alias the reused slot's handle. It
    /// must read as a clean `None`, never reach the new occupant's handle.
    #[test]
    fn freed_ref_index_reused_without_aliasing() {
        let stale: Ref<InputHandle>;
        {
            let mut scope = Scope::new();
            stale = scope.ref_();
            stale.fill(InputHandle { focus_count: Rc::new(Cell::new(0)) });
            assert!(stale.is_mounted());
        }
        // The freed index is reclaimed by the next allocation.
        let mut scope2 = Scope::new();
        let fresh: Ref<InputHandle> = scope2.ref_();
        assert_eq!(stale.id.index, fresh.id.index, "index should be reused from the free-list");
        assert_ne!(stale.id.generation, fresh.id.generation, "generation must advance on reuse");

        assert!(!stale.is_mounted(), "stale ref must not see the reused slot");
        let count = Rc::new(Cell::new(0));
        fresh.fill(InputHandle { focus_count: count.clone() });
        let reached = stale.with(|h| h.focus());
        assert!(reached.is_none(), "stale ref must not reach the reused slot's handle");
        assert_eq!(count.get(), 0, "stale ref must not invoke the new handle's method");
    }

    // --- Test 8: filling a freed slot is caught in debug --------------------

    /// Regression: a component that fills its ref after the owning scope was
    /// dropped is a mount/unmount ordering bug. In debug builds it must be
    /// surfaced loudly rather than silently leaving the ref permanently dead.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "freed slot")]
    fn fill_on_freed_slot_panics_in_debug() {
        let r: Ref<InputHandle>;
        {
            let mut scope = Scope::new();
            r = scope.ref_();
        }
        // Scope dropped — the slot is freed. Filling now is the bug.
        r.fill(InputHandle { focus_count: Rc::new(Cell::new(0)) });
    }
}
