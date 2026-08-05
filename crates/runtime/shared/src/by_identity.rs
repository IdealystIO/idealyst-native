//! [`ByIdentity`] / [`ByIdentityArc`] — put a value with no meaningful
//! value equality into a `Signal`.
//!
//! # Why this exists
//!
//! `Signal<T>` / `ReadSignal<T>` / `WriteSignal<T>` are bounded on
//! `T: PartialEq` — at *creation* and at `get`, not merely on the
//! equality-guarded `set`. The guard is what makes `set` cheap: committing
//! a value equal to the current one notifies nobody. So the reactive
//! kernel always needs an answer to "is the new value the same as the old
//! one?", and a type with no `PartialEq` cannot be held in app state at
//! all.
//!
//! For most payloads the answer is `#[derive(PartialEq)]`. For a *handle*
//! — a live capture, an open connection, a cancel button, a mounted node
//! — there is no value to compare: the payload is a channel plus some
//! native resource, and comparing its current contents would be both wrong
//! (a stream is not its current frame) and unbounded. The right question
//! for those is **identity**: "is this the same instance?" That is exactly
//! what a guarded `set` wants to know, and `Rc::ptr_eq` answers it.
//!
//! Framework types answer it themselves (see `MediaStream`,
//! `net::CancelHandle`, `NavHandle`, …). `ByIdentity` is the escape hatch
//! for the values the framework does *not* own: a third-party type, or one
//! of your own in another crate, where the orphan rule blocks you from
//! writing `impl PartialEq for … ` just as it blocks us.
//!
//! ```ignore
//! use runtime_core::{signal, ByIdentity};
//!
//! // `third_party::Session` has no PartialEq and you cannot add one.
//! let session = signal(ByIdentity::new(third_party::Session::open()?));
//!
//! // Deref gives you the payload back, no unwrapping ceremony:
//! session.with(|s| s.ping());
//!
//! // Replacing with a genuinely different session notifies; storing the
//! // same one again is a no-op, same as any other guarded `set`.
//! session.set(ByIdentity::new(third_party::Session::open()?));
//! ```
//!
//! # Which one
//!
//! [`ByIdentity`] wraps `Rc<T>` and is the default: the reactive arena is
//! thread-local, so signal payloads never cross threads and `Rc` is the
//! right sharing primitive. [`ByIdentityArc`] wraps `Arc<T>` and exists
//! for the case where you are handed an `Arc` you did not create —
//! `storage::platform_storage() -> Arc<dyn Storage>` is the shipped
//! example. Wrapping such a value in `ByIdentity` would allocate a *new*
//! `Rc` around it, so two wrappers holding clones of the same `Arc` would
//! compare unequal — the opposite of what identity means. Match the
//! pointer you already have.
//!
//! Both are `?Sized`-tolerant, so `ByIdentity<dyn Any>` /
//! `ByIdentityArc<dyn Storage>` work.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

/// Generate one identity wrapper over a reference-counted pointer.
///
/// `Rc` and `Arc` have identical surface here but no shared trait that
/// exposes `ptr_eq` + `as_ptr`, so the two impls are macro-generated
/// rather than written against a bound.
macro_rules! by_identity {
    (
        $(#[$meta:meta])*
        $name:ident, $ptr:ident, $ptr_path:literal, $ctor_doc:literal
    ) => {
        $(#[$meta])*
        pub struct $name<T: ?Sized>($ptr<T>);

        impl<T> $name<T> {
            #[doc = $ctor_doc]
            pub fn new(value: T) -> Self {
                Self($ptr::new(value))
            }
        }

        impl<T: ?Sized> $name<T> {
            /// Wrap a pointer you already hold. Clones of that pointer
            /// wrapped separately still compare equal — the wrapper adds
            /// no allocation and no indirection of its own.
            #[doc = concat!("Takes a `", $ptr_path, "<T>`.")]
            pub fn from_ptr(ptr: $ptr<T>) -> Self {
                Self(ptr)
            }

            /// Borrow the underlying pointer (to clone it, or to hand it
            /// to an API that wants the bare pointer type).
            pub fn as_ptr(&self) -> &$ptr<T> {
                &self.0
            }

            /// Unwrap back to the bare pointer.
            pub fn into_ptr(self) -> $ptr<T> {
                self.0
            }
        }

        /// Pointer identity — the whole point of the type. This is the
        /// question a guarded `set` asks ("is this the same instance?"),
        /// and the one answer available for a payload with no value
        /// equality.
        impl<T: ?Sized> PartialEq for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                $ptr::ptr_eq(&self.0, &other.0)
            }
        }

        /// Pointer identity is reflexive, symmetric and transitive for
        /// any `T`, including one whose own `PartialEq` would not be
        /// (`f64::NAN`), so `Eq` holds unconditionally.
        impl<T: ?Sized> Eq for $name<T> {}

        /// Hashes the ADDRESS, so `Hash` agrees with the `Eq` above.
        /// Hashing the pointee would break the `a == b ⇒ hash(a) ==
        /// hash(b)` contract in the other direction the moment `T` is
        /// interior-mutable.
        impl<T: ?Sized> Hash for $name<T> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                ($ptr::as_ptr(&self.0) as *const ()).hash(state);
            }
        }

        /// Cloning shares the pointee, so a clone compares equal to its
        /// source — the property every "two clones of one handle" call
        /// site depends on. Hand-written (not derived) because a derive
        /// would demand `T: Clone`, which defeats wrapping an unclonable
        /// payload.
        impl<T: ?Sized> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<T: ?Sized> Deref for $name<T> {
            type Target = T;
            fn deref(&self) -> &T {
                &self.0
            }
        }

        impl<T: ?Sized> AsRef<T> for $name<T> {
            fn as_ref(&self) -> &T {
                &self.0
            }
        }

        impl<T: ?Sized> From<$ptr<T>> for $name<T> {
            fn from(ptr: $ptr<T>) -> Self {
                Self(ptr)
            }
        }

        impl<T: Default> Default for $name<T> {
            fn default() -> Self {
                Self::new(T::default())
            }
        }

        impl<T: ?Sized + fmt::Debug> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&&self.0).finish()
            }
        }
    };
}

