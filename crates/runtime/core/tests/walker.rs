//! Walker suite — primitive dispatch, lifecycle, refs.
//!
//! Tests the build walker against `MockBackend`, asserting on the
//! exact sequence of backend calls. Each module covers one slice of
//! the walker's behavior; the MockBackend's event log is the shared
//! observation tool.
//!
//! Run scoped:
//!   cargo test -p runtime-core --test walker primitives
//!   cargo test -p runtime-core --test walker lifecycle
//!   cargo test -p runtime-core --test walker refs
//!   cargo test -p runtime-core --test walker batched_repeat

#[path = "common/mod.rs"]
mod common;

#[path = "walker/primitives.rs"]
mod primitives;
#[path = "walker/lifecycle.rs"]
mod lifecycle;
#[path = "walker/rebuild.rs"]
mod rebuild;
#[path = "walker/refs.rs"]
mod refs;
#[path = "walker/batched_repeat.rs"]
mod batched_repeat;
#[path = "walker/control_flow.rs"]
mod control_flow;
#[path = "walker/key_events.rs"]
mod key_events;
