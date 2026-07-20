//! Redis-backed [`Cache`] — shared across instances. Feature `redis`.
//!
//! The client is app-provided context (see the crate docs): the same
//! `redis::Client` the app installs for its server fns and hands to
//! server-kit's rate limiter. Connection management mirrors
//! `RedisLimitStore`: a lazily-established multiplexed manager with
//! TIGHT timeouts, so an unreachable Redis surfaces as a fast
//! [`CacheError`] (bounded seconds) instead of stalling requests on the
//! manager's minutes-long default retry schedule.

use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::AsyncCommands;
use tokio::sync::OnceCell;

use crate::{Cache, CacheError, CacheFuture};

pub struct RedisCache {
    client: redis::Client,
    conn: OnceCell<ConnectionManager>,
    namespace: String,
}

impl RedisCache {
    /// Build over an app-provided client.
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            conn: OnceCell::new(),
            namespace: "cache".to_string(),
        }
    }

    /// Build over the `redis::Client` already installed via
    /// `server::install_state` — the "provided context" spelling.
    ///
    /// # Panics
    ///
    /// Panics when no client is installed (boot-time misconfiguration
    /// should stop the server, not surface per-request).
    pub fn from_installed() -> Self {
        let client = server::use_state::<redis::Client>().unwrap_or_else(|| {
            panic!(
                "RedisCache::from_installed: no redis::Client installed; call \
                 server::install_state(client.clone()) at boot (or pass one via RedisCache::new)"
            )
        });
        Self::new(client)
    }

    /// Key-prefix namespace (default `cache`) — set when several apps
    /// share one Redis.
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    fn key(&self, key: &str) -> String {
        format!("{}:{key}", self.namespace)
    }

    async fn connection(&self) -> Result<ConnectionManager, CacheError> {
        self.conn
            .get_or_try_init(|| async {
                let config = ConnectionManagerConfig::new()
                    .set_connection_timeout(Duration::from_secs(1))
                    .set_response_timeout(Duration::from_secs(1))
                    .set_number_of_retries(1);
                ConnectionManager::new_with_config(self.client.clone(), config)
                    .await
                    .map_err(|e| CacheError(format!("redis connect: {e}")))
            })
            .await
            .cloned()
    }
}

impl Cache for RedisCache {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let mut conn = self.connection().await?;
            conn.get::<_, Option<Vec<u8>>>(self.key(key))
                .await
                .map_err(|e| CacheError(format!("redis get {key}: {e}")))
        })
    }

    fn set<'a>(
        &'a self,
        key: &'a str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> CacheFuture<'a, ()> {
        Box::pin(async move {
            let mut conn = self.connection().await?;
            let k = self.key(key);
            match ttl {
                // SET PX — TTL floor of 1ms (PX 0 is a redis error).
                Some(ttl) => conn
                    .set_ex::<_, _, ()>(k, value, ttl.as_secs().max(1))
                    .await
                    .map_err(|e| CacheError(format!("redis set {key}: {e}"))),
                None => conn
                    .set::<_, _, ()>(k, value)
                    .await
                    .map_err(|e| CacheError(format!("redis set {key}: {e}"))),
            }
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> CacheFuture<'a, ()> {
        Box::pin(async move {
            let mut conn = self.connection().await?;
            conn.del::<_, ()>(self.key(key))
                .await
                .map_err(|e| CacheError(format!("redis del {key}: {e}")))
        })
    }
}
