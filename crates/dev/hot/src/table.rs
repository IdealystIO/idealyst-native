//! The jump table and the dispatch through it, for wasm — owned here
//! rather than by subsecond.
//!
//! # Why not subsecond's
//!
//! subsecond's wasm `apply_patch` does three things in one call: it
//! fetches and instantiates the patch, supplying it ONE import namespace
//! (`env`, the base's exports plus `__memory_base`/`__table_base`), and
//! then commits the jump table through a private `commit_patch`. A patch
//! whose imports are anything but base exports therefore had to be
//! rewritten until it needed nothing else, which is the walrus pass that
//! cost 0.7–1.0 s of every save on CrewForge (parse and re-emit 27 MB to
//! turn ~3,600 imports into local stubs).
//!
//! The page now loads the patch itself (`backend_web::hot_patch`) and
//! supplies every import directly — a base function as the function, a
//! `GOT` entry as a global. That leaves the commit, which subsecond does
//! not expose (0.7.9 or 0.8.0-alpha.1), and the lookup a hot call makes,
//! which reads the table the commit wrote. Both are small, so they live
//! here. Native keeps subsecond: its patches are dylibs that link against
//! subsecond's own table.
//!
//! # What a hot call does
//!
//! `#[component]` hands [`call`] a `fn(..)` POINTER to its
//! `__<Name>_hot_impl`. On wasm a `fn` pointer is an index into
//! `__indirect_function_table`; the table maps the base's index to the
//! patch's. A hit calls through the patch's index; a miss calls the
//! pointer as given. That is subsecond's fn-pointer path
//! (`HotFunction::call_as_ptr`) — the only one our jump tables have
//! entries for, since they are built by pairing `__*_hot_impl` symbols.
//!
//! Compiled on the host under `cfg(test)` too: a host `fn` pointer is an
//! address rather than a table index, but the dispatch treats both as an
//! opaque `usize`, so the same code is tested natively.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

thread_local! {
    /// Base `fn` pointer → patched `fn` pointer. Replaced wholesale by
    /// each [`commit`]: a patch carries every function patched since the
    /// base (see `build_web::hotpatch_build`), so the newest table is the
    /// whole truth.
    static TABLE: RefCell<Rc<HashMap<u64, u64>>> = RefCell::new(Rc::new(HashMap::new()));
    /// Run after every commit, in registration order.
    static HANDLERS: RefCell<Vec<fn()>> = const { RefCell::new(Vec::new()) };
}

/// A callable [`call`] can dispatch: a function of one arity, taking its
/// arguments as a tuple.
///
/// Mirrors `subsecond::HotFunction` for the one path we use. `Marker`
/// keeps the per-arity impls from overlapping, as subsecond's does.
pub trait HotFunction<Args, Marker> {
    type Return;
    /// Call `self` directly.
    fn call_it(self, args: Args) -> Self::Return;
    /// Call the function at `target` — a `fn` pointer value of the same
    /// signature as `Self`.
    ///
    /// # Safety
    ///
    /// `target` must be a valid `fn` pointer whose signature is `Self`'s.
    /// The jump table pairs functions by identical mangled name, and a
    /// mangled name encodes the signature, so an entry satisfies this by
    /// construction.
    unsafe fn call_at(self, target: usize, args: Args) -> Self::Return;
}

macro_rules! impl_hot_function {
    ($( ($marker:ident $(, $arg:ident)*) ),* $(,)?) => {$(
        #[doc(hidden)]
        pub struct $marker;

        impl<T, $($arg,)* R> HotFunction<($($arg,)*), $marker> for T
        where
            T: FnOnce($($arg),*) -> R,
        {
            type Return = R;

            #[allow(non_snake_case)]
            fn call_it(self, args: ($($arg,)*)) -> R {
                let ($($arg,)*) = args;
                self($($arg),*)
            }

            #[allow(non_snake_case)]
            unsafe fn call_at(self, target: usize, args: ($($arg,)*)) -> R {
                let ($($arg,)*) = args;
                // SAFETY: the caller's contract — `target` is a `fn`
                // pointer of exactly this signature. A `fn` pointer and a
                // `usize` have the same size on every target we build.
                let f = unsafe { std::mem::transmute::<usize, fn($($arg),*) -> R>(target) };
                f($($arg),*)
            }
        }
    )*};
}

impl_hot_function!(
    (Fn0Marker),
    (Fn1Marker, A),
    (Fn2Marker, A, B),
    (Fn3Marker, A, B, C),
    (Fn4Marker, A, B, C, D),
    (Fn5Marker, A, B, C, D, E),
    (Fn6Marker, A, B, C, D, E, F),
    (Fn7Marker, A, B, C, D, E, F, G),
    (Fn8Marker, A, B, C, D, E, F, G, H),
    (Fn9Marker, A, B, C, D, E, F, G, H, I),
);

