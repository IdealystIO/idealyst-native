//! Tests for `pubsub::configure_from_env`'s redis arm — specifically the
//! shared-client fallback added alongside `RedisBackend::from_client` /
//! `from_installed`.
//!
//! Env vars are process globals; this file keeps every env-touching scenario
//! in ONE test fn so the parallel runner can't interleave `set_var` calls.

#![cfg(feature = "redis")]

/// Regression: `IDEALYST_PUBSUB_BACKEND=redis` with no URL used to be a hard
/// "URL is required" error. It now falls back to the app-installed
/// `redis::Client`; when neither is present the error must name both fixes
/// so a misconfigured boot is diagnosable.
#[tokio::test]
async fn regression_env_redis_without_url_names_installed_client_fallback() {
    std::env::set_var("IDEALYST_PUBSUB_BACKEND", "redis");
    std::env::remove_var("IDEALYST_PUBSUB_URL");

    let err = pubsub::configure_from_env()
        .await
        .expect_err("no URL and no installed client must not silently configure");
    let msg = err.to_string();
    assert!(
        msg.contains("IDEALYST_PUBSUB_URL") && msg.contains("install_state"),
        "error should name both the URL and the installed-client fallback, got: {msg}"
    );

    std::env::remove_var("IDEALYST_PUBSUB_BACKEND");
}
