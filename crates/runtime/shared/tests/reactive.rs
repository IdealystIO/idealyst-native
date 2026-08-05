//! The legacy reactive arena suite — `runtime_shared::reactive`.
//!
//! RELOCATED wholesale from `runtime-core/tests/reactive{.rs,/**}`
//! (deletion baseline §4.2, SV-R). The subject — the arena, its
//! generational handles, `Scope`, `batch`, `memo`, context injection,
//! `on`/`on_defer`, `on_cleanup`, `reducer`, `resource`, `split` — all
//! lives in `runtime-shared` and SURVIVES the walker's deletion. It is
//! still load-bearing: `shared/src/style.rs`'s token registry drives it,
//! and every `runtime_shared::*` reactive export is public author API
//! through the facade.
//!
//! Nothing here referenced the old core except by import path, with two
//! exceptions (both flagged in-place with a `RELOCATION NOTE`):
//! `dispose::scope_owned_signal_freed_by_scope_after_early_dispose_is_safe`
//! and `split::regression_stale_write_half_is_a_safe_noop_after_scope_drop`
//! opened their owning scope by RENDERING through the walker's
//! `TestRuntime`. The scope was scenery; both now drive
//! `reactive::Scope` / `reactive::with_scope` directly, which exercises
//! the same batched-free path with the walker out of the picture.
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
