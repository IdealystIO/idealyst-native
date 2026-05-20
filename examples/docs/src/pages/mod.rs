//! One module per documentation page. Each exposes:
//!
//! - `pub fn page() -> Primitive` — emitted by the `docs!` macro;
//!   the navigator wires this into the matching route.
//! - `pub static PAGE_META: PageMeta` — also emitted by `docs!`;
//!   registered in [`crate::registry::PAGES`] for the MCP server
//!   and any other introspection tool.

// Pages migrated to the `docs!` macro (one `docs! { ... }` per file).
pub mod backends;
pub mod building_a_theme_system;
pub mod components;
pub mod dev_tools;
pub mod icons;
pub mod lists;
pub mod navigation;
pub mod overview;
pub mod portal;
pub mod primitives;
pub mod quickstart;
pub mod reactivity;
pub mod refs;
pub mod robot;
pub mod styles;
pub mod third_party_primitives;
pub mod wgpu_native_api;
pub mod writing_a_backend;

// Hand-built — embeds the `Simulator` component for a live preview.
// The `docs!` macro only emits text-flavored blocks, so a page with
// a custom `Primitive` in the middle is built directly.
pub mod simulator_demo;

// Pages still hand-built. To be migrated to the `docs!` macro when
// their markdown drafts are written.
pub mod cli;
pub mod platforms;
pub mod ui_dsl;

// "Macros" page is named with an underscore to avoid clashing with
// any future `macros` module Rust might suggest at this path.
pub mod macros_page;
