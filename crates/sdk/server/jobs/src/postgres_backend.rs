//! Postgres-backed [`QueueBackend`] using the classic `SELECT … FOR UPDATE SKIP
//! LOCKED` pattern — no extra infrastructure for apps already running Postgres.
//!
//! One table (default `jobs_queue`) holds every job. Visibility is a
//! `reserved_until` timestamp rather than a separate state: a row is reservable
//! when `available_at <= now()` and it isn't currently leased
//! (`reserved_until IS NULL OR reserved_until <= now()`), so an expired lease is
//! transparently reclaimed. `SKIP LOCKED` lets many workers pull concurrently
//! without blocking each other.

use crate::persist::BackoffRepr;
use crate::{JobId, OutgoingJob, QueueBackend, QueueError, ReserveOpts, ReservedJob};
use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::time::Duration;

/// A [`QueueBackend`] backed by a Postgres table. Clone shares the pool.
#[derive(Clone)]
pub struct PostgresBackend {
    pool: PgPool,
    table: String,
}

fn be<E: std::fmt::Display>(e: E) -> QueueError {
    QueueError::Backend(e.to_string())
}

impl PostgresBackend {
    /// Connect to `url` and ensure the default `jobs_queue` table exists.
    pub async fn connect(url: &str) -> Result<Self, QueueError> {
        Self::connect_table(url, "jobs_queue").await
    }

    /// Connect with a custom table name.
    pub async fn connect_table(url: &str, table: &str) -> Result<Self, QueueError> {
        // Guard the table name (it's interpolated, not bound) against injection.
        if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(QueueError::Backend(format!(
                "invalid table name {table:?}: only [A-Za-z0-9_] allowed"
            )));
        }
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await
            .map_err(be)?;
        let backend = Self {
            pool,
            table: table.to_string(),
        };
        backend.migrate().await?;
        Ok(backend)
    }

    /// Use an existing pool (e.g. the app's shared `PgPool`).
    pub async fn from_pool(pool: PgPool, table: &str) -> Result<Self, QueueError> {
        let backend = Self {
            pool,
            table: table.to_string(),
        };
        backend.migrate().await?;
        Ok(backend)
    }

    async fn migrate(&self) -> Result<(), QueueError> {
        let ddl = format!(
            "CREATE TABLE IF NOT EXISTS {t} (
                id             BIGSERIAL PRIMARY KEY,
                queue          TEXT        NOT NULL,
                name           TEXT        NOT NULL,
                payload        BYTEA       NOT NULL,
                attempts       INT         NOT NULL DEFAULT 0,
                max_attempts   INT         NOT NULL,
                backoff        TEXT        NOT NULL,
                available_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
                reserved_until TIMESTAMPTZ,
                state          TEXT        NOT NULL DEFAULT 'ready',
                dead_reason    TEXT
            );
            CREATE INDEX IF NOT EXISTS {t}_reservable
                ON {t} (queue, available_at)
                WHERE state = 'ready';",
            t = self.table
        );
        sqlx::raw_sql(&ddl).execute(&self.pool).await.map_err(be)?;
        Ok(())
    }
}

#[async_trait]
impl QueueBackend for PostgresBackend {
    async fn enqueue(&self, job: OutgoingJob) -> Result<JobId, QueueError> {
        let delay_ms = job.delay.map(|d| d.as_millis() as i64).unwrap_or(0);
        let sql = format!(
            "INSERT INTO {t} (queue, name, payload, attempts, max_attempts, backoff, available_at, state)
             VALUES ($1, $2, $3, 0, $4, $5, now() + ($6::bigint * interval '1 millisecond'), 'ready')
             RETURNING id",
            t = self.table
        );
        let id: i64 = sqlx::query(&sql)
            .bind(&job.queue)
            .bind(&job.name)
            .bind(&job.payload)
            .bind(job.max_attempts.max(1) as i32)
            .bind(BackoffRepr::to_json(&job.backoff))
            .bind(delay_ms)
            .fetch_one(&self.pool)
            .await
            .map_err(be)?
            .get("id");
        Ok(JobId(id.to_string()))
    }

    async fn reserve(&self, opts: &ReserveOpts) -> Result<Option<ReservedJob>, QueueError> {
        let visibility_ms = opts.visibility.as_millis() as i64;
        // Atomically claim the highest-priority ready row and stamp its lease.
        // `array_position($1, queue)` honors the queue priority order.
        let sql = format!(
            "UPDATE {t} SET
                 reserved_until = now() + ($2::bigint * interval '1 millisecond'),
                 attempts = attempts + 1
             WHERE id = (
                 SELECT id FROM {t}
                 WHERE state = 'ready'
                   AND queue = ANY($1)
                   AND available_at <= now()
                   AND (reserved_until IS NULL OR reserved_until <= now())
                 ORDER BY array_position($1, queue), available_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1
             )
             RETURNING id, queue, name, payload, attempts, max_attempts, backoff",
            t = self.table
        );
        let row = sqlx::query(&sql)
            .bind(&opts.queues)
            .bind(visibility_ms)
            .fetch_optional(&self.pool)
            .await
            .map_err(be)?;

        let Some(row) = row else { return Ok(None) };
        let id: i64 = row.get("id");
        let backoff: String = row.get("backoff");
        Ok(Some(ReservedJob {
            id: JobId(id.to_string()),
            queue: row.get("queue"),
            name: row.get("name"),
            payload: row.get("payload"),
            attempt: row.get::<i32, _>("attempts").max(1) as u32,
            max_attempts: row.get::<i32, _>("max_attempts").max(1) as u32,
            backoff: BackoffRepr::from_json(&backoff),
            receipt: id.to_string(),
        }))
    }

    async fn ack(&self, job: &ReservedJob) -> Result<(), QueueError> {
        let id: i64 = job.receipt.parse().unwrap_or(-1);
        let sql = format!("DELETE FROM {t} WHERE id = $1", t = self.table);
        sqlx::query(&sql)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn retry(&self, job: &ReservedJob, delay: Duration) -> Result<(), QueueError> {
        let id: i64 = job.receipt.parse().unwrap_or(-1);
        let delay_ms = delay.as_millis() as i64;
        let sql = format!(
            "UPDATE {t} SET
                 reserved_until = NULL,
                 available_at = now() + ($2::bigint * interval '1 millisecond')
             WHERE id = $1",
            t = self.table
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(delay_ms)
            .execute(&self.pool)
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn dead_letter(&self, job: &ReservedJob, reason: &str) -> Result<(), QueueError> {
        let id: i64 = job.receipt.parse().unwrap_or(-1);
        let sql = format!(
            "UPDATE {t} SET state = 'dead', reserved_until = NULL, dead_reason = $2 WHERE id = $1",
            t = self.table
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(reason)
            .execute(&self.pool)
            .await
            .map_err(be)?;
        Ok(())
    }
}
