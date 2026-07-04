//! The pub/sub backend abstraction every transport implements.

use crate::PubSubError;
use async_trait::async_trait;
use futures_util::stream::BoxStream;

/// A pluggable publish/subscribe transport. `Send + Sync` so one instance is
/// shared behind an `Arc` across every publisher and subscriber.
///
/// Semantics: **fan-out, at-most-once, ephemeral.** Every current subscriber to
/// a topic receives each published message; there is no buffering or replay for
/// subscribers that aren't connected at publish time (durable delivery is what
/// `jobs` is for).
#[async_trait]
pub trait PubSubBackend: Send + Sync {
    /// Broadcast `payload` to every current subscriber of `topic`.
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), PubSubError>;

    /// Subscribe to `topic`, yielding each subsequently published payload until
    /// the returned stream is dropped.
    async fn subscribe(&self, topic: &str)
        -> Result<BoxStream<'static, Vec<u8>>, PubSubError>;
}
