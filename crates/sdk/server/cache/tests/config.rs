//! Tests for the process-wide configure surface (`cache::configure` /
//! `configured` / `configure_from_env`).
//!
//! The configured slot and the `IDEALYST_CACHE_*` env vars are process
//! globals, so every scenario runs sequentially inside ONE test fn — a
//! parallel test runner would otherwise interleave `set_var` calls.

use std::time::Duration;

use cache::{CacheExt, MemoryCache};

#[tokio::test]
async fn configure_surface_and_env_selection() {
    // -- nothing configured yet (this binary is the only config toucher) --
    assert!(cache::configured().is_none());

    // -- configure() installs; configured() hands the same cache back --
    cache::configure(MemoryCache::new());
    let handle = cache::configured().expect("configured after configure()");
    handle
        .set_json("k", &42_u32, Some(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(
        cache::configured().unwrap().get_json::<u32>("k").await.unwrap(),
        Some(42)
    );

    // -- default env (no vars) selects a FRESH memory cache (replaces) --
    std::env::remove_var("IDEALYST_CACHE_BACKEND");
    std::env::remove_var("IDEALYST_CACHE_URL");
    cache::configure_from_env().expect("default env → memory backend");
    let fresh = cache::configured().unwrap();
    assert_eq!(
        fresh.get_json::<u32>("k").await.unwrap(),
        None,
        "configure_from_env must install a new cache, not reuse the old one"
    );

    // -- unknown backend name is a clear error --
    std::env::set_var("IDEALYST_CACHE_BACKEND", "carrier-pigeon");
    let err = cache::configure_from_env().unwrap_err();
    assert!(
        err.to_string().contains("unknown IDEALYST_CACHE_BACKEND"),
        "got: {err}"
    );

    // -- redis without a URL falls back to the installed client; with
    //    neither, the error names both fixes (regression: the shared-client
    //    fallback must not silently configure nothing) --
    #[cfg(feature = "redis")]
    {
        std::env::set_var("IDEALYST_CACHE_BACKEND", "redis");
        std::env::remove_var("IDEALYST_CACHE_URL");
        let err = cache::configure_from_env().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("IDEALYST_CACHE_URL") && msg.contains("install_state"),
            "error should name both the URL and the installed-client fallback, got: {msg}"
        );

        // A URL (even unreachable — RedisCache connects lazily) configures.
        std::env::set_var("IDEALYST_CACHE_URL", "redis://127.0.0.1:1");
        cache::configure_from_env().expect("redis + URL configures lazily");
        assert!(cache::configured().is_some());
    }

    // -- redis selected but the feature isn't compiled in: clear error --
    #[cfg(not(feature = "redis"))]
    {
        std::env::set_var("IDEALYST_CACHE_BACKEND", "redis");
        let err = cache::configure_from_env().unwrap_err();
        assert!(err.to_string().contains("feature"), "got: {err}");
    }

    std::env::remove_var("IDEALYST_CACHE_BACKEND");
    std::env::remove_var("IDEALYST_CACHE_URL");
}
