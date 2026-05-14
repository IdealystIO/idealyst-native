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
//! API is stable we want to iterate without breaking framework-core.
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
//! ## What's intentionally excluded
//!
//! - Macros. The `methods!` block expansion is left for framework-macros.
//!   Custom handles in this prototype are hand-written to prove the
//!   underlying API supports them.
//! - Backend integration. Primitive handles here use a trivial dispatch
//!   trait; the real backend wiring lives in framework-core.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;

// ----------------------------------------------------------------------------
// IDs and arena
// ----------------------------------------------------------------------------

/// Index into the arena's ref slot table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefId(u32);

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
}

struct Arena {
    /// Each slot is `Option<Option<Box<dyn Any>>>`:
    /// - outer `Option`: `None` once the slot is freed by its scope.
    /// - inner `Option`: `None` while the ref exists but hasn't been
    ///   filled by a mount yet; `Some` once mounted.
    refs: Vec<Option<Option<Box<dyn Any>>>>,
}

impl Arena {
    fn new() -> Self { Self { refs: Vec::new() } }

    fn insert(&mut self) -> RefId {
        let id = RefId(self.refs.len() as u32);
        self.refs.push(Some(None));
        id
    }

    fn fill<H: 'static>(&mut self, id: RefId, handle: H) {
        if let Some(slot) = self.refs.get_mut(id.0 as usize) {
            // Slot must exist (i.e. scope not dropped). Overwrite is
            // legal — happens on remount.
            if let Some(inner) = slot.as_mut() {
                *inner = Some(Box::new(handle));
            }
        }
    }

    fn clear(&mut self, id: RefId) {
        if let Some(Some(inner)) = self.refs.get_mut(id.0 as usize) {
            *inner = None;
        }
    }

    fn free(&mut self, id: RefId) {
        if let Some(slot) = self.refs.get_mut(id.0 as usize) {
            *slot = None;
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
/// yet — pre-mount calls are silently skipped, matching React's
/// "ref.current may be null" semantics but without the boilerplate.
pub struct Ref<H> {
    id: RefId,
    _phantom: PhantomData<H>,
}

impl<H> Copy for Ref<H> {}
impl<H> Clone for Ref<H> {
    fn clone(&self) -> Self { *self }
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
    /// the component hasn't mounted yet (or has been torn down).
    ///
    /// The handle is held by `&` reference inside `f`, so method calls
    /// on it must take `&self`. Since handles only mutate via Signals
    /// (which use interior mutability), this is sufficient — and avoids
    /// any borrow-checker friction with multiple refs.
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
    /// Useful for parent components that want to render differently
    /// depending on mount state, though in practice `with(...).is_some()`
    /// reads as well.
    pub fn is_mounted(&self) -> bool {
        ARENA.with(|arena| {
            let arena = arena.borrow();
            arena.refs
                .get(self.id.0 as usize)
                .and_then(|s| s.as_ref())
                .map(|inner| inner.is_some())
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
    pub fn new() -> Self { Self { refs: Vec::new() } }

    pub fn ref_<H: 'static>(&mut self) -> Ref<H> {
        let r = Ref::<H>::new();
        self.refs.push(r.id);
        r
    }
}

impl Default for Scope {
    fn default() -> Self { Self::new() }
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
    fn mount_date_picker(r: Ref<DatePickerHandle>)
        -> (Rc<Cell<i32>>, Rc<Cell<bool>>)
    {
        // Component-local state. In a real component these are Signals.
        let value = Rc::new(Cell::new(0));
        let open = Rc::new(Cell::new(false));

        // Mount: fill the ref with a handle that closes over the state.
        r.fill(DatePickerHandle {
            value: value.clone(),
            open: open.clone(),
        });
        (value, open)
    }

    #[test]
    fn custom_handle_methods_mutate_component_state() {
        let mut scope = Scope::new();
        let picker_ref: Ref<DatePickerHandle> = scope.ref_();

        let (value, open) = mount_date_picker(picker_ref);

        // Parent calls the custom method.
        picker_ref.with(|p| p.jump_to_date(20260514));
        assert_eq!(value.get(), 20260514,
            "method closure mutated component-local state");

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
        // Scope dropped: slot is gone.
        ARENA.with(|a| {
            let a = a.borrow();
            assert!(a.refs[id.0 as usize].is_none(),
                "scope drop must free the ref slot");
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
        let call_focus = move || { r.with(|h| h.focus()); };
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
        // This test exists mostly as a compile-only check. The fact
        // that the following code compiles without complaint is the
        // assertion.
        let mut scope = Scope::new();
        let input_ref: Ref<InputHandle> = scope.ref_();
        let picker_ref: Ref<DatePickerHandle> = scope.ref_();

        // A user cannot accidentally `picker_ref.with(|i: &InputHandle| ...)`
        // — the closure parameter type drives inference back to
        // DatePickerHandle, so the wrong type would be a compile error
        // at the call site, not a runtime panic. The downcast in `with`
        // is purely a belt-and-braces check.
        let _ = input_ref;
        let _ = picker_ref;
    }
}
