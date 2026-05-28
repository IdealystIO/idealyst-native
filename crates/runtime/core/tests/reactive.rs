//! Reactive suite — top-level entry. Subsidiary test modules live in
//! `tests/reactive/`. Scope at the cargo level:
//!
//! ```bash
//! cargo test -p runtime-core --test reactive                  # everything in this suite
//! cargo test -p runtime-core --test reactive smoke::          # just smoke checks
//! cargo test -p runtime-core --test reactive topology::diamond  # one test
//! ```
//!
//! Each module under `tests/reactive/` covers one slice of the
//! reactive surface. Keep modules narrow — when a module exceeds a
//! few hundred lines, split it again.

#[path = "common/mod.rs"]
mod common;

#[path = "reactive/smoke.rs"]
mod smoke;
#[path = "reactive/topology.rs"]
mod topology;
#[path = "reactive/on_cleanup.rs"]
mod on_cleanup;
#[path = "reactive/batch.rs"]
mod batch_tests;
#[path = "reactive/context.rs"]
mod context;
#[path = "reactive/memo.rs"]
mod memo_tests;
#[path = "reactive/on_and_defer.rs"]
mod on_and_defer;
#[path = "reactive/reducer.rs"]
mod reducer_tests;
#[path = "reactive/nested_update.rs"]
mod nested_update;

// Resource is feature-gated — only compiled in when async-driver is on.
#[cfg(feature = "async-driver")]
#[path = "reactive/resource.rs"]
mod resource_tests;
