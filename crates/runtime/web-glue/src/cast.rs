//! Typed handles: [`JsCast`] and the [`js_class!`](crate::js_class) macro.
//!
//! A typed handle (`dom::Element`, `dom::PointerEvent`, …) is a
//! `#[repr(transparent)]` wrapper over one [`JsValue`]. It owns the same
//! single slab slot; the type only says what the value is known to be.
//! This mirrors how the framework uses web-sys today (`Node` / `Element` /
//! `HtmlElement` / `HtmlInputElement`, each derefing to the one it
//! extends), so phase 3's SDK ports keep their shape.
//!
//! Casts:
//!
//! * [`JsCast::dyn_into`] / [`JsCast::dyn_ref`] — checked with the JS
//!   `instanceof` against the class's global constructor
//!   (`globalThis.HTMLElement`, …). A failed check hands the value back.
//! * [`JsCast::unchecked_into`] / [`JsCast::unchecked_ref`] — no check, for
//!   values whose type the JS snippet that produced them already
//!   guarantees (an `Event` handed to a `pointerdown` listener IS a
//!   `PointerEvent`). Unchecked is not unsafe: a wrongly typed handle can
//!   only make a later JS call throw or return a wrong value, never touch
//!   Rust memory — every access still goes through a snippet.
//!
//! The reference casts are sound because every class is
//! `#[repr(transparent)]` over `JsValue`, so `&JsValue`, `&Node` and
//! `&HtmlElement` share one layout.

use crate::{string, JsValue};

crate::import! {
    // The by-name check behind [`instance_of`]. NOT what a declared class
    // uses — see `js_class!` for why each class has its own import.
    fn js_instance_of(h: u32, p: usize, l: usize) -> u32 =
        "(() => { const cache = new Map(); return (h, p, l) => { \
           const name = G.str(p, l); let C = cache.get(name); \
           if (C === undefined) { C = globalThis[name]; cache.set(name, C); } \
           return typeof C === 'function' && G.get(h) instanceof C ? 1 : 0; }; })()";
}

/// A typed view of a JS value. Implemented by [`JsValue`] and every
/// [`js_class!`](crate::js_class).
pub trait JsCast: AsRef<JsValue> + Into<JsValue> + Sized + 'static {
    /// Whether `v` is an instance of this type (`instanceof` its JS
    /// class). `JsValue` accepts everything.
    fn is_type_of(v: &JsValue) -> bool;

    /// Wrap without checking. See the module docs for why this is safe.
    fn unchecked_from_js(v: JsValue) -> Self;

    /// Borrow as this type without checking.
    fn unchecked_from_js_ref(v: &JsValue) -> &Self;

    /// `self instanceof T`.
    fn is_instance_of<T: JsCast>(&self) -> bool {
        T::is_type_of(self.as_ref())
    }

    /// Checked downcast; on failure the value comes back unchanged.
    fn dyn_into<T: JsCast>(self) -> Result<T, Self> {
        if T::is_type_of(self.as_ref()) {
            Ok(T::unchecked_from_js(self.into()))
        } else {
            Err(self)
        }
    }

    /// Checked reference downcast.
    fn dyn_ref<T: JsCast>(&self) -> Option<&T> {
        T::is_type_of(self.as_ref()).then(|| T::unchecked_from_js_ref(self.as_ref()))
    }

    /// Unchecked conversion (see the module docs).
    fn unchecked_into<T: JsCast>(self) -> T {
        T::unchecked_from_js(self.into())
    }

    /// Unchecked reference conversion.
    fn unchecked_ref<T: JsCast>(&self) -> &T {
        T::unchecked_from_js_ref(self.as_ref())
    }
}

impl AsRef<JsValue> for JsValue {
    fn as_ref(&self) -> &JsValue {
        self
    }
}

impl JsCast for JsValue {
    fn is_type_of(_: &JsValue) -> bool {
        true
    }
    fn unchecked_from_js(v: JsValue) -> Self {
        v
    }
    fn unchecked_from_js_ref(v: &JsValue) -> &Self {
        v
    }
}

/// `v instanceof globalThis[class]`, for a class known only at run time
/// (or one not worth declaring, like a once-per-flow WebAuthn check). A
/// class the page's global scope lacks (e.g. `PointerEvent` in an old
/// engine) is never matched.
///
/// Every call ships `class` across the boundary and decodes it in JS, so a
/// [`js_class!`](crate::js_class) type's [`JsCast::is_type_of`] does NOT
/// come through here: each declared class has its own `instanceof` import.
pub fn instance_of(v: &JsValue, class: &str) -> bool {
    let (p, l) = string::abi(class);
    unsafe { js_instance_of(v.raw(), p, l) != 0 }
}

