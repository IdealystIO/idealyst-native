//! Server-tier cache: a byte-oriented KV-with-TTL abstraction.
//!
//! [`Cache`] is deliberately minimal — `get` / `set`(+TTL) / `delete` —
//! because that's the whole contract a cache can promise. Anything that
//! needs *atomicity across a read-modify-write* (rate-limit buckets,
//! counters) is NOT a cache and gets its own purpose-built store
//! (`server_kit::LimitStore`); expressing those through get/set would
//! race across instances. The two share the same **app-provided
//! connection**, not the same trait.
//!
//! ```ignore
//! // boot — one Redis client, provided as context for every consumer:
//! let client = redis::Client::open(cfg.redis_url)?;
//! server::install_state(client.clone());
//! server::install_state::<std::sync::Arc<dyn cache::Cache>>(
//!     std::sync::Arc::new(cache::RedisCache::from_installed()),
//! );
//!
//! // a server fn — the cache as injected context:
//! server_kit::context! {
//!     /// The app cache.
//!     pub struct AppCache(std::sync::Arc<dyn cache::Cache>);
//! }
//!
//! #[server]
//! async fn dashboard(#[ctx] cache: AppCache) -> Result<Dash, ServerError> {
//!     if let Some(hit) = cache.get_json::<Dash>("dash:v1").await? {
//!         return Ok(hit);
//!     }
//!     let fresh = compute().await?;
//!     cache.set_json("dash:v1", &fresh, Some(Duration::from_secs(60))).await?;
//!     Ok(fresh)
//! }
//! ```
//!
//! Values are bytes; [`CacheExt::get_json`] / [`CacheExt::set_json`] add
//! typed convenience over serde_json. Expiry contract: a `set` with
//! `Some(ttl)` makes the entry unreadable after `ttl` (backends may
//! evict lazily); `None` means no expiry.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

mod memory;
pub use memory::MemoryCache;

#[cfg(feature = "redis")]
mod redis_cache;
#[cfg(feature = "redis")]
pub use redis_cache::RedisCache;

/// Cache backend failure (connection down, protocol error). Consumers
/// decide their own degradation policy — a cache miss and a cache outage
/// are deliberately distinct.
#[derive(Debug)]
pub struct CacheError(pub String);

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cache error: {}", self.0)
    }
}
impl std::error::Error for CacheError {}

/// Future returned by [`Cache`] methods — boxed so the trait is
/// object-safe (`Arc<dyn Cache>` is the shape apps install and SDKs
/// accept).
pub type CacheFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CacheError>> + Send + 'a>>;

/// The cache contract. Implement over any KV backend; `Arc<Self>` /
/// `Arc<dyn Cache>` are also implementations (blanket impls below), so
/// APIs can accept `impl Cache` and callers can pass shared handles.
pub trait Cache: Send + Sync + 'static {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>>;
    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>, ttl: Option<Duration>)
        -> CacheFuture<'a, ()>;
    fn delete<'a>(&'a self, key: &'a str) -> CacheFuture<'a, ()>;
}

impl<C: Cache + ?Sized> Cache for std::sync::Arc<C> {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>> {
        (**self).get(key)
    }
    fn set<'a>(
        &'a self,
        key: &'a str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> CacheFuture<'a, ()> {
        (**self).set(key, value, ttl)
    }
    fn delete<'a>(&'a self, key: &'a str) -> CacheFuture<'a, ()> {
        (**self).delete(key)
    }
}

/// Typed JSON convenience over the byte contract. Blanket-implemented
/// for every [`Cache`].
pub trait CacheExt: Cache {
    /// `get` + JSON-decode. A value that fails to decode reads as a
    /// MISS (`Ok(None)`) — a schema change invalidates naturally
    /// instead of erroring forever.
    fn get_json<'a, T: serde::de::DeserializeOwned>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<T>, CacheError>> + Send + 'a>> {
        Box::pin(async move {
            match self.get(key).await? {
                Some(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
                None => Ok(None),
            }
        })
    }

    /// JSON-encode + `set`.
    fn set_json<'a, T: serde::Serialize + Sync>(
        &'a self,
        key: &'a str,
        value: &'a T,
        ttl: Option<Duration>,
    ) -> CacheFuture<'a, ()> {
        Box::pin(async move {
            let bytes = serde_json::to_vec(value)
                .map_err(|e| CacheError(format!("encode {key}: {e}")))?;
            self.set(key, bytes, ttl).await
        })
    }
}

impl<C: Cache + ?Sized> CacheExt for C {}
