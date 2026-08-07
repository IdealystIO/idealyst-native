//! # runtime-core — the `runtime_core::…` author surface
//!
//! This crate IS the author-facing root. Every app, SDK, component
//! library, and example in the tree reaches the framework through
//! `runtime_core::…` paths. Preserving the spelling is the point:
//! thousands of doc, example, and test references keep working with no
//! tree-wide rename.
//!
//! Deliberately paper-thin: the entire surface lives in
//! [`runtime_vocabulary::glue`] — **extend glue** (with vocabulary-suite
//! tests when the extension carries logic), never this crate, so the
//! items stay next to the machinery they wrap.
//!
//! The only things that live HERE are the items a module re-export
//! cannot carry:
//!
//! - the proc-macro set (`ui!`, `#[component]`, `#[props]`,
//!   `stylesheet!`, `#[lazy_component]`, …) — re-exported from
//!   `runtime-macros`, so their EMISSION targets
//!   `::runtime_vocabulary::glue`;
//! - `rx!` / `effect!` / `timeline!` / `animated!` — decl macros whose
//!   `$crate::…` expansions must resolve against a root (the vocabulary
//!   defines the bodies, see `runtime_vocabulary::rx`);
//! - `typeface!` / `face!` / `node_ref!` — the same, anchored at
//!   `runtime-shared`.
//!
//! ## History
//!
//! Until the runtime-v2 deletion this package held the pre-v2 walker:
//! the `Element` enum, the 159-method `Backend` mega-trait, the render
//! walker, `Bound`/builders, the `External` table and the legacy
//! reactive arena. All of it is gone; the surviving substrate lives in
//! `runtime-shared` (style engine, assets, animation, scheduling,
//! robot, prop/handle types), the reactive kernel in `runtime-world`,
//! the scene model in `runtime-scene`, and the capability traits +
//! builtin handlers in `runtime-vocabulary`. During the migration this
//! root lived in a separate `runtime-facade` package reached through an
//! `extern crate runtime_facade as runtime_core;` alias in every
//! consumer; that package and all 105 alias lines are gone —
//! `runtime_core` is the real crate again.
//!
//! Backends do NOT depend on this crate. They consume `runtime-shared`
//! (substrate types), `runtime-scene` (`Host`/`Registry`) and
//! `runtime-vocabulary` (the caps traits) directly — this root is the
//! *author* surface, and its `glue` re-export deliberately shadows
//! several substrate names with authoring wrappers.

pub use runtime_vocabulary::glue::*;

// The proc-macro set.
pub use runtime_macros::{
    component, doc_scope, idealyst_tool, jsx, lazy_component, props, recipe, stylesheet, ui,
    IdealystSchema,
};

// `lazy!` is deprecated (use `#[component(lazy)]`); re-exported for
// compatibility while call sites migrate. The `allow` silences the
// deprecation warning on the re-export itself — use sites still warn.
#[allow(deprecated)]
pub use runtime_macros::lazy;

// `rx!` / `effect!` / `timeline!` / `animated!` — defined in the
// vocabulary (their `$crate` expansions target
// `runtime_vocabulary::glue`), re-exported under the historical names.
// (`effect` the MACRO coexists with glue's `effect` FN — the imports
// occupy different namespaces.) `timeline!` and `animated!` must be the
// vocabulary's mirrors: the pre-v2 expansions anchored their scope and
// cleanup in the deleted arena, which made them inert here (the frozen
// Switch-thumb bug).
pub use runtime_vocabulary::{animated, effect, register_style_token, rx, timeline};

// Shared-substrate `#[macro_export]` decl macros whose `$crate::…`
// expansions resolve against runtime-shared (where they and the types
// they construct — `assets::Typeface` for `typeface!`/`face!`, `Ref<H>`
// for `node_ref!` — live). A module re-export can't carry them, so they
// live here.
pub use runtime_shared::{face, node_ref, typeface};

// The per-level logging macros, same story. `#[macro_export]` puts them at
// their defining crate's ROOT, so `pub use runtime_shared::logging` (in
// glue) re-exports the module's FUNCTIONS but not these — and authors have
// spelled `runtime_core::log_info!(…)` since 0.5.x, when `logging` was a
// module of this crate and the macros landed at this root for free. Keeping
// them here preserves that surface; without it the break is silent in the
// migration guide's inventory (they are not among its removed 19 names).
pub use runtime_shared::{log_debug, log_error, log_info, log_warn};

#[cfg(test)]
mod tests {
    /// Path pin: authors spell the identity wrapper `runtime_core::ByIdentity`.
    /// It is DEFINED in runtime-shared and reaches here only via glue's
    /// re-export plus this crate's `pub use runtime_vocabulary::glue::*`. If
    /// either link is dropped the type still exists and every one of its own
    /// tests still passes, while every app breaks — the same silent-surface
    /// failure mode as the `log_*` macros above.
    #[test]
    fn by_identity_resolves_on_the_author_surface() {
        let a = crate::ByIdentity::new(String::from("payload"));
        let b = a.clone();
        assert!(a == b, "clones are the same instance");
        assert!(
            a != crate::ByIdentity::new(String::from("payload")),
            "an equal-valued but separate allocation is a different instance"
        );
        assert_eq!(&*a, "payload", "Deref reaches the payload");

        let arc: std::sync::Arc<str> = std::sync::Arc::from("shared");
        let c = crate::ByIdentityArc::from_ptr(arc.clone());
        assert!(c == crate::ByIdentityArc::from_ptr(arc));
    }
}

#[cfg(test)]
mod _scope_alive_reachable {
    /// `ScopeAlive` must be reachable from the AUTHOR/SDK root, not just
    /// from `runtime-vocabulary`: a third-party scene `Registry` extension
    /// wires native callbacks the framework never sees, so it needs the
    /// same guard, and it depends on this crate rather than the vocabulary.
    #[test]
    fn scope_alive_is_on_the_author_surface() {
        let t = crate::ScopeAlive::immortal();
        assert!(t.get());
    }
}
