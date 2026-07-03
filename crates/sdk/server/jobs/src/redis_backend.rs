//! Redis-backed [`QueueBackend`]. A reliable-queue design (not fire-and-forget
//! `LPUSH`/`BRPOP`): every reserved job is tracked with a visibility deadline so
//! a crashed worker's job is re-delivered, giving at-least-once semantics.
//!
//! Keys, per queue `q` (namespace prefix `ns`, default `jobs`):
//! - `ns:job:{id}`         HASH  — the job (name, payload, attempts, …)
//! - `ns:q:{q}:ready`      LIST  — ids ready to run (RPOP end is the head)
//! - `ns:q:{q}:scheduled`  ZSET  — id → epoch-ms it becomes ready (delay/backoff)
//! - `ns:q:{q}:inflight`   ZSET  — id → epoch-ms its lease expires
//! - `ns:q:{q}:dead`       LIST  — dead-lettered ids (job hash retained)
//!
//! `reserve` runs a single Lua script so the "promote due scheduled → reclaim
//! expired leases → pop one → mark in-flight → bump attempts" sequence is
//! atomic across concurrent workers.

use crate::persist::BackoffRepr;
use crate::{JobId, OutgoingJob, QueueBackend, QueueError, ReserveOpts, ReservedJob};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::time::{Duration, SystemTime};

/// A [`QueueBackend`] backed by Redis. Cheap to clone (shares the multiplexed
/// connection).
#[derive(Clone)]
pub struct RedisBackend {
    conn: ConnectionManager,
    ns: String,
}

/// Atomic reserve. KEYS: ready, scheduled, inflight. ARGV: now_ms, lease_deadline_ms,
/// job-key-prefix. Returns `nil` or `{id, attempts}`.
const RESERVE_LUA: &str = r#"
local now = tonumber(ARGV[1])
local lease = tonumber(ARGV[2])
local prefix = ARGV[3]
-- promote scheduled jobs whose delay has elapsed
local due = redis.call('ZRANGEBYSCORE', KEYS[2], '-inf', now)
for _, id in ipairs(due) do
  redis.call('ZREM', KEYS[2], id)
  redis.call('LPUSH', KEYS[1], id)
end
-- reclaim leases that timed out
local expired = redis.call('ZRANGEBYSCORE', KEYS[3], '-inf', now)
for _, id in ipairs(expired) do
  redis.call('ZREM', KEYS[3], id)
  redis.call('LPUSH', KEYS[1], id)
end
local id = redis.call('RPOP', KEYS[1])
if not id then return nil end
redis.call('ZADD', KEYS[3], lease, id)
local attempts = redis.call('HINCRBY', prefix .. id, 'attempts', 1)
return {id, attempts}
"#;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RedisBackend {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1/`) with the default
    /// `jobs` key namespace.
    pub async fn connect(url: &str) -> Result<Self, QueueError> {
        Self::connect_ns(url, "jobs").await
    }

    /// Connect with a custom key namespace (lets multiple apps share one Redis).
    pub async fn connect_ns(url: &str, ns: &str) -> Result<Self, QueueError> {
        let client = redis::Client::open(url).map_err(be)?;
        let conn = ConnectionManager::new(client).await.map_err(be)?;
        Ok(Self {
            conn,
            ns: ns.to_string(),
        })
    }

    fn job_prefix(&self) -> String {
        format!("{}:job:", self.ns)
    }
    fn job_key(&self, id: &str) -> String {
        format!("{}:job:{}", self.ns, id)
    }
    fn ready_key(&self, q: &str) -> String {
        format!("{}:q:{}:ready", self.ns, q)
    }
    fn scheduled_key(&self, q: &str) -> String {
        format!("{}:q:{}:scheduled", self.ns, q)
    }
    fn inflight_key(&self, q: &str) -> String {
        format!("{}:q:{}:inflight", self.ns, q)
    }
    fn dead_key(&self, q: &str) -> String {
        format!("{}:q:{}:dead", self.ns, q)
    }
}

/// Map a redis error into a `QueueError`.
fn be<E: std::fmt::Display>(e: E) -> QueueError {
    QueueError::Backend(e.to_string())
}

