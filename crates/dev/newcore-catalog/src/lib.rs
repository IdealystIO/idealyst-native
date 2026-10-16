//! Test-only crate: the host for the MCP catalog-emission suite.
//!
//! The suite itself is `tests/registers_component.rs`, which `include!`s
//! the body from `tests/shared/catalog_emission.rs`. This lib is
//! intentionally empty. `mcp-catalog`'s own `tests/registers_component.rs`
//! compiles the same body (plus its round-trip and scanner tests).