/// Dispatch `f(args)` through the jump table.
///
/// Only a `fn` POINTER can be looked up: it is the one `F` whose value
/// is the address the table is keyed by. `#[component]` always passes
/// one (see `runtime-macros`' `hot_split`); anything else — a ZST fn
/// item, a closure — is called directly, as subsecond's fn-pointer path
/// would have missed it too.
pub fn call<Args, F, M>(f: F, args: Args) -> F::Return
where
    F: HotFunction<Args, M>,
{
    if std::mem::size_of::<F>() == std::mem::size_of::<fn()>() {
        // SAFETY: `F` is pointer-sized; for the `fn` pointer #[component]
        // passes, this reads its value. For any other pointer-sized `F`
        // the value is used only as a lookup key that a jump table built
        // from `fn` pointers will not contain.
        let key = unsafe { std::mem::transmute_copy::<F, usize>(&f) } as u64;
        if let Some(target) = TABLE.with(|t| t.borrow().get(&key).copied()) {
            // SAFETY: a table entry pairs two functions of one mangled
            // name — see `HotFunction::call_at`.
            return unsafe { f.call_at(target as usize, args) };
        }
    }
    f.call_it(args)
}

/// Install `map` as the jump table — base `fn` pointer value → patched
/// `fn` pointer value, both ABSOLUTE — and run the registered handlers.
///
/// # Safety
///
/// Every value must be a valid `fn` pointer with the same signature as
/// its key's. A table built by pairing identical mangled names, and
/// rebased onto the table slots the patch was instantiated into, is.
pub unsafe fn commit(map: HashMap<u64, u64>) {
    TABLE.with(|t| *t.borrow_mut() = Rc::new(map));
    // Cloned out first: a handler that registers another must not find
    // the list borrowed.
    let handlers = HANDLERS.with(|h| h.borrow().clone());
    for handler in handlers {
        handler();
    }
}

/// Run `f` after every [`commit`].
pub fn register_handler(f: fn()) {
    HANDLERS.with(|h| h.borrow_mut().push(f));
}

/// Whether the committed table redirects `ptr`, and where to.
pub fn redirect_of(ptr: u64) -> Option<u64> {
    TABLE.with(|t| t.borrow().get(&ptr).copied())
}

/// How many entries the committed table has.
pub fn len() -> usize {
    TABLE.with(|t| t.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn old(n: u32) -> u32 {
        n + 1
    }
    fn new(n: u32) -> u32 {
        n + 100
    }
    fn two(a: u32, b: &str) -> String {
        format!("{a}{b}")
    }
    fn two_new(a: u32, b: &str) -> String {
        format!("{b}{a}")
    }

    fn ptr1(f: fn(u32) -> u32) -> u64 {
        f as usize as u64
    }

    /// Before any commit a hot call is a direct call.
    #[test]
    fn an_empty_table_calls_the_pointer_given() {
        unsafe { commit(HashMap::new()) };
        let f: fn(u32) -> u32 = old;
        assert_eq!(call(f, (1,)), 2);
    }

    /// The whole point: a committed entry redirects the base's pointer to
    /// the patch's, whatever the arity.
    #[test]
    fn a_committed_entry_redirects_the_call() {
        let f: fn(u32) -> u32 = old;
        let g: fn(u32, &str) -> String = two;
        let mut map = HashMap::new();
        map.insert(ptr1(old), ptr1(new));
        map.insert(two as usize as u64, two_new as usize as u64);
        unsafe { commit(map) };
        assert_eq!(call(f, (1,)), 101);
        assert_eq!(call(g, (7, "x")), "x7");
        assert_eq!(redirect_of(ptr1(old)), Some(ptr1(new)));
        assert_eq!(len(), 2);
    }

    /// A later patch's table REPLACES the earlier one: an entry it does
    /// not carry goes back to the base.
    #[test]
    fn a_new_table_replaces_the_old_one() {
        let f: fn(u32) -> u32 = old;
        let mut map = HashMap::new();
        map.insert(ptr1(old), ptr1(new));
        unsafe { commit(map) };
        assert_eq!(call(f, (1,)), 101);
        unsafe { commit(HashMap::new()) };
        assert_eq!(call(f, (1,)), 2);
    }

    /// A closure is not pointer-sized in general and is never looked up.
    #[test]
    fn a_closure_is_called_directly() {
        let k = 5u32;
        let mut map = HashMap::new();
        map.insert(ptr1(old), ptr1(new));
        unsafe { commit(map) };
        assert_eq!(call(move |n: u32| n + k, (1,)), 6);
    }

    thread_local!(static FIRED: Cell<u32> = const { Cell::new(0) });
    fn bump() {
        FIRED.with(|f| f.set(f.get() + 1));
    }

    /// Handlers run once per commit — the page's rebuild rides this.
    #[test]
    fn handlers_run_after_each_commit() {
        register_handler(bump);
        let before = FIRED.with(|f| f.get());
        unsafe { commit(HashMap::new()) };
        unsafe { commit(HashMap::new()) };
        assert_eq!(FIRED.with(|f| f.get()) - before, 2);
    }
}
