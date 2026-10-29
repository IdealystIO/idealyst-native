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
//! ## Where things live
//!
//! Nothing is implemented here. The substrate lives in `runtime-shared`
//! (style engine, assets, animation, scheduling, robot, prop/handle
//! types), the reactive kernel in `runtime-world`, the scene model in
//! `runtime-scene`, and the capability traits + builtin handlers in
//! `runtime-vocabulary`.
//!
//! Backends do NOT depend on this crate. They consume `runtime-shared`
//! (substrate types), `runtime-scene` (`Host`/`Registry`) and
//! `runtime-vocabulary` (the caps traits) directly — this root is the
//! *author* surface, and its `glue` re-export deliberately shadows
//! several substrate names with authoring wrappers.

pub use runtime_vocabulary::glue::*;

// The proc-macro set.
pub use runtime_macros::{
    axis_values, component, component_styles, doc_scope, host_fn, idealyst_tool, jsx, lazy_component, props, recipe, robot_fn, stylesheet, ui,
    Remote,
    IdealystSchema,
    Key,
    Tokens,
};

// Generic `#[host_fn]` bounds (`Key` above is its derive, this the trait).
pub use runtime_vocabulary::host_types::{Key, KeyBytes, Numeric, Opaque, OpaqueBytes};

// `lazy!` is deprecated (use `#[component(lazy)]`); re-exported for
// compatibility while call sites migrate. The `allow` silences the
// deprecation warning on the re-export itself — use sites still warn.
#[allow(deprecated)]
pub use runtime_macros::lazy;

// `rx!` / `effect!` / `timeline!` / `animated!` — defined in the
// vocabulary (their `$crate` expansions target
// `runtime_vocabulary::glue`), re-exported under the historical names.
// (`effect` the MACRO coexists with glue's `effect` FN — the imports
// occupy different namespaces.)
pub use runtime_vocabulary::{animated, effect, register_style_token, rx, timeline};

// Shared-substrate `#[macro_export]` decl macros whose `$crate::…`
// expansions resolve against runtime-shared (where they and the types
// they construct — `assets::Typeface` for `typeface!`/`face!`,
// `assets::Asset` for `asset!`/`embed_asset!`, `Ref<H>` for `node_ref!` —
// live). A module re-export can't carry them, so they live here.
pub use runtime_shared::{asset, embed_asset, face, node_ref, typeface};

// The per-level logging macros, same story. `#[macro_export]` puts them at
// their defining crate's ROOT, so `pub use runtime_shared::logging` (in
// glue) re-exports the module's FUNCTIONS but not these — and authors have
// spelled `runtime_core::log_info!(…)` since 0.5.x; dropping this line
// would break them with nothing else in the build noticing.
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

#[cfg(all(test, feature = "async-driver"))]
mod _spawn_then_reachable {
    /// `spawn_then` must be reachable from the AUTHOR root — the guides
    /// (`sdks.md`, `server-functions.md`, `reactivity-in-depth.md`) all
    /// spell it `runtime_core::spawn_then`, and app code never depends on
    /// `runtime-vocabulary` directly.
    #[test]
    fn spawn_then_is_on_the_author_surface() {
        let _f: fn(std::future::Ready<i32>, fn(i32)) = crate::spawn_then;
        let _g: fn(&crate::ScopeAlive, std::future::Ready<i32>, fn(i32)) = crate::spawn_then_in;
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

#[cfg(test)]
mod _tokens_derive_reachable {
    /// `#[derive(Tokens)]` and the `Tokens` trait share one name on the
    /// author root, so the single import an app writes —
    /// `use runtime_core::Tokens` — brings the derive AND `.with` /
    /// `install` / `tokens()` into scope. Dropping either re-export would
    /// still compile every vocabulary test while breaking every app.
    #[test]
    fn one_import_brings_the_derive_and_its_methods() {
        use crate::{Color, Tokens};

        #[derive(Tokens, Clone)]
        #[tokens(default = base)]
        struct Theme {
            surface: Color,
        }
        fn base() -> Theme {
            Theme { surface: "#fff".into() }
        }

        let dark = base().with(|t| t.surface = "#000".into());
        assert_eq!(dark.surface, Color("#000".into()));
        assert_eq!(Theme::tokens().surface().name(), Some("surface"));
    }
}
