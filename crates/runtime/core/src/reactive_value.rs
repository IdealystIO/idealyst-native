//! `Reactive<T>` — a prop value that is either a fixed snapshot or a
//! live reactive source, plus the `IntoProp<T>` coercion shim that
//! lets `ui!`/`jsx!` call sites pass a plain value, a `Signal<T>`, or
//! an `rx!(...)` expression into the *same* prop field.
//!
//! ## Why this exists
//!
//! Leaf primitives (`Text`, etc.) already accept both static and
//! reactive content via `IntoTextSource` — `text(move || …)` is
//! reactive, `text("hi")` is static. But a USER component
//! (`Typography`, `Button`, …) receives its props as a struct, and a
//! `content: String` field is a one-time snapshot: a `.get()` read at
//! the call site is evaluated once and the binding is severed. So
//! `Typography(content = format!("{}", sig.get()))` never updates,
//! even though the identical `Text(...)` call does.
//!
//! `Reactive<T>` closes that gap. A component declares a dynamic prop
//! as `Reactive<T>` instead of `T`; it can then route the value to a
//! leaf reactively (e.g. `text(content)` via
//! [`IntoTextSource for Reactive<String>`]). The value stays live.
//!
//! ## Type-driven, not heuristic
//!
//! Reactivity is decided by the value's TYPE, never by scanning the
//! call site for `.get()` (the same principle the `for`-loop
//! iteration lowering follows):
//!
//! - `content = "hi".to_string()` → `Reactive::Static` (snapshot).
//! - `content = some_signal`       → `Reactive::Dynamic` (live).
//! - `content = rx!(format!("{}", c.get()))` → `Reactive::Dynamic`.
//!
//! `rx!(expr)` is the explicit opt-in for an inline computed value —
//! the reactive-prop analog of `bind!` for `text_fmt!`. You opt IN
//! with one token rather than the framework guessing from a substring.
//!
//! ## The `IntoProp<T>` coercion shim
//!
//! Invocation macros wrap every prop value in
//! `IntoProp::into_prop(value)`. The reflexive blanket
//! `impl<T> IntoProp<T> for T` makes this a no-op for every existing
//! prop type — including call sites that already write `X.into()`
//! (the field type pins the target, so there is no ambiguous middle
//! type and no double conversion). The targeted reactive impls
//! (`T`/`&str`/`Signal<T>` → `Reactive<T>`) are what make a reactive
//! field accept a bare value, a signal, or an `rx!`. Net effect:
//! reactive props with ZERO call-site churn.

use std::rc::Rc;

use crate::derive::Derived;
use crate::reactive::Signal;
use crate::sources::{IntoTextSource, TextSource};

/// A prop value that is either a fixed snapshot (`Static`) or a live
/// reactive computation (`Dynamic`). Construct via `From`/`into_prop`
/// (static + signals) or the [`rx!`](crate::rx) macro (computed).
pub enum Reactive<T> {
    /// A one-time value. No subscription, no reactivity.
    Static(T),
    /// A live computation. Reading it inside a reactive scope (e.g. a
    /// leaf primitive's update Effect) subscribes to whatever signals
    /// the closure touches.
    Dynamic(Rc<dyn Fn() -> T>),
}

impl<T> Reactive<T> {
    /// Build a `Dynamic` from a closure. `rx!(expr)` expands to this.
    pub fn derive<F: Fn() -> T + 'static>(f: F) -> Self {
        Reactive::Dynamic(Rc::new(f))
    }

    /// True if this is a `Static` snapshot — lets a component keep the
    /// static fast-path (no Effect) when no reactive prop was passed.
    pub fn is_static(&self) -> bool {
        matches!(self, Reactive::Static(_))
    }
}

impl<T: Clone> Reactive<T> {
    /// Read the current value. For `Dynamic`, this runs the closure —
    /// call it inside a reactive scope to subscribe to the signals it
    /// reads.
    pub fn get(&self) -> T {
        match self {
            Reactive::Static(v) => v.clone(),
            Reactive::Dynamic(f) => f(),
        }
    }

    /// Convert into a `Fn() -> T` closure for routing into any
    /// primitive constructor that accepts a reactive closure
    /// (`with_style`, animated bindings, …). `Static` becomes a
    /// constant closure that subscribes to nothing.
    pub fn into_closure(self) -> Rc<dyn Fn() -> T>
    where
        T: 'static,
    {
        match self {
            Reactive::Static(v) => Rc::new(move || v.clone()),
            Reactive::Dynamic(f) => f,
        }
    }
}

impl<T: Clone> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        match self {
            Reactive::Static(v) => Reactive::Static(v.clone()),
            Reactive::Dynamic(f) => Reactive::Dynamic(f.clone()),
        }
    }
}

impl<T: Default> Default for Reactive<T> {
    fn default() -> Self {
        Reactive::Static(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Reactive<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reactive::Static(v) => f.debug_tuple("Reactive::Static").field(v).finish(),
            Reactive::Dynamic(_) => f.write_str("Reactive::Dynamic(<closure>)"),
        }
    }
}

// =============================================================================
// Coercions into Reactive<T> — used at call sites via `.into()`
// =============================================================================
//
// A component declares a dynamic prop as `Reactive<T>`. Call sites
// produce one by:
//   - a bare value          → `Reactive::Static`  (via `From<T>`),
//   - a string literal       → `Reactive::Static`  (via `From<&str>`;
//     `ui!`/`jsx!` already append `.into()` to literals),
//   - a `Signal<T>`/`memo`   → `Reactive::Dynamic` (via `From<Signal>`),
//   - an `rx!(expr)`         → `Reactive::Dynamic` (already a `Reactive`).
//
// Non-string-literal dynamic values opt in explicitly at the call site
// (`sig.into()` / `rx!(...)`), keeping reactivity visible — the
// type-driven counterpart to `bind!` for `text_fmt!`.

