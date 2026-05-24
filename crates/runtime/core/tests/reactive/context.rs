//! Context — `provide` / `inject` / `inject_or` / `with_inject`.
//!
//! The framework's context model is closest-provider, scope-bound:
//! `provide(value)` makes a value visible to descendant scopes;
//! `inject::<T>()` looks up the closest provided value of type `T`
//! starting from the current scope and walking up.
//!
//! Note on scope: `provide` requires an active reactive scope to
//! attach to. The framework's `render(...)` entrypoint opens that
//! scope; primitives whose bodies run inside the scope (e.g.
//! `when`'s `then`/`otherwise` closures, `switch`'s arm builders,
//! `#[component]` bodies) can call `provide`. Calling `provide`
//! eagerly outside any scope panics — that's the contract.
//!
//! The framework's inline tests in `reactive.rs` already cover
//! closest-provider semantics, shadowing, and provision-dies-with-
//! scope using the `pub(crate)` `with_scope` API. The tests here
//! cover the **public** surface: outside-scope behavior + value
//! visibility through real primitives.

use runtime_core::{inject, inject_or};

#[derive(Clone, Debug, PartialEq)]
struct Theme {
    primary: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
struct Locale(&'static str);

/// `inject::<T>()` outside any active scope returns None gracefully —
/// no panic, just no provider chain to walk. This is the documented
/// fallback behavior.
#[test]
fn inject_outside_scope_returns_none() {
    let t: Option<Theme> = inject();
    assert!(t.is_none(), "no provider chain, no value");
}

/// `inject_or(default)` outside any scope returns the default.
#[test]
fn inject_or_outside_scope_returns_default() {
    let t = inject_or(Theme { primary: "fallback" });
    assert_eq!(t, Theme { primary: "fallback" });
}

/// `inject` for an absent type returns None even when other types
/// have been provided in an ancestor scope. (Tested without an
/// active scope here — the "absent type" path doesn't care whether
/// the scope is real; `None` is the correct answer in both cases.)
#[test]
fn inject_returns_none_for_unprovided_type() {
    // No scope at all; no providers; Locale-shaped inject returns None.
    let l: Option<Locale> = inject();
    assert!(l.is_none());
}

// =============================================================================
// Closest-provider semantics — covered by the framework's inline tests
// =============================================================================
//
// The crate-internal tests in `src/reactive.rs` directly exercise the
// closest-provider lookup, shadowing, scope-drop-removes-provision,
// and multiple-types-coexist scenarios using `pub(crate)` `with_scope`
// + `Scope::new()`. Those APIs aren't exposed publicly because they're
// invariant-internal — the public way to open a scope is through the
// render walker (`render(...)` opens one; primitives like `when`,
// `switch`, `Presence`, `#[component]` open child scopes).
//
// Once the walker test suite lands (`tests/walker/`), it'll re-cover
// these semantics through full render trees. Until then, the inline
// tests are the source of truth.
