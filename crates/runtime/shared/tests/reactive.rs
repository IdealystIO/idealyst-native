//! The substrate reactive arena suite — `runtime_shared::reactive`: the
//! arena, its generational handles, `Scope`, `batch`, `memo`, context
//! injection, `on`/`on_defer`, `on_cleanup`, `reducer`, `resource`,
//! `split`. It is load-bearing: `shared/src/style.rs`'s token registry
//! drives it, and the `runtime_shared::*` reactive exports are public API.
//!
//! `runtime-world`'s own inline suite duplicates much of this by
//! coincidence, but NOT these: `Signal::dispose()` plus the
//! unowned-signal leak diagnostic, `batch()` semantics (the world kernel
//! has no analogue by design — staging replaced it), and `inject_or`
//! defaults. Losing this file would lose those outright.
//!
//! ```bash
//! cargo test -p runtime-shared --test reactive                 # everything
//! cargo test -p runtime-shared --test reactive smoke::         # one module
//! cargo test -p runtime-shared --test reactive topology::diamond
//! ```

#[path = "reactive/counted.rs"]
mod counted;

#[path = "reactive/dispose.rs"]
mod dispose;
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
#[path = "reactive/split.rs"]
mod split;
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
