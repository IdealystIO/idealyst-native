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
//! On web the job runs in a Web Worker that instantiates the same app module
//! (`web_glue::worker`), with the argument and result postcard-encoded — no
//! `SharedArrayBuffer`, so no COOP/COEP headers and embedding keeps working, and
//! no wasm-bindgen. On native it runs on a `std::thread`. A job that panics
//! resolves to [`OffloadError::Canceled`] on both.
//!
//! The crate that defines a job needs no extra dependencies: `#[offload::job]`
//! generates nothing on any target (the handle carries the function pointer), and
//! call sites only name `offload`.

mod error;
mod handle;

pub use error::OffloadError;
pub use handle::Handle;
/// Marks a free function as an offload job. A no-op marker on every target:
/// [`handle!`] carries the function pointer, which is all either backend needs.
pub use offload_macro::job;

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod imp;

pub use imp::run;
