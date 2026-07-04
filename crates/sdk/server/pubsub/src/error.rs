//! Error type for the pub/sub SDK.

use thiserror::Error;

/// A publish/subscribe failure.
#[derive(Debug, Error)]
pub enum PubSubError {
    /// The backend (Redis / Postgres / …) reported an error.
    #[error("pubsub backend error: {0}")]
    Backend(String),
    /// A message could not be (de)serialized.
    #[error("pubsub codec error: {0}")]
    Codec(String),
    /// `pubsub::configure(...)` was never called.
    #[error("no pubsub backend configured; call pubsub::configure(...) at startup")]
    NotConfigured,
}
