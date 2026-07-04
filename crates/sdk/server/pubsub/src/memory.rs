//! In-process [`PubSubBackend`] over `tokio::sync::broadcast` — the single-node
//! reference and the crate's test substrate.
//!
//! One `broadcast::Sender` per topic, created on first publish or subscribe.
//! Fan-out and (correctly) at-most-once: a subscriber only sees messages
//! published *after* it subscribed, and a slow subscriber that overflows the
//! channel drops the oldest messages (broadcast lag) rather than blocking
//! publishers. Cross-process it does nothing — that's what `redis` / `postgres`
//! are for.

use crate::{PubSubBackend, PubSubError};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Default per-topic ring-buffer capacity. Messages beyond this that a
/// subscriber hasn't consumed are dropped for that subscriber (broadcast lag).
const DEFAULT_CAPACITY: usize = 1024;

/// In-process pub/sub. Cheap to clone — clones share the topic registry.
#[derive(Clone)]
pub struct MemoryBackend {
    topics: Arc<Mutex<HashMap<String, broadcast::Sender<Vec<u8>>>>>,
    capacity: usize,
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Build with a custom per-topic buffer capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            capacity: capacity.max(1),
        }
    }

    /// The sender for `topic`, created on first use.
    fn sender(&self, topic: &str) -> broadcast::Sender<Vec<u8>> {
        let mut topics = self.topics.lock().unwrap();
        topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }
}

#[async_trait]
impl PubSubBackend for MemoryBackend {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), PubSubError> {
        // `send` errors only when there are zero receivers — a no-op fan-out,
        // not a failure.
        let _ = self.sender(topic).send(payload.to_vec());
        Ok(())
    }

    async fn subscribe(
        &self,
        topic: &str,
    ) -> Result<BoxStream<'static, Vec<u8>>, PubSubError> {
        let rx = self.sender(topic).subscribe();
        // Drop lag/closed errors — the subscriber sees the messages it can keep up with.
        let stream = BroadcastStream::new(rx).filter_map(|r| async move { r.ok() });
        Ok(Box::pin(stream))
    }
}
