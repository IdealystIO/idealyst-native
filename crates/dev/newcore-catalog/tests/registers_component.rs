//! Catalog-emission suite.
//!
//! The suite body lives in `shared/catalog_emission.rs` and is
//! `include!`d here. One compilation exercises BOTH `__mcp` inventory
//! anchors and proves they land in the same catalog:
//!
//! - `#[component]` → `::runtime_vocabulary::glue::__mcp` (its emission
//!   passes through `runtime_macros::finish`, which rewrites absolute
//!   `::runtime_core::` path heads);
//! - `#[derive(IdealystSchema)]` / `#[idealyst_tool]` / `recipe!` /
//!   `doc_scope!` → `::runtime_core::__mcp`, resolved through the alias
//!   below (those entry points do NOT go through `finish`).
//!
//! The test-target name is load-bearing: `module_path!()` for an
//! integration test is the test crate's name, and the inventory
//! fingerprint in the body compares module paths.
//!
//! Invocation: `cargo test -p newcore-catalog`.

include!("shared/catalog_emission.rs");
