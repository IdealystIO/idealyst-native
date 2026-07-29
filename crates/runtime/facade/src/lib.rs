//! # runtime-facade — old-core author surface, new-core implementation
//!
//! The alias target for `extern crate runtime_facade as runtime_core;`
//! (see Cargo.toml for the mechanism). Deliberately paper-thin: the
//! entire compatibility surface lives in [`runtime_vocabulary::glue`] —
//! **extend glue** (with vocabulary-suite tests when the extension
//! carries logic), never this crate, so the mirrors stay next to the
//! machinery they wrap and every aliased SDK crate picks them up.
//!
//! The only things that live HERE are the items a module re-export
//! cannot carry:
//!
//! - the proc-macro set old `runtime_core`'s root re-exports (`ui!`,
//!   `#[component]`, `#[props]`, `stylesheet!`, …) — re-exported from
//!   `runtime-macros`, whose `new-core` feature this crate enables, so
//!   their EMISSION is already retargeted at `::runtime_vocabulary::glue`;
//! - `rx!` — a decl macro whose `$crate::…` expansion must resolve
//!   against this root (the vocabulary defines the new-core body, see
//!   `runtime_vocabulary::rx`).

pub use runtime_vocabulary::glue::*;

// The proc-macro set (mirror of runtime-core lib.rs's re-export block).
// `lazy`/`lazy_component` are NOT mirrored: `#[component(lazy)]` is a
// documented new-core deferral (no lazy/chunk-mount prim yet) and the
// macro fails loudly under `new-core` — re-exporting the attribute here
// would just move the same error.
pub use runtime_macros::{
    component, doc_scope, idealyst_tool, jsx, props, recipe, stylesheet, ui, IdealystSchema,
};

// `rx!` / `effect!` — defined in the vocabulary (their `$crate`
// expansions target `runtime_vocabulary::glue`), re-exported under the
// old names. (`effect` the MACRO coexists with glue's `effect` FN — the
// imports occupy different namespaces.)
pub use runtime_vocabulary::{effect, rx};

// Old-core `#[macro_export]` decl macros whose `$crate::…` expansions
// resolve against runtime-core itself and construct types glue already
// re-exports (`assets::Typeface` for `typeface!`/`face!`, `Ref<H>` for
// `node_ref!`). Same items both cores — a module re-export can't carry
// them, so they live here (the website retarget's `typeface.rs` and
// `node_ref!` call sites are the consumers).
pub use runtime_core::{face, node_ref, typeface};
