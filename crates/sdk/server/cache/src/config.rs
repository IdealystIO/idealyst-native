//! Process-wide cache configuration — the same boot-time shape as
//! `jobs::configure` / `pubsub::configure`: one cache installed at startup,
//! read cheaply from anywhere server-side via [`configured`].
//!
//! This is a *convenience* over the crate's primary consumption model
//! (app-provided context via `server::install_state`, see the crate docs) —
//! the two compose: `configure_from_env()` at boot, then
//! `server::install_state(cache::configured().unwrap())` if you want the
//! cache as injected `#[ctx]` state. The env spelling exists so `idealyst
//! dev` (and any container platform) can select the backend without a code
//! change, exactly like `IDEALYST_JOBS_*` / `IDEALYST_PUBSUB_*`.

use crate::{Cache, CacheError};
use std::sync::{Arc, OnceLock, RwLock};

static CACHE: OnceLock<RwLock<Option<Arc<dyn Cache>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<dyn Cache>>> {
    CACHE.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide cache. Call once at startup. Calling again
/// replaces it.
pub fn configure<C: Cache>(cache: C) {
    *slot().write().unwrap() = Some(Arc::new(cache));
}

/// The configured cache, if [`configure`] has been called.
pub fn configured() -> Option<Arc<dyn Cache>> {
    slot().read().unwrap().clone()
}

/// Configure the cache from the environment: `IDEALYST_CACHE_BACKEND`
/// (`memory` | `redis`, default `memory`) + `IDEALYST_CACHE_URL`. The bridge
/// for `idealyst dev`, which sets those vars; mirrors
/// `pubsub::configure_from_env`. Errors if the selected backend's cargo
/// feature isn't compiled in.
///
/// For the `redis` backend, a missing `IDEALYST_CACHE_URL` falls back to the
/// `redis::Client` already installed via `server::install_state` — the same
/// shared-connection spelling as [`crate::RedisCache::from_installed`], so one
/// client serves cache, sessions, rate-limiting, and pubsub.
pub fn configure_from_env() -> Result<(), CacheError> {
    let backend = std::env::var("IDEALYST_CACHE_BACKEND").unwrap_or_else(|_| "memory".to_string());
    let url = std::env::var("IDEALYST_CACHE_URL").ok();
    match backend.as_str() {
        "memory" => {
            configure(crate::MemoryCache::new());
            Ok(())
        }
        "redis" => configure_redis(url),
        other => Err(CacheError(format!(
            "unknown IDEALYST_CACHE_BACKEND `{other}` (expected memory|redis)"
        ))),
    }
}

#[cfg(feature = "redis")]
fn configure_redis(url: Option<String>) -> Result<(), CacheError> {
    let cache = match url {
        Some(u) => crate::RedisCache::from_url(&u)?,
        None => {
            let client = server::use_state::<redis::Client>().ok_or_else(|| {
                CacheError(
                    "IDEALYST_CACHE_URL is not set and no redis::Client is installed to fall \
                     back on; set the URL or call server::install_state(client.clone()) at boot"
                        .to_string(),
                )
            })?;
            crate::RedisCache::new(client)
        }
    };
    configure(cache);
    Ok(())
}

#[cfg(not(feature = "redis"))]
fn configure_redis(_url: Option<String>) -> Result<(), CacheError> {
    Err(CacheError(
        "cache backend `redis` selected via IDEALYST_CACHE_BACKEND, but the `redis` cargo \
         feature isn't enabled on the `cache` dependency"
            .to_string(),
    ))
}
