//! Process-wide backend configuration + the publish / subscribe entry points.
//! Mirrors `jobs::config`: one backend installed at startup, read cheaply from
//! every publisher and subscriber.

use crate::{PubSubBackend, PubSubError};
use futures_util::{stream, Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::{Arc, OnceLock, RwLock};

static BACKEND: OnceLock<RwLock<Option<Arc<dyn PubSubBackend>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<dyn PubSubBackend>>> {
    BACKEND.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide pub/sub backend. Call once at startup. Calling again
/// replaces it.
pub fn configure<B: PubSubBackend + 'static>(backend: B) {
    *slot().write().unwrap() = Some(Arc::new(backend));
}

/// The configured backend, if [`configure`] has been called.
pub fn configured_backend() -> Option<Arc<dyn PubSubBackend>> {
    slot().read().unwrap().clone()
}

/// Broadcast `msg` to every current subscriber of `topic`.
pub async fn publish<T: Serialize>(topic: &str, msg: &T) -> Result<(), PubSubError> {
    let backend = configured_backend().ok_or(PubSubError::NotConfigured)?;
    let payload = serde_json::to_vec(msg).map_err(|e| PubSubError::Codec(e.to_string()))?;
    backend.publish(topic, &payload).await
}

/// Subscribe to `topic`, yielding each subsequently published `T`.
///
/// Infallible by design so it drops straight into a `#[subscription]` body
/// (`-> impl Stream<Item = T>`): the backend connection is established lazily on
/// first poll, a connect failure logs and yields an empty stream, and any
/// payload that doesn't decode to `T` is skipped rather than ending the stream.
pub fn subscribe<T>(topic: impl Into<String>) -> impl Stream<Item = T>
where
    T: DeserializeOwned + 'static,
{
    let topic = topic.into();
    // Lazily open the backend subscription on first poll.
    stream::once(async move {
        match configured_backend() {
            Some(backend) => match backend.subscribe(&topic).await {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("[pubsub] subscribe({topic}) failed: {e}");
                    None
                }
            },
            None => {
                eprintln!("[pubsub] subscribe({topic}): no backend configured");
                None
            }
        }
    })
    // Option<BoxStream> → 0-or-1 BoxStream, then flatten to the raw payloads.
    .filter_map(|opt| async move { opt })
    .flatten()
    // Decode; skip anything that isn't a valid `T`.
    .filter_map(|bytes| async move { serde_json::from_slice::<T>(&bytes).ok() })
}

/// Configure the backend from the environment: `IDEALYST_PUBSUB_BACKEND`
/// (`memory` | `redis` | `postgres`, default `memory`) + `IDEALYST_PUBSUB_URL`.
/// The bridge for `idealyst dev`, which sets those vars. Errors if the selected
/// backend's cargo feature isn't compiled in.
///
/// For the `redis` backend, a missing `IDEALYST_PUBSUB_URL` falls back to the
/// `redis::Client` already installed via `server::install_state` — one client
/// configured at boot serves cache, sessions, rate-limiting, and pubsub.
pub async fn configure_from_env() -> Result<(), PubSubError> {
    let backend = std::env::var("IDEALYST_PUBSUB_BACKEND").unwrap_or_else(|_| "memory".to_string());
    let url = std::env::var("IDEALYST_PUBSUB_URL").ok();
    match backend.as_str() {
        "memory" => configure_memory(),
        "redis" => configure_redis(url).await,
        "postgres" => configure_postgres(url).await,
        other => Err(PubSubError::Backend(format!(
            "unknown IDEALYST_PUBSUB_BACKEND `{other}` (expected memory|redis|postgres)"
        ))),
    }
}

#[allow(dead_code)] // used only by the disabled-backend arms
fn feature_off(name: &str) -> PubSubError {
    PubSubError::Backend(format!(
        "pubsub backend `{name}` selected via IDEALYST_PUBSUB_BACKEND, but its cargo feature \
         isn't enabled on the `pubsub` dependency"
    ))
}

#[allow(dead_code)] // used only by the enabled cross-instance-backend arms
fn require_url(url: Option<String>, backend: &str) -> Result<String, PubSubError> {
    url.ok_or_else(|| {
        PubSubError::Backend(format!("IDEALYST_PUBSUB_URL is required for the `{backend}` backend"))
    })
}

fn configure_memory() -> Result<(), PubSubError> {
    #[cfg(feature = "memory")]
    {
        configure(crate::MemoryBackend::new());
        Ok(())
    }
    #[cfg(not(feature = "memory"))]
    {
        Err(feature_off("memory"))
    }
}

async fn configure_redis(url: Option<String>) -> Result<(), PubSubError> {
    #[cfg(feature = "redis")]
    {
        // No URL → fall back to the app-installed `redis::Client`, so one
        // client configured at boot serves cache, sessions, AND pubsub.
        let backend = match url {
            Some(u) => crate::RedisBackend::connect(&u).await?,
            None => {
                let client = server::use_state::<redis::Client>().ok_or_else(|| {
                    PubSubError::Backend(
                        "IDEALYST_PUBSUB_URL is not set and no redis::Client is installed to \
                         fall back on; set the URL or call \
                         server::install_state(client.clone()) at boot"
                            .to_string(),
                    )
                })?;
                crate::RedisBackend::from_client(client).await?
            }
        };
        configure(backend);
        Ok(())
    }
    #[cfg(not(feature = "redis"))]
    {
        let _ = url;
        Err(feature_off("redis"))
    }
}

async fn configure_postgres(url: Option<String>) -> Result<(), PubSubError> {
    #[cfg(feature = "postgres")]
    {
        configure(crate::PostgresBackend::connect(&require_url(url, "postgres")?).await?);
        Ok(())
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = url;
        Err(feature_off("postgres"))
    }
}