/// Bare value → `Reactive::Static`. The blanket covers `String`,
/// `bool`, `i32`, theme refs, … — anything used as a static prop.
///
/// Coherence: this `From<T> for Reactive<T>` and
/// `From<Signal<T>> for Reactive<T>` (below) don't overlap — unifying
/// them needs `T = Signal<T>`, which fails the occurs-check.
impl<T> From<T> for Reactive<T> {
    fn from(v: T) -> Self {
        Reactive::Static(v)
    }
}

/// `&str` → `Reactive<String>` (the blanket above only covers an owned
/// `String`). `ui!`/`jsx!` append `.into()` to string literals, so
/// `content = "hi"` lands here as a static snapshot.
impl From<&str> for Reactive<String> {
    fn from(s: &str) -> Self {
        Reactive::Static(s.to_string())
    }
}

/// `Signal<T>` (or a `memo`, which is also a `Signal`) → reactive
/// `Reactive<T>`. Reading it subscribes, so the component updates when
/// the signal changes: `content = my_signal.into()`.
impl<T: Clone + 'static> From<Signal<T>> for Reactive<T> {
    fn from(sig: Signal<T>) -> Self {
        Reactive::Dynamic(Rc::new(move || sig.get()))
    }
}

// =============================================================================
// Routing Reactive<String> into a text leaf
// =============================================================================

/// Lets a component render a `Reactive<String>` prop with `text(...)`:
/// `Static` → a fixed string (no Effect); `Dynamic` → a
/// `Derived<String>` the leaf re-evaluates on signal changes. This is
/// the seam that makes `Typography(content = …)` reactive.
impl IntoTextSource for Reactive<String> {
    fn into_text_source(self) -> TextSource {
        match self {
            Reactive::Static(s) => TextSource::Static(s),
            Reactive::Dynamic(compute) => TextSource::Bound(Derived {
                method: "",
                inputs: Vec::new(),
                initial: Vec::new(),
                compute,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Signal;

    #[test]
    fn bare_value_into_reactive_is_static() {
        // `From<T>` blanket — an owned value becomes a static snapshot.
        let r: Reactive<String> = String::from("hi").into();
        assert!(r.is_static());
        assert_eq!(r.get(), "hi");

        // `&str` → `Reactive<String>` (the literal path `ui!` produces).
        let r2: Reactive<String> = "yo".into();
        assert!(r2.is_static());
        assert_eq!(r2.get(), "yo");
    }

    #[test]
    fn option_string_coerces_into_reactive_option_string() {
        // `Reactive<Option<String>>` is how optional text props (Switch
        // /Field `label`, Alert `body`) are typed. The call-site idioms
        // `label = Some("x".to_string())` and `label = None` must coerce
        // via the blanket `From<Option<String>>` — this is what lets the
        // migration land with zero call-site churn (the orphan rule
        // blocks the `Option<Reactive<String>>` alternative).
        let some: Reactive<Option<String>> = Some("hi".to_string()).into();
        assert!(some.is_static());
        assert_eq!(some.get(), Some("hi".to_string()));

        let none: Reactive<Option<String>> = Option::<String>::None.into();
        assert!(none.is_static());
        assert_eq!(none.get(), None);

        // A `Signal<Option<String>>` arrives `Dynamic` (live presence
        // AND content).
        let sig: Signal<Option<String>> = Signal::new(Some("a".to_string()));
        let live: Reactive<Option<String>> = sig.into();
        assert!(!live.is_static());
        assert_eq!(live.get(), Some("a".to_string()));
        sig.set(None);
        assert_eq!(live.get(), None);
    }

    #[test]
    fn signal_into_reactive_is_dynamic_and_live() {
        let sig: Signal<i32> = Signal::new(1);
        let r: Reactive<i32> = sig.into();
        assert!(!r.is_static());
        assert_eq!(r.get(), 1);
        // The binding is live: a later signal write is reflected on the
        // next read (the component reading inside an Effect would
        // re-fire here).
        sig.set(42);
        assert_eq!(r.get(), 42);
    }

    #[test]
    fn rx_macro_builds_a_live_dynamic() {
        let count: Signal<i32> = Signal::new(0);
        let r: Reactive<String> = crate::rx!(format!("n={}", count.get()));
        assert!(!r.is_static());
        assert_eq!(r.get(), "n=0");
        count.set(5);
        assert_eq!(r.get(), "n=5");
    }

    #[test]
    fn reactive_string_routes_to_text_source() {
        // Static → TextSource::Static (no Effect path).
        let st: Reactive<String> = Reactive::Static("hi".to_string());
        assert!(matches!(st.into_text_source(), TextSource::Static(s) if s == "hi"));

        // Dynamic → TextSource::Bound carrying the compute closure.
        let count: Signal<i32> = Signal::new(3);
        let dy: Reactive<String> = crate::rx!(format!("n={}", count.get()));
        match dy.into_text_source() {
            TextSource::Bound(d) => assert_eq!((d.compute)(), "n=3"),
            _ => panic!("expected TextSource::Bound from a Dynamic Reactive"),
        }
    }

    #[test]
    fn default_is_static_default() {
        let r: Reactive<String> = Reactive::default();
        assert!(r.is_static());
        assert_eq!(r.get(), "");
    }

    #[test]
    fn into_closure_static_is_constant() {
        let r: Reactive<i32> = Reactive::Static(9);
        let f = r.into_closure();
        assert_eq!(f(), 9);
        assert_eq!(f(), 9);
    }
}
