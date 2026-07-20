//! Redis-backed [`LimitStore`](crate::LimitStore) — shared buckets across
//! server instances. Feature `redis`.
//!
//! The Redis connection is **app-provided context, exactly like a
//! database**: the app opens one `redis::Client`, installs it for server
//! fns to use (`server::install_state` + a `context!` wrapper if
//! desired), and hands the same client to the limiter — one connection
//! config, many consumers, none of them owning it:
//!
//! ```ignore
//! let client = redis::Client::open(cfg.redis_url)?;
//! server::install_state(client.clone());
//! server_kit::context! {
//!     /// App cache / queue handle, usable from any #[server] fn.
//!     pub struct Cache(redis::Client);
//! }
//! server_kit::install_middleware(
//!     server_kit::rate_limit()
//!         .key_by(…)
//!         .store(server_kit::RedisLimitStore::from_installed()),
//! );
//! ```
//!
//! The bucket algorithm is the same continuous-refill token bucket as
//! [`MemoryLimitStore`](crate::MemoryLimitStore), executed as one atomic
//! Lua script per admit (read → refill → take/report → write), so
//! concurrent instances can't double-spend a token. State is a Redis
//! hash per `(route, key)` with a TTL of 2× the window — idle buckets
//! evict themselves and re-materialize full, which never changes
//! anyone's effective limit (mirrors the memory store's sweep).
//!
//! Timekeeping is wall-clock (`SystemTime`) rather than `Instant`
//! because the timestamps are shared across machines; NTP-level skew
//! shifts refill by milliseconds, which is noise at these windows.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::sync::OnceCell;

use crate::rate_limit::{Admitted, Limit, LimitStore, StoreFuture, StoreUnavailable};

/// Atomic token-bucket admit. KEYS[1] = the bucket hash. ARGV: capacity,
/// refill rate (tokens/ms), now (epoch ms), ttl (ms). Returns
/// `{allowed (0|1), retry_secs}`.
const ADMIT_LUA: &str = r#"
local capacity = tonumber(ARGV[1])
local rate_per_ms = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])
local h = redis.call('HMGET', KEYS[1], 'tokens', 'ts')
local tokens = tonumber(h[1])
local ts = tonumber(h[2])
if tokens == nil or ts == nil then
  tokens = capacity
  ts = now
end
local elapsed = now - ts
if elapsed < 0 then elapsed = 0 end
tokens = tokens + elapsed * rate_per_ms
if tokens > capacity then tokens = capacity end
local allowed = 0
local retry = 0
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
else
  retry = math.ceil((1 - tokens) / rate_per_ms / 1000)
end
redis.call('HSET', KEYS[1], 'tokens', tokens, 'ts', now)
redis.call('PEXPIRE', KEYS[1], ttl)
return {allowed, retry}
"#;

/// See the module docs. Cheap to construct; the multiplexed connection
/// is established lazily on first use and shared thereafter.
pub struct RedisLimitStore {
    client: redis::Client,
    conn: OnceCell<ConnectionManager>,
    namespace: String,
}

impl RedisLimitStore {
    /// Build over an app-provided client (the same instance the app may
    /// expose to server fns as context).
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            conn: OnceCell::new(),
            namespace: "srvkit:rl".to_string(),
        }
    }

    /// Build over the `redis::Client` already installed via
    /// `server::install_state` — the "provided context" spelling.
    ///
    /// # Panics
    ///
    /// Panics when no client is installed: this runs at boot while the
    /// middleware stack is assembled, and a missing store dependency
    /// should stop the server, not surface per-request.
    pub fn from_installed() -> Self {
        let client = server::use_state::<redis::Client>().unwrap_or_else(|| {
            panic!(
                "RedisLimitStore::from_installed: no redis::Client installed; call \
                 server::install_state(client.clone()) before building the middleware stack \
                 (or pass one explicitly via RedisLimitStore::new)"
            )
        });
        Self::new(client)
    }

    /// Key-prefix namespace (default `srvkit:rl`) — set when several
    /// apps share one Redis.
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    async fn connection(&self) -> Result<ConnectionManager, StoreUnavailable> {
        self.conn
            .get_or_try_init(|| async {
                // Tight timeouts: the limiter fails OPEN on store errors,
                // and that failure must be FAST. The manager's defaults
                // retry with minutes of backoff — a dead Redis at boot
                // would stall every request for the whole retry schedule
                // before the fail-open kicked in (observed: ~7 min).
                // Bounded here to ~2s worst case per request until the
                // first successful connect; once established, the
                // manager reconnects in the background and errors fast.
                let config = ConnectionManagerConfig::new()
                    .set_connection_timeout(Duration::from_secs(1))
                    .set_response_timeout(Duration::from_secs(1))
                    .set_number_of_retries(1);
                ConnectionManager::new_with_config(self.client.clone(), config)
                    .await
                    .map_err(|e| StoreUnavailable(format!("redis connect: {e}")))
            })
            .await
            .cloned()
    }
}

impl LimitStore for RedisLimitStore {
    fn admit<'a>(&'a self, route: &'a str, key: &'a str, limit: Limit) -> StoreFuture<'a> {
        Box::pin(async move {
            let mut conn = self.connection().await?;
            let bucket_key = format!("{}:{{{route}}}:{key}", self.namespace);
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let rate_per_ms = limit.per_second() / 1000.0;
            let ttl_ms = (limit.window().as_millis() as u64).saturating_mul(2).max(1000);
            let (allowed, retry): (i64, i64) = redis::Script::new(ADMIT_LUA)
                .key(&bucket_key)
                .arg(limit.capacity())
                .arg(rate_per_ms)
                .arg(now_ms)
                .arg(ttl_ms)
                .invoke_async(&mut conn)
                .await
                .map_err(|e| StoreUnavailable(format!("redis admit: {e}")))?;
            if allowed == 1 {
                Ok(Admitted::Allowed)
            } else {
                Ok(Admitted::Denied { retry_secs: retry.max(1) as u64 })
            }
        })
    }
}
