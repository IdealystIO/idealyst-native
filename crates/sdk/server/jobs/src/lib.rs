//! Server-tier background-job / queue SDK. See `Cargo.toml` for the pitch.
//!
//! # The 30-second tour
//!
//! ```ignore
//! use jobs::{job, JobError};
//! use serde::{Serialize, Deserialize};
//!
//! // Define a unit of deferred work ONCE.
//! #[job]
//! async fn send_email(to: String, subject: String) -> Result<(), JobError> {
//!     // ... runs on a worker, off the request path ...
//!     Ok(())
//! }
//!
//! // ----- enqueue (from anywhere server-side, e.g. a #[server] fn) -----
//! async fn signup(addr: String) {
//!     send_email::enqueue(addr, "Welcome".into())
//!         .delay(std::time::Duration::from_secs(30))
//!         .await
//!         .unwrap();
//! }
//!
//! // ----- worker binary (built --features worker) -----
//! #[tokio::main]
//! async fn main() {
//!     jobs::configure(jobs::MemoryBackend::new()); // or Redis / SQS / Postgres
//!     jobs::worker().run().await.unwrap();
//! }
//! ```
//!
//! # Shape
//!
//! - [`QueueBackend`] is the queue abstraction; [`MemoryBackend`] is the
//!   in-process reference. Redis / SQS / Postgres backends are feature-gated.
//! - `#[job]` (re-exported from `jobs-macros`) generates a typed `enqueue`
//!   surface + registers the handler. It reuses `server`'s `State<T>` / `#[ctx]`
//!   extractors, so a job body injects app state exactly like a `#[server]` fn.
//! - [`worker`] drains the queue — as a dedicated process (`.run()`) or
//!   in-process next to `server::serve` (`.spawn()`).

mod backend;
mod builder;
mod config;
mod error;
mod model;

pub use backend::QueueBackend;
pub use builder::Enqueue;
pub use config::{configure, configure_from_env, configured_backend, enqueue};
pub use error::{JobError, QueueError};
pub use model::{
    Backoff, JobId, OutgoingJob, ReserveOpts, ReservedJob, DEFAULT_MAX_ATTEMPTS,
};

#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "memory")]
pub use memory::{Clock, MemoryBackend, SystemClock};

// Serialization helpers shared by the persistent backends.
#[cfg(any(feature = "redis", feature = "postgres", feature = "sqs"))]
mod persist;

#[cfg(feature = "redis")]
mod redis_backend;
#[cfg(feature = "redis")]
pub use redis_backend::RedisBackend;

#[cfg(feature = "postgres")]
mod postgres_backend;
#[cfg(feature = "postgres")]
pub use postgres_backend::PostgresBackend;

#[cfg(feature = "sqs")]
mod sqs_backend;
#[cfg(feature = "sqs")]
pub use sqs_backend::SqsBackend;

/// The `#[job]` attribute macro. See `jobs-macros` for the emitted shape;
/// re-exported here so authors write `use jobs::job;` rather than depending on
/// the macro crate directly. (Path-qualified `jobs::job` deliberately distinct
/// from the unrelated `offload::job` CPU-offload macro.)
pub use jobs_macros::job;

#[cfg(feature = "worker")]
mod worker;
#[cfg(feature = "worker")]
pub use worker::{worker, Worker};

#[doc(hidden)]
#[path = "private.rs"]
pub mod __private;
