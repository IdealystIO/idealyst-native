//! [`Handle`] — a typed reference to an `#[offload::job]` function, and the
//! [`handle!`](crate::handle) macro that builds one. Shared by both
//! backends: native calls the function pointer on a thread; web sends its
//! function-table index to a Worker running the same module (see `web.rs`).

/// A typed handle to an offload job — its name (for diagnostics) and the
/// function pointer to invoke. Built by the [`handle!`](crate::handle)
/// macro, so the call site is identical across platforms.
pub struct Handle<T, R> {
    pub(crate) name: &'static str,
    pub(crate) f: fn(T) -> R,
}

impl<T, R> Handle<T, R> {
    /// Construct a handle from a job's name + function pointer. Prefer the
    /// [`handle!`](crate::handle) macro, which fills in the name via `stringify!`.
    #[doc(hidden)]
    pub fn new_unchecked(name: &'static str, f: fn(T) -> R) -> Self {
        Self { name, f }
    }

    /// The job's source name (the identifier passed to `handle!`).
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl<T, R> Clone for Handle<T, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, R> Copy for Handle<T, R> {}

/// **Value** equality, not pointer identity — the deliberate exception
/// among the framework's handle types.
///
/// A `Handle` is not a live resource; it is a pair of already-comparable
/// plain values (`&'static str` + a `fn` pointer) naming a job. Two
/// handles built from the same `#[offload::job]` fn ARE the same job, and
/// a guarded `set` re-storing `handle!(rasterize)` should correctly be a
/// no-op. Reaching for `Rc::ptr_eq` here would be wrong twice over: there
/// is no `Rc`, and `Handle` is `Copy`, so "instance" is not even a
/// meaningful notion.
///
/// Written by hand rather than `#[derive(PartialEq)]` because the derive
/// would bound the impl on `T: PartialEq, R: PartialEq`. Neither type
/// parameter is stored — they appear only inside the `fn(T) -> R` pointer,
/// which is comparable regardless — so the derive's bounds would be pure
/// false coupling, and would break exactly the jobs whose payloads (the
/// motivating case for offloading) have no equality of their own. Mirrors
/// the manual `Clone`/`Copy` right above, which skip `T: Clone` for the
/// same reason.
///
/// `fn_addr_eq` is documented as unreliable in both directions (codegen
/// may merge identical bodies, or emit one fn at two addresses across
/// codegen units), so `name` carries the real discriminator and the
/// address only tightens it. Every way the pair can be wrong errs toward
/// reporting UNEQUAL, which in a guarded `set` means a redundant notify —
/// never a swallowed one, so no update can be lost.
impl<T, R> PartialEq for Handle<T, R> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && std::ptr::fn_addr_eq(self.f, other.f)
    }
}

impl<T, R> Eq for Handle<T, R> {}

impl<T, R> std::fmt::Debug for Handle<T, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "offload::Handle({})", self.name)
    }
}

/// Build a typed [`Handle`] from a `#[offload::job]` function:
/// `offload::handle!(rasterize)` (a path works too: `handle!(jobs::rasterize)`).
#[macro_export]
macro_rules! handle {
    ($f:path) => {
        $crate::Handle::new_unchecked(::core::stringify!($f), $f)
    };
}

#[cfg(test)]
mod tests {
    fn add_one(x: u64) -> u64 {
        x + 1
    }

    /// `Handle: PartialEq` is VALUE equality — two handles naming the
    /// same job are the same job, so a guarded `Signal::set` of
    /// `handle!(add_one)` over itself is correctly a no-op. (`Signal<T>`
    /// is bounded on `T: PartialEq` at creation and `get`; without this
    /// impl a job handle could not be held in app state at all.)
    #[test]
    fn handles_for_the_same_job_compare_equal() {
        let a = crate::handle!(add_one);
        let b = crate::handle!(add_one);
        assert!(a == b, "the same job named twice is one job");
        assert!(a == a.clone());
        assert_eq!(a.name(), "add_one");
    }

    /// Distinct jobs must compare unequal — otherwise swapping which job
    /// a signal holds would silently fail to notify.
    #[test]
    fn handles_for_different_jobs_compare_unequal() {
        fn add_two(x: u64) -> u64 {
            x + 2
        }
        let a = crate::handle!(add_one);
        let b = crate::handle!(add_two);
        assert!(a != b, "different jobs are different handles");
    }

    /// The impl must not require `T: PartialEq` / `R: PartialEq`. This
    /// job's argument type deliberately has neither — a derive would have
    /// bounded the impl on them and refused to compile this line, which
    /// is precisely the false coupling the manual impl avoids.
    #[test]
    fn handle_eq_does_not_bound_the_payload_types() {
        #[derive(Clone)]
        struct NoEq(u8);
        fn identity(x: NoEq) -> NoEq {
            x
        }
        let a = crate::handle!(identity);
        let b = crate::handle!(identity);
        assert!(a == b);
    }
}
