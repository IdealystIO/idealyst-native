//! One module per documentation page. Each exposes:
//!
//! - `pub fn page() -> Primitive` — emitted by the `docs!` macro;
//!   the navigator wires this into the matching route.
//! - `pub static PAGE_META: PageMeta` — also emitted by `docs!`;
//!   registered in [`crate::registry::PAGES`] for the MCP server
//!   and any other introspection tool.

// Pages migrated to the `docs!` macro (one `docs! { ... }` per file).
pub mod backends;
pub mod components;
pub mod dev_tools;
pub mod icons;
pub mod lists;
pub mod navigation;
pub mod overview;
pub mod primitives;
pub mod quickstart;
pub mod reactivity;
pub mod refs;
pub mod robot;
pub mod styles;
pub mod writing_a_backend;

// Pages still hand-built. To be migrated to the `docs!` macro when
// their markdown drafts are written.
pub mod cli;
pub mod platforms;
pub mod ui_dsl;

// "Macros" page is named with an underscore to avoid clashing with
// any future `macros` module Rust might suggest at this path.
pub mod macros_page;