#[async_trait]
impl QueueBackend for RedisBackend {
    async fn enqueue(&self, job: OutgoingJob) -> Result<JobId, QueueError> {
        let mut conn = self.conn.clone();
        let seq: u64 = conn.incr(format!("{}:seq", self.ns), 1).await.map_err(be)?;
        let id = format!("{seq}");
        let job_key = self.job_key(&id);

        // Store the job as a hash. Payload bytes go in verbatim (Redis strings
        // are binary-safe).
        let backoff_json = BackoffRepr::to_json(&job.backoff);
        let _: () = redis::pipe()
            .hset(&job_key, "name", &job.name)
            .hset(&job_key, "payload", job.payload.as_slice())
            .hset(&job_key, "attempts", 0u32)
            .hset(&job_key, "max_attempts", job.max_attempts.max(1))
            .hset(&job_key, "backoff", backoff_json)
            .hset(&job_key, "queue", &job.queue)
            .query_async(&mut conn)
            .await
            .map_err(be)?;

        match job.delay {
            Some(d) if !d.is_zero() => {
                let at = now_ms() + d.as_millis() as u64;
                let _: () = conn
                    .zadd(self.scheduled_key(&job.queue), &id, at)
                    .await
                    .map_err(be)?;
            }
            _ => {
                let _: () = conn
                    .lpush(self.ready_key(&job.queue), &id)
                    .await
                    .map_err(be)?;
            }
        }
        Ok(JobId(id))
    }

    async fn reserve(&self, opts: &ReserveOpts) -> Result<Option<ReservedJob>, QueueError> {
        let mut conn = self.conn.clone();
        let now = now_ms();
        let lease = now + opts.visibility.as_millis() as u64;
        let script = redis::Script::new(RESERVE_LUA);

        for q in &opts.queues {
            let picked: Option<(String, i64)> = script
                .key(self.ready_key(q))
                .key(self.scheduled_key(q))
                .key(self.inflight_key(q))
                .arg(now)
                .arg(lease)
                .arg(self.job_prefix())
                .invoke_async(&mut conn)
                .await
                .map_err(be)?;

            let Some((id, attempts)) = picked else {
                continue;
            };

            let job_key = self.job_key(&id);
            let (name, payload, max_attempts, backoff): (
                Option<String>,
                Option<Vec<u8>>,
                Option<u32>,
                Option<String>,
            ) = conn
                .hget(&job_key, &["name", "payload", "max_attempts", "backoff"])
                .await
                .map_err(be)?;

            // Hash vanished (acked concurrently) — treat as nothing reserved.
            let Some(name) = name else { continue };

            return Ok(Some(ReservedJob {
                id: JobId(id.clone()),
                queue: q.clone(),
                name,
                payload: payload.unwrap_or_default(),
                attempt: attempts.max(1) as u32,
                max_attempts: max_attempts.unwrap_or(1).max(1),
                backoff: backoff
                    .map(|s| BackoffRepr::from_json(&s))
                    .unwrap_or_default(),
                receipt: id,
            }));
        }
        Ok(None)
    }

    async fn ack(&self, job: &ReservedJob) -> Result<(), QueueError> {
        let mut conn = self.conn.clone();
        let _: () = redis::pipe()
            .zrem(self.inflight_key(&job.queue), &job.receipt)
            .del(self.job_key(&job.receipt))
            .query_async(&mut conn)
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn retry(&self, job: &ReservedJob, delay: Duration) -> Result<(), QueueError> {
        let mut conn = self.conn.clone();
        let at = now_ms() + delay.as_millis() as u64;
        // Remove the lease; re-schedule (attempts already bumped at reserve).
        let _: () = conn
            .zrem(self.inflight_key(&job.queue), &job.receipt)
            .await
            .map_err(be)?;
        if delay.is_zero() {
            let _: () = conn
                .lpush(self.ready_key(&job.queue), &job.receipt)
                .await
                .map_err(be)?;
        } else {
            let _: () = conn
                .zadd(self.scheduled_key(&job.queue), &job.receipt, at)
                .await
                .map_err(be)?;
        }
        Ok(())
    }

    async fn dead_letter(&self, job: &ReservedJob, reason: &str) -> Result<(), QueueError> {
        let mut conn = self.conn.clone();
        let _: () = redis::pipe()
            .zrem(self.inflight_key(&job.queue), &job.receipt)
            .hset(self.job_key(&job.receipt), "dead_reason", reason)
            .lpush(self.dead_key(&job.queue), &job.receipt)
            .query_async(&mut conn)
            .await
            .map_err(be)?;
        Ok(())
    }
}
