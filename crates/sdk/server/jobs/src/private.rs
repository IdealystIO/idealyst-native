//! Internals the `#[job]` macro expands references to. **Not** a stable API —
//! everything here is an implementation detail of code generation. Authors
//! reach jobs through `#[job]`, `jobs::configure`, and `jobs::worker`.

pub use inventory;
pub use serde_json;

use crate::error::{JobError, QueueError};

/// Re-export so generated `enqueue` bodies can funnel to the configured backend
/// without naming the private `config` module.
pub use crate::config::enqueue;
pub use crate::model::{Backoff, OutgoingJob};

/// Decode a job payload (the serialized handler-argument tuple) inside a
/// generated handler.
pub fn decode_payload<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, JobError> {
    serde_json::from_slice(bytes).map_err(|e| JobError::new(format!("payload decode: {e}")))
}

/// Encode handler arguments into a job payload inside a generated `enqueue`.
pub fn encode_payload<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, QueueError> {
    serde_json::to_vec(value).map_err(|e| QueueError::Codec(e.to_string()))
}

/// A registered job handler — one submitted per `#[job]` via `inventory`, and
/// collected by the worker runtime to build its name → handler map. Only
/// compiled in the worker/server build (`feature = "worker"`); the wasm client
/// never runs handlers.
#[cfg(feature = "worker")]
pub struct JobEntry {
    /// The job's dispatch name (the `#[job]` fn name, or its `name = "…"`).
    pub name: &'static str,
    /// Decodes the payload, resolves extractors, runs the body.
    pub handler: fn(
        Vec<u8>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), JobError>> + Send>,
    >,
}

// SAFETY: holds only static data + a fn pointer, both Send + Sync.
#[cfg(feature = "worker")]
unsafe impl Send for JobEntry {}
#[cfg(feature = "worker")]
unsafe impl Sync for JobEntry {}

#[cfg(feature = "worker")]
inventory::collect!(JobEntry);
