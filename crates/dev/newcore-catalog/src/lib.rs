//! Test-only crate: the host for the MCP catalog-emission suite.
//!
//! The suite itself is `tests/registers_component.rs`, which `include!`s
//! the body from `tests/shared/catalog_emission.rs`. This lib is
//! intentionally empty — see `Cargo.toml` for why the suite needs its own
//! crate rather than living in `mcp-catalog`.
