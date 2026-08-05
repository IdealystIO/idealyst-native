//! Catalog-emission suite — the whole catalog contract, run against the
//! crate that defines it.
//!
//! One compilation exercises BOTH `__mcp` inventory anchors and proves
//! they land in the same catalog:
//!
//! - `#[component]` → `::runtime_vocabulary::glue::__mcp` (its emission
//!   passes through `runtime_macros::finish`, which rewrites absolute
//!   `::runtime_core::` path heads);
//! - `#[derive(IdealystSchema)]` / `#[idealyst_tool]` / `recipe!` /
//!   `doc_scope!` → `::runtime_core::__mcp`, resolved through the alias
//!   below (those entry points do NOT go through `finish`).
//!
//! Both anchors re-export this crate, so exactly one inventory must
//! result — which is what [`catalog_inventory_is_identical_across_cores`]
//! pins, via a sorted fingerprint of every macro-emitted slice whose
//! expected value is a literal in the suite source.
//!
//! The test-target name is load-bearing: `module_path!()` for an
//! integration test is the *target* name, and the fingerprint compares
//! module paths — so this file must stay `registers_component.rs`.
//!
//! Hosting: the suite body still lives at
//! `crates/dev/newcore-catalog/tests/shared/catalog_emission.rs`, which
//! is where it was parked while the pre-v2 core still owned the other
//! anchor and mcp-catalog could not name the facade (a dev-dep could not
//! be made optional, and an unconditional one would have flipped the
//! macro lowering for the dying leg too). With one core that constraint
//! is gone — mcp-catalog takes the facade as a dev-dep directly, cycle
//! and all (cargo permits cycles through dev-dependencies). The body
//! should be folded back in here and `crates/dev/newcore-catalog`
//! removed; until then this `include!` is the single source.
//!
//! Invocation: `cargo test -p mcp-catalog`.

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../dev/newcore-catalog/tests/shared/catalog_emission.rs"
));
