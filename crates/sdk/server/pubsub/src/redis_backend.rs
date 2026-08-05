//! Redis-backed [`PubSubBackend`] over Redis native Pub/Sub. Cross-instance: a
//! `PUBLISH` on one server reaches `SUBSCRIBE`rs on every server connected to the
//! same Redis. At-most-once with no buffering (standard Redis Pub/Sub) — a
//! subscriber that isn't connected at publish time misses the message.
//!
//! Topic == Redis channel (channels are arbitrary binary-safe strings, so no
//! name mangling). Each `subscribe` opens its own connection in subscriber mode;
//! `publish` reuses a shared multiplexed connection.

use crate::{PubSubBackend, PubSubError};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;

/// A [`PubSubBackend`] backed by Redis Pub/Sub. Clone shares the publish connection.
#[derive(Clone)]
pub struct RedisBackend {
    client: redis::Client,
    conn: ConnectionManager,
}

fn be<E: std::fmt::Display>(e: E) -> PubSubError {
    PubSubError::Backend(e.to_string())
}

impl RedisBackend {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1/`).
    pub async fn connect(url: &str) -> Result<Self, PubSubError> {
        Self::from_client(redis::Client::open(url).map_err(be)?).await
    }

    /// Build over an existing `redis::Client` — the shared-connection
    /// spelling. A `redis::Client` is cheap connection *config* (connections
    /// are opened from it per use), so the same client the app installs for
    /// `cache::RedisCache` / server-kit's rate limiter serves pub/sub too:
    /// one URL configured at boot, every consumer attached to it.
    pub async fn from_client(client: redis::Client) -> Result<Self, PubSubError> {
        let conn = ConnectionManager::new(client.clone()).await.map_err(be)?;
        Ok(Self { client, conn })
    }

    /// Build over the `redis::Client` already installed via
    /// `server::install_state` — the "provided context" spelling, mirroring
    /// `cache::RedisCache::from_installed`. Errors when no client is
    /// installed (boot-time misconfiguration).
    pub async fn from_installed() -> Result<Self, PubSubError> {
        let client = server::use_state::<redis::Client>().ok_or_else(|| {
            PubSubError::Backend(
                "RedisBackend::from_installed: no redis::Client installed; call \
                 server::install_state(client.clone()) at boot (or pass one via \
                 RedisBackend::from_client)"
                    .to_string(),
            )
        })?;
        Self::from_client(client).await
    }
}

#[async_trait]
impl PubSubBackend for RedisBackend {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), PubSubError> {
        let mut conn = self.conn.clone();
        // PUBLISH returns the receiver count; we don't need it.
        let _: i64 = conn.publish(topic, payload).await.map_err(be)?;
        Ok(())
    }

    async fn subscribe(
        &self,
        topic: &str,
    ) -> Result<BoxStream<'static, Vec<u8>>, PubSubError> {
        let mut pubsub = self.client.get_async_pubsub().await.map_err(be)?;
        pubsub.subscribe(topic).await.map_err(be)?;
        // `into_on_message` owns the connection, so the stream is `'static`.
        let stream = pubsub
            .into_on_message()
            .map(|msg| msg.get_payload_bytes().to_vec());
        Ok(Box::pin(stream))
    }
}