/// Declare typed handle classes.
///
/// ```ignore
/// web_glue::js_class! {
///     /// `HTMLElement`.
///     pub struct HtmlElement: Element, Node, EventTarget = "HTMLElement";
/// }
/// ```
///
/// After the colon come the class's ancestors, nearest first: the struct
/// derefs to the first, converts `Into` and borrows `AsRef` as every one
/// of them. The string is the global constructor `instanceof` checks
/// against. Each class is `#[repr(transparent)]` over [`JsValue`], which
/// is what makes the reference casts sound.
///
/// # One `instanceof` import per class
///
/// [`JsCast::is_type_of`] is generated as its own glue import whose JS
/// names the constructor (`globalThis.HTMLElement`), the way wasm-bindgen
/// emits `__wbg_instanceof_<Class>`. It used to call [`instance_of`] with
/// the class name, which made every checked cast decode that name with a
/// `TextDecoder` in JS before the `instanceof`: ~110 ns per cast against
/// ~13 ns (measured in Chrome 154), and `dyn_into::<Node>()` runs once per
/// mounted node — it took backend-web's batch decode from 7 ms to 31 ms
/// over the benchmark's 67 k-row rebuild. The constructor is read from
/// `globalThis` on each call rather than captured: a global the engine
/// lacks stays a clean `false`, and the load is an inline-cached property
/// read. Only classes something actually casts to keep their import —
/// LLD drops the rest, and their JS with them.
#[macro_export]
macro_rules! js_class {
    () => {};
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident $(: $parent:ident $(, $ancestor:ident)* )? = $class:literal ;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Clone)]
        $vis struct $name($crate::JsValue);

        impl $crate::cast::JsCast for $name {
            fn is_type_of(v: &$crate::JsValue) -> bool {
                // One import per class, its constructor named in the JS —
                // no class-name string per call (see the macro docs).
                $crate::import! {
                    fn instance_of(h: u32) -> u32 = concat!(
                        "(h) => { const C = globalThis.", $class,
                        "; return typeof C === 'function' && G.get(h) instanceof C ? 1 : 0; }"
                    );
                }
                // SAFETY: the snippet only reads the handle's slot.
                unsafe { instance_of(v.raw()) != 0 }
            }
            fn unchecked_from_js(v: $crate::JsValue) -> Self {
                $name(v)
            }
            fn unchecked_from_js_ref(v: &$crate::JsValue) -> &Self {
                // SAFETY: `$name` is `#[repr(transparent)]` over `JsValue`.
                unsafe { &*(v as *const $crate::JsValue as *const $name) }
            }
        }

        impl ::core::convert::AsRef<$crate::JsValue> for $name {
            fn as_ref(&self) -> &$crate::JsValue {
                &self.0
            }
        }

        impl ::core::convert::From<$name> for $crate::JsValue {
            fn from(v: $name) -> $crate::JsValue {
                v.0
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}({})", $class, self.0.raw())
            }
        }

        impl $name {
            /// The underlying handle.
            pub fn as_js(&self) -> &$crate::JsValue {
                &self.0
            }
        }

        $crate::js_class!(@deref $name $(: $parent $(, $ancestor)*)?);

        $crate::js_class! { $($rest)* }
    };
    // A root class derefs to `JsValue`, so — as with web-sys — every class
    // derefs down its chain to `JsValue` and `&window` passes where a
    // `&JsValue` is wanted.
    (@deref $name:ident) => {
        impl ::core::ops::Deref for $name {
            type Target = $crate::JsValue;
            fn deref(&self) -> &$crate::JsValue {
                &self.0
            }
        }
    };
    (@deref $name:ident : $parent:ident $(, $ancestor:ident)*) => {
        impl ::core::ops::Deref for $name {
            type Target = $parent;
            fn deref(&self) -> &$parent {
                <$parent as $crate::cast::JsCast>::unchecked_from_js_ref(&self.0)
            }
        }
        $crate::js_class!(@up $name, $parent $(, $ancestor)*);
    };
    (@up $name:ident, $($up:ident),*) => {
        $(
            impl ::core::convert::AsRef<$up> for $name {
                fn as_ref(&self) -> &$up {
                    <$up as $crate::cast::JsCast>::unchecked_from_js_ref(&self.0)
                }
            }
            impl ::core::convert::From<$name> for $up {
                fn from(v: $name) -> $up {
                    <$up as $crate::cast::JsCast>::unchecked_from_js(v.0)
                }
            }
        )*
    };
}
