//! Server functions SDK. See the crate's `Cargo.toml` header for
//! architecture; see the `#[server]` attribute for the surface.
//!
//! # The 30-second tour
//!
//! ```ignore
//! use server::{server, ServerError};
//!
//! // One function, two compilations.
//! #[server]
//! async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
//!     Ok(a + b)
//! }
//!
//! // ----- server binary -----
//! // cargo build --features server
//! #[tokio::main]
//! async fn main() {
//!     server::serve("0.0.0.0:3000".parse().unwrap()).await.unwrap();
//! }
//!
//! // ----- client binary -----
//! // cargo build  (default features)
//! fn init() {
//!     server::configure(server::ClientConfig {
//!         base_url: "http://localhost:3000".into(),
//!     });
//! }
//!
//! async fn demo() {
//!     // Identical call site on both sides:
//!     let sum = add(2, 3).await.unwrap();
//!     assert_eq!(sum, 5);
//! }
//! ```

mod error;

pub use error::{ServerError, ServerFnReturn, TransportError};

/// The `#[server]` attribute macro. See [`server_macros::server`] for
/// the parsing rules and emitted shape; re-exported here so authors
/// `use server::server;` rather than depending on the macro crate
/// directly.
pub use server_macros::server;

// =============================================================================
// Extractor wrappers — present on BOTH builds (they appear in the
// author's shared `#[server]` fn signature). The resolution machinery
// they rely on is server-only (see below).
// =============================================================================

mod extract;
pub use extract::{Auth, Cookies, Extension, Headers, State};

// =============================================================================
// Client-only surface: configuration + the `call()` the macro emits.
// =============================================================================

#[cfg(not(feature = "server"))]
mod batch;
#[cfg(not(feature = "server"))]
mod cancel;
#[cfg(not(feature = "server"))]
mod client;
#[cfg(not(feature = "server"))]
pub use batch::{batch, BatchScope};
#[cfg(not(feature = "server"))]
pub use cancel::{with_cancel, with_cancel_token, WithCancel};
#[cfg(not(feature = "server"))]
pub use client::{configure, ClientConfig};

// =============================================================================
// Server-only surface: axum router + bind/serve.
// =============================================================================

#[cfg(feature = "server")]
mod extractors;
#[cfg(feature = "server")]
mod middleware;
#[cfg(feature = "server")]
mod runtime;
#[cfg(feature = "server")]
pub use extract::{Context, ContextBuilder, FromContext};
#[cfg(feature = "server")]
pub use extractors::{install_state, use_request_header, use_request_headers, use_state};
#[cfg(feature = "server")]
pub use middleware::{from_fn, install_middleware, FnMiddleware, Middleware, MiddlewareFuture};
#[cfg(feature = "server")]
pub use runtime::{router, schema_for, serve};

// =============================================================================
// Macro-facing internals. Not stable surface — re-exports here are
// the only sanctioned coupling between the macro and the SDK.
// =============================================================================

#[doc(hidden)]
pub mod __private {
    pub use inventory;

    use crate::error::TransportError;
    #[cfg(not(feature = "server"))]
    use crate::error::ServerFnReturn;
    use serde::{de::DeserializeOwned, Serialize};
    use std::future::Future;
    use std::pin::Pin;

    /// One registered server function. The macro emits one of these
    /// per `#[server]` fn via `inventory::submit!`; the runtime walks
    /// them at startup to populate the axum router.
    ///
    /// `path` is the wire path under `/_srv/` (e.g. `path: "add"` →
    /// served at `POST /_srv/add`).
    ///
    /// `handler` takes the raw request body, decodes the args tuple,
    /// awaits the function, and encodes the `Result` for the wire. Its
    /// error half is a [`TransportError`] — it fails only when the
    /// input/output codec itself fails (the user's `Err` is encoded into
    /// the success bytes, never surfaced here).
    pub struct ServerFnEntry {
        pub path: &'static str,
        /// Wire schema hash (serialized arg types + return type). Used to
        /// distinguish a version-drift codec failure from a same-version
        /// one, and for `strict_version` pre-decode gating.
        pub schema: u64,
        /// `#[server(strict_version)]`: reject a mismatched client schema
        /// up front, before decoding.
        pub strict: bool,
        pub handler: fn(
            Vec<u8>,
        )
            -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send>>,
    }

    // SAFETY: ServerFnEntry holds only static data + a fn pointer; both
    // are trivially Send + Sync.
    unsafe impl Send for ServerFnEntry {}
    unsafe impl Sync for ServerFnEntry {}

    inventory::collect!(ServerFnEntry);

    /// Decode the args tuple from the request body. Used by the macro's
    /// server-side expansion.
    pub fn decode_args<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, TransportError> {
        serde_json::from_slice(bytes).map_err(|e| TransportError::Codec(e.to_string()))
    }

    /// Encode the function's `Result` for the wire. Used by the macro's
    /// server-side expansion.
    pub fn encode_result<T: Serialize>(value: &T) -> Result<Vec<u8>, TransportError> {
        serde_json::to_vec(value).map_err(|e| TransportError::Codec(e.to_string()))
    }

    /// Clone the current request's [`Context`](crate::extract::Context).
    /// The macro's server-side handler calls this once, then resolves
    /// each `#[ctx]` / reserved-wrapper extractor param against it via
    /// [`FromContext`](crate::extract::FromContext).
    #[cfg(feature = "server")]
    pub fn current_context() -> crate::extract::Context {
        crate::extractors::current_context()
    }

    /// The client-side call function. The macro's client-side
    /// expansion emits a single call to this with the args tuple +
    /// the expected return type.
    ///
    /// Returns `Ret` (not `Result<Ret, _>`) — transport / codec
    /// failures are surfaced through `Ret`'s [`ServerFnReturn`] impl,
    /// which is how `Result<T, ServerError>` (and any user type
    /// implementing the trait) folds network errors into its own
    /// error variant.
    #[cfg(not(feature = "server"))]
    pub async fn call<Args, Ret>(path: &str, schema: u64, args: &Args) -> Ret
    where
        Args: Serialize,
        Ret: DeserializeOwned + ServerFnReturn,
    {
        crate::client::call_impl::<Args, Ret>(path, schema, args).await
    }
}
