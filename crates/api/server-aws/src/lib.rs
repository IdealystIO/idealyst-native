//! AWS Lambda adapter for the `server` (`#[server]`) SDK.
//!
//! The `server` crate composes every linked `#[server]` fn into an
//! [`axum::Router`] (`server::router()`), dispatching each call at
//! `POST /_srv/<path>` and batched calls at `POST /_srv/_batch`. An
//! `axum::Router` is a [`tower::Service`], and the AWS Lambda Rust runtime
//! ([`lambda_http`]) runs any such service against API Gateway / Function URL
//! events. So hosting the whole server-fn API as one Lambda is a thin wrapper:
//!
//! ```ignore
//! // src/bin/lambda.rs   (built with --features server)
//! #[tokio::main]
//! async fn main() -> Result<(), server_aws::Error> {
//!     // Cold-start init runs once per execution environment and is reused
//!     // across warm invocations — install DB pools / state / middleware here.
//!     server::install_state(/* ... */);
//!     server_aws::run().await
//! }
//! ```
//!
//! # What ports, what doesn't
//!
//! - **HTTP `#[server]` fns** (including the `_batch` route) port as-is. The
//!   client's existing transport posts to `<base>/_srv/<fn>`; a Function URL or
//!   API Gateway route maps straight onto the router. Schema-drift headers,
//!   cookies, middleware and extractors all work unchanged — it is the same
//!   dispatch core.
//! - **`#[channel]` / `#[subscription]`** (WebSockets) and **`#[sse]`** do NOT
//!   work over plain Lambda request/response. WebSockets need an API Gateway
//!   WebSocket API (connect/disconnect/message model, no persistent socket in
//!   the function); SSE needs a Lambda Function URL with response streaming
//!   (`InvokeMode: RESPONSE_STREAM`). Those are separate adapters; this one is
//!   the unary-HTTP path.

/// Re-exported so the author's `main` can name the error type without a direct
/// `lambda_http`/`lambda_runtime` dependency: `Result<(), server_aws::Error>`.
pub use lambda_http::Error;

/// Run every linked `#[server]` fn as a single Lambda handler.
///
/// Equivalent to `run_router(server::router())`. Call this from a
/// `#[tokio::main] async fn main()` after any cold-start state/middleware
/// install. The future resolves only when the runtime shuts down.
///
/// The router is built once (here, at cold start) and reused for every warm
/// invocation — `server::router()` walks the inventory a single time.
pub async fn run() -> Result<(), Error> {
    run_router(server::router()).await
}

/// Run a caller-composed [`axum::Router`] as a Lambda handler.
///
/// Use this instead of [`run`] when you need to compose the server-fn router
/// with extra routes (health checks, a static fallback, custom middleware
/// layers) before handing it to the runtime — the same escape hatch
/// `server::router()` gives over `server::serve()`:
///
/// ```ignore
/// let app = server::router().route("/health", axum::routing::get(|| async { "ok" }));
/// server_aws::run_router(app).await
/// ```
pub async fn run_router(app: axum::Router) -> Result<(), Error> {
    lambda_http::run(app).await
}
