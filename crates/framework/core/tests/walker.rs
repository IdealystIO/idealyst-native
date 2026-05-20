//! Walker suite — primitive dispatch, lifecycle, refs.
//!
//! Tests the build walker against `MockBackend`, asserting on the
//! exact sequence of backend calls. Each module covers one slice of
//! the walker's behavior; the MockBackend's event log is the shared
//! observation tool.
//!
//! Run scoped:
//!   cargo test -p framework-core --test walker primitives
//!   cargo test -p framework-core --test walker lifecycle
//!   cargo test -p framework-core --test walker refs

#[path = "common/mod.rs"]
mod common;

#[path = "walker/primitives.rs"]
mod primitives;
#[path = "walker/lifecycle.rs"]
mod lifecycle;
#[path = "walker/refs.rs"]
mod refs;
