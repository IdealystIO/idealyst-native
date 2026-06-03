//! Hand-curated registration table for framework-owned [`ScopeEntry`]s.
//!
//! These are the scopes the framework ships unconditionally — the
//! counterpart to the `primitives` / `utilities` / `states` tables.
//! Library scopes (idea-ui, app crates) are declared with the
//! `doc_scope!` macro at their own module sites; framework scopes live
//! here because `runtime-core` can't self-reference `::runtime_core::__mcp`
//! (no `extern crate self`), and the surface they cover (the
//! `runtime_core` utilities) is itself registered from this crate's
//! `utilities` table.
//!
//! Its `module_path` is the `runtime_core` crate root, making it the
//! ambient scope for every `runtime_core` / `runtime_core::*` utility
//! (see [`crate::ResolvedCatalog::scope_for`]).

use crate::ScopeEntry;

inventory::submit! {
    ScopeEntry {
        slug: "core",
        title: "Core",
        docs: "Framework runtime surface — the `runtime_core` utilities (platform, color, time, theme, layout, math) that author code calls from plain Rust, outside `ui!`.",
        module_path: "runtime_core",
        order: 0,
    }
}
