//! Postgres-backed [`PubSubBackend`] over `LISTEN`/`NOTIFY` — cross-instance
//! pub/sub for apps already running Postgres, no extra broker.
//!
//! Postgres `NOTIFY` channel names are SQL identifiers (≤63 bytes, case-folded),
//! which would mangle arbitrary topic strings and explode connections, so all
//! topics ride **one** channel (`idealyst_pubsub`) and the topic is prefixed
//! into the notification payload (`topic \x1f payload`); each subscriber filters
//! for its topic. Note Postgres caps a NOTIFY payload at ~8000 bytes — publish a
//! reference (id/URL) for larger messages.

use crate::{PubSubBackend, PubSubError};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use sqlx::postgres::{PgListener, PgPoolOptions};
use sqlx::PgPool;

/// The single Postgres channel every topic multiplexes over.
const CHANNEL: &str = "idealyst_pubsub";
/// Separator between the topic and the payload in a notification. ASCII Unit
/// Separator — won't appear in a JSON payload.
const SEP: char = '\u{1f}';

/// A [`PubSubBackend`] over Postgres `LISTEN`/`NOTIFY`. Clone shares the pool.
#[derive(Clone)]
pub struct PostgresBackend {
    pool: PgPool,
}

fn be<E: std::fmt::Display>(e: E) -> PubSubError {
    PubSubError::Backend(e.to_string())
}

impl PostgresBackend {
    /// Connect to `url`.
    pub async fn connect(url: &str) -> Result<Self, PubSubError> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(url)
            .await
            .map_err(be)?;
        Ok(Self { pool })
    }

    /// Use an existing pool (e.g. the app's shared `PgPool`).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PubSubBackend for PostgresBackend {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), PubSubError> {
        // NOTIFY payloads are text; our payloads are JSON (valid UTF-8).
        let text = std::str::from_utf8(payload)
            .map_err(|e| PubSubError::Codec(format!("payload not UTF-8: {e}")))?;
        let framed = format!("{topic}{SEP}{text}");
        // pg_notify binds the payload safely (no manual escaping).
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(CHANNEL)
            .bind(framed)
            .execute(&self.pool)
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn subscribe(
        &self,
        topic: &str,
    ) -> Result<BoxStream<'static, Vec<u8>>, PubSubError> {
        let mut listener = PgListener::connect_with(&self.pool).await.map_err(be)?;
        listener.listen(CHANNEL).await.map_err(be)?;
        let want = topic.to_string();
        let stream = listener.into_stream().filter_map(move |res| {
            let want = want.clone();
            async move {
                let notif = res.ok()?;
                let raw = notif.payload();
                // Split "topic\x1fpayload"; keep only our topic's messages.
                let (t, body) = raw.split_once(SEP)?;
                if t == want {
                    Some(body.as_bytes().to_vec())
                } else {
                    None
                }
            }
        });
        Ok(Box::pin(stream))
    }
}
