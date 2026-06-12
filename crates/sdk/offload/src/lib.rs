//! Run a CPU-heavy function off the main thread, with one platform-agnostic call
//! site. See the crate's `Cargo.toml` for the high-level pitch.
//!
//! ```ignore
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Req { /* ... */ }
//! #[derive(Serialize, Deserialize)]
//! struct Out { /* ... */ }
//!
//! // Define the heavy job ONCE (a free fn; arg + return are serde types):
//! #[offload::job]
//! fn rasterize(req: Req) -> Out { /* expensive, pure CPU work */ todo!() }
//!
//! // Call it the SAME way on every platform:
//! async fn go(req: Req) -> Result<Out, offload::OffloadError> {
//!     offload::run(offload::handle!(rasterize), &req).await
//! }
//! ```
//!
//! On web the job runs in a Web Worker (background thread) via `wasmworker` — no
//! `SharedArrayBuffer`, so no COOP/COEP headers and embedding keeps working. On
//! native it runs on a `std::thread`. The public surface (`job`, `handle!`,
//! `run`, [`OffloadError`]) is backend-agnostic so the web implementation can be
//! replaced later without touching callers.
//!
//! ## Consumer dependency note (web)
//!
//! On `wasm32`, `#[offload::job]` re-exports wasmworker's `#[webworker_fn]`, whose
//! generated code references the `wasmworker` and `wasm_bindgen` crates **by
//! name**. A crate that *defines* a job must therefore add both as direct
//! `wasm32` dependencies:
//!
//! ```toml
//! [target.'cfg(target_arch = "wasm32")'.dependencies]
//! wasmworker = { version = "0.4", features = ["macros"] }
//! wasm-bindgen = "0.2"
//! ```
//!
//! The call site (`offload::run(offload::handle!(f), &req)`) still only names
//! `offload`; only the `#[job]` expansion needs the two crates in scope.

mod error;
pub use error::OffloadError;

// ── Web ─────────────────────────────────────────────────────────────────────
// `#[offload::job]` IS wasmworker's `#[webworker_fn]` (registers the fn for the
// worker); `offload::handle!` IS wasmworker's `webworker!` (builds the typed
// handle). `run` dispatches to the global worker pool. See `web.rs`.
#[cfg(target_arch = "wasm32")]
pub use wasmworker::{webworker as handle, webworker_fn as job};

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

// ── Native ──────────────────────────────────────────────────────────────────
// `#[offload::job]` is a no-op (the job is an ordinary fn run on a thread);
// `offload::handle!` is defined in `native.rs` (exported at the crate root via
// `#[macro_export]`). `run` spawns a `std::thread`. See `native.rs`.
#[cfg(not(target_arch = "wasm32"))]
pub use offload_macro::job;

#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod imp;

pub use imp::run;
#[cfg(not(target_arch = "wasm32"))]
pub use imp::Handle;
