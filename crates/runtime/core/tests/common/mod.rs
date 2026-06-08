//! Shared test infrastructure for runtime-core integration tests.
//!
//! Modules:
//! - [`mock_backend`] — a `Backend` impl that records every method
//!   call into an event log so tests can assert on the exact sequence
//!   of backend operations.
//! - [`counted`] — `counted_effect` and `counted_memo` helpers that
//!   wrap an `Effect` / `Memo` in a fire-counter, so tests can assert
//!   re-run counts (not just final values).
//! - [`runtime`] — a `TestRuntime` that owns a `MockBackend`, installs
//!   a synchronous scheduler + time source, and drives the framework
//!   without needing a real platform.
//!
//! Every test file under `tests/<concern>/...` declares
//! `mod common;` and pulls these in via `common::*`.

pub mod counted;
pub mod mock_backend;
pub mod runtime;

#[allow(unused_imports)]
pub use counted::{counted_effect, counted_memo, FireCounter};
#[allow(unused_imports)]
pub use mock_backend::{
    fire_all_layouts, reset_layout_subs, BatchOpSummary, Event, MockBackend, MockBackendConfig,
    NodeId,
};
#[allow(unused_imports)]
pub use runtime::TestRuntime;