by_identity!(
    /// A payload compared by **pointer identity** instead of value, so it
    /// satisfies the `T: PartialEq` bound every signal handle carries.
    ///
    /// Wraps `Rc<T>`; `Deref`s to `T`, so the wrapper is invisible at use
    /// sites. See the [module docs](self) for when to reach for it and for
    /// the `Arc` sibling, [`ByIdentityArc`].
    ByIdentity,
    Rc,
    "Rc",
    "Allocate a fresh `Rc` around `value`. The result is equal only to \
     its own clones — a second `ByIdentity::new` of an equal value is a \
     DIFFERENT instance and compares unequal, which is what makes a \
     guarded `set` notify."
);

by_identity!(
    /// The `Arc` sibling of [`ByIdentity`], for wrapping a thread-safe
    /// pointer you were handed rather than one you allocated (e.g.
    /// `storage::platform_storage() -> Arc<dyn Storage>`).
    ///
    /// Reach for this only when the `Arc` already exists; otherwise prefer
    /// [`ByIdentity`], since signal payloads never leave the thread that
    /// owns the reactive arena.
    ByIdentityArc,
    Arc,
    "Arc",
    "Allocate a fresh `Arc` around `value`. Prefer \
     [`ByIdentityArc::from_ptr`] when you already hold the `Arc` — \
     re-wrapping a clone in a new `Arc` would lose the identity you \
     wanted to preserve."
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Deliberately has NO `PartialEq` — that is the situation the type
    /// exists for. (`Debug` is derived only so `assert_eq!` can print;
    /// it plays no part in the equality being tested.)
    #[derive(Debug)]
    struct NoEq {
        hits: Cell<u32>,
    }

    impl NoEq {
        fn new() -> Self {
            NoEq { hits: Cell::new(0) }
        }
        fn bump(&self) {
            self.hits.set(self.hits.get() + 1);
        }
    }

    #[test]
    fn by_identity_clones_are_equal_distinct_instances_are_not() {
        let a = ByIdentity::new(NoEq::new());
        let a2 = a.clone();
        let b = ByIdentity::new(NoEq::new());

        assert_eq!(a, a2, "clones of one wrapper address the same instance");
        assert_ne!(a, b, "independently allocated payloads are different instances");
    }

    #[test]
    fn by_identity_derefs_to_the_payload() {
        let a = ByIdentity::new(NoEq::new());
        a.bump();
        a.bump();
        assert_eq!(a.hits.get(), 2, "Deref reaches the payload's own methods");
        // …and the clone observes the same interior state.
        assert_eq!(a.clone().hits.get(), 2);
    }

    #[test]
    fn by_identity_from_ptr_preserves_the_callers_pointer() {
        let rc = Rc::new(NoEq::new());
        let a = ByIdentity::from_ptr(rc.clone());
        let b = ByIdentity::from_ptr(rc);
        assert_eq!(a, b, "two wrappers over clones of ONE Rc are the same instance");
    }

    #[test]
    fn by_identity_hash_agrees_with_eq() {
        use std::collections::HashSet;
        let a = ByIdentity::new(NoEq::new());
        let mut set = HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&a), "a clone hashes into the same bucket");
        assert!(!set.contains(&ByIdentity::new(NoEq::new())));
    }

    #[test]
    fn by_identity_wraps_unsized_payloads() {
        let rc: Rc<dyn std::fmt::Debug> = Rc::new(7u8);
        let a = ByIdentity::from_ptr(rc);
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn by_identity_arc_clones_are_equal_distinct_instances_are_not() {
        // `Arc` so the payload must be Send+Sync-able in principle; the
        // test payload is a plain value.
        let a = ByIdentityArc::new(String::from("x"));
        let a2 = a.clone();
        let b = ByIdentityArc::new(String::from("x"));

        assert_eq!(a, a2);
        assert_ne!(
            a, b,
            "EQUAL VALUES, different instances — identity, not value, is the question"
        );
        assert_eq!(&*a, "x", "Deref still reaches the payload");
    }

    #[test]
    fn by_identity_arc_from_ptr_preserves_the_callers_pointer() {
        let arc: Arc<dyn std::fmt::Debug + Send + Sync> = Arc::new(1u32);
        let a = ByIdentityArc::from_ptr(arc.clone());
        let b = ByIdentityArc::from_ptr(arc);
        assert_eq!(a, b);
    }
}
