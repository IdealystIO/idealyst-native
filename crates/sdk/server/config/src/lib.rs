//! Unified configuration for the server-tier SDKs. See `Cargo.toml` for the
//! pitch.
//!
//! ```ignore
//! // One call at startup wires every configured SDK from the files on disk.
//! idealyst_config::configure_all().await?;
//! ```
//!
//! ```toml
//! # idealyst.toml — shared base
//! [connections.aws-main]
//! kind = "aws"
//! region = "us-east-1"
//! profile = "prod"
//!
//! [connections.cache]
//! kind = "redis"
//! url = "redis://127.0.0.1:6379"
//!
//! [jobs]
//! backend = "sqs"
//! connection = "aws-main"          # shares the AWS account with email…
//! queue_url = "https://sqs.us-east-1.amazonaws.com/123/jobs"
//!
//! [email]
//! provider = "ses"
//! connection = "aws-main"          # …same account
//! from = "Idealyst <no-reply@app.dev>"
//!
//! [pubsub]
//! backend = "redis"
//! connection = "cache"             # …shares the redis endpoint with…
//!
//! [cache]
//! backend = "redis"
//! connection = "cache"             # …the KV cache: one endpoint, two tools
//! ```

mod error;
mod loader;
mod schema;

pub use error::ConfigError;
pub use loader::{load, load_from};
pub use schema::{
    AwsConnection, CacheSection, Config, Connection, EmailSection, JobsSection, PubsubSection,
    UrlConnection,
};

/// Build a resolved AWS [`SdkConfig`](aws_config::SdkConfig) from a connection
/// (region / profile / endpoint override). Credentials still resolve via the
/// standard AWS provider chain; `profile` selects a named shared-config
/// profile. Shared by the SES (email) and SQS (jobs) wiring so two tools
/// pointing at the same `[connections.<name>]` get an identical account.
#[cfg(feature = "aws")]
pub async fn aws_sdk_config(conn: &AwsConnection) -> aws_config::SdkConfig {
    let mut builder = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if let Some(region) = &conn.region {
        builder = builder.region(aws_config::Region::new(region.clone()));
    }
    if let Some(profile) = &conn.profile {
        builder = builder.profile_name(profile.clone());
    }
    if let Some(endpoint) = &conn.endpoint_url {
        builder = builder.endpoint_url(endpoint.clone());
    }
    builder.load().await
}

/// Load the merged configuration from the current directory and configure
/// every enabled SDK it declares. The SDKs wired are exactly those whose cargo
/// feature is enabled on `idealyst-config`; sections for disabled SDKs are
/// ignored. A missing/empty config is fine — nothing is configured.
pub async fn configure_all() -> Result<(), ConfigError> {
    let config = load()?;
    configure_from(&config).await
}

/// Configure every enabled SDK from an already-loaded [`Config`] (e.g. one you
/// built or loaded with [`load_from`]).
pub async fn configure_from(config: &Config) -> Result<(), ConfigError> {
    #[cfg(feature = "jobs")]
    if let Some(section) = &config.jobs {
        wire::jobs(config, section).await?;
    }
    #[cfg(feature = "pubsub")]
    if let Some(section) = &config.pubsub {
        wire::pubsub(config, section).await?;
    }
    #[cfg(feature = "email")]
    if let Some(section) = &config.email {
        wire::email(config, section).await?;
    }
    #[cfg(feature = "cache")]
    if let Some(section) = &config.cache {
        wire::cache(config, section)?;
    }
    // Suppress "unused" when no SDK feature is enabled.
    let _ = config;
    Ok(())
}

/// Per-SDK wiring: resolve the section's connection to a concrete backend and
/// install it via the SDK's own `configure`. Each fn is gated on its SDK
/// feature; a selected backend whose broker feature is off returns a clear
/// [`ConfigError::FeatureOff`].
#[cfg(any(feature = "jobs", feature = "pubsub", feature = "email", feature = "cache"))]
mod wire {
    use crate::error::ConfigError;
    use crate::schema::Config;

    #[allow(dead_code)]
    fn feature_off(section: &str, backend: &str) -> ConfigError {
        ConfigError::FeatureOff(format!(
            "`[{section}]` selects the `{backend}` backend, but its feature isn't enabled on the \
             `idealyst-config` dependency (enable `idealyst-config/{backend}`)"
        ))
    }

    #[allow(dead_code)]
    fn resolve_err(section: &'static str) -> impl Fn(String) -> ConfigError {
        move |reason| ConfigError::Resolve { section: section.into(), reason }
    }

    #[allow(dead_code)]
    fn backend_err(section: &'static str) -> impl Fn(String) -> ConfigError {
        move |reason| ConfigError::Backend { section: section.into(), reason }
    }

    fn unknown(section: &str, backend: &str, expected: &str) -> ConfigError {
        ConfigError::Resolve {
            section: section.into(),
            reason: format!("unknown backend `{backend}` (expected {expected})"),
        }
    }

    #[cfg(feature = "jobs")]
    pub(super) async fn jobs(
        config: &Config,
        section: &crate::schema::JobsSection,
    ) -> Result<(), ConfigError> {
        match section.backend.as_deref().unwrap_or("memory") {
            "memory" => {
                jobs::configure(jobs::MemoryBackend::new());
                Ok(())
            }
            "redis" => {
                #[cfg(feature = "redis")]
                {
                    let url = config
                        .url_for(section.connection.as_deref(), section.url.as_deref())
                        .map_err(resolve_err("jobs"))?;
                    let backend = jobs::RedisBackend::connect(&url)
                        .await
                        .map_err(|e| backend_err("jobs")(e.to_string()))?;
                    jobs::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "redis"))]
                Err(feature_off("jobs", "redis"))
            }
            "postgres" => {
                #[cfg(feature = "postgres")]
                {
                    let url = config
                        .url_for(section.connection.as_deref(), section.url.as_deref())
                        .map_err(resolve_err("jobs"))?;
                    let backend = jobs::PostgresBackend::connect(&url)
                        .await
                        .map_err(|e| backend_err("jobs")(e.to_string()))?;
                    jobs::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "postgres"))]
                Err(feature_off("jobs", "postgres"))
            }
            "sqs" => {
                #[cfg(feature = "sqs")]
                {
                    let aws = config
                        .aws_for(section.connection.as_deref(), None, None)
                        .map_err(resolve_err("jobs"))?;
                    let queue_url = section.queue_url.clone().ok_or_else(|| ConfigError::Resolve {
                        section: "jobs".into(),
                        reason: "the `sqs` backend requires `queue_url`".into(),
                    })?;
                    let sdk = crate::aws_sdk_config(&aws).await;
                    let mut queues = std::collections::HashMap::new();
                    queues.insert("default".to_string(), queue_url);
                    let mut backend = jobs::SqsBackend::from_aws(&sdk, queues);
                    if let Some(dlq) = &section.dead_letter_url {
                        backend = backend.dead_letter_url(dlq.clone());
                    }
                    jobs::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "sqs"))]
                Err(feature_off("jobs", "sqs"))
            }
            other => Err(unknown("jobs", other, "memory|redis|postgres|sqs")),
        }
    }

    #[cfg(feature = "pubsub")]
    pub(super) async fn pubsub(
        config: &Config,
        section: &crate::schema::PubsubSection,
    ) -> Result<(), ConfigError> {
        match section.backend.as_deref().unwrap_or("memory") {
            "memory" => {
                pubsub::configure(pubsub::MemoryBackend::new());
                Ok(())
            }
            "redis" => {
                #[cfg(feature = "redis")]
                {
                    let url = config
                        .url_for(section.connection.as_deref(), section.url.as_deref())
                        .map_err(resolve_err("pubsub"))?;
                    let backend = pubsub::RedisBackend::connect(&url)
                        .await
                        .map_err(|e| backend_err("pubsub")(e.to_string()))?;
                    pubsub::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "redis"))]
                Err(feature_off("pubsub", "redis"))
            }
            "postgres" => {
                #[cfg(feature = "postgres")]
                {
                    let url = config
                        .url_for(section.connection.as_deref(), section.url.as_deref())
                        .map_err(resolve_err("pubsub"))?;
                    let backend = pubsub::PostgresBackend::connect(&url)
                        .await
                        .map_err(|e| backend_err("pubsub")(e.to_string()))?;
                    pubsub::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "postgres"))]
                Err(feature_off("pubsub", "postgres"))
            }
            other => Err(unknown("pubsub", other, "memory|redis|postgres")),
        }
    }

    #[cfg(feature = "cache")]
    pub(super) fn cache(
        config: &Config,
        section: &crate::schema::CacheSection,
    ) -> Result<(), ConfigError> {
        match section.backend.as_deref().unwrap_or("memory") {
            "memory" => {
                cache::configure(cache::MemoryCache::new());
                Ok(())
            }
            "redis" => {
                #[cfg(feature = "redis")]
                {
                    let url = config
                        .url_for(section.connection.as_deref(), section.url.as_deref())
                        .map_err(resolve_err("cache"))?;
                    // Connects lazily on first use, so wiring is sync.
                    let mut backend = cache::RedisCache::from_url(&url)
                        .map_err(|e| backend_err("cache")(e.to_string()))?;
                    if let Some(ns) = &section.namespace {
                        backend = backend.namespace(ns.clone());
                    }
                    cache::configure(backend);
                    Ok(())
                }
                #[cfg(not(feature = "redis"))]
                Err(feature_off("cache", "redis"))
            }
            other => Err(unknown("cache", other, "memory|redis")),
        }
    }

    #[cfg(feature = "email")]
    pub(super) async fn email(
        config: &Config,
        section: &crate::schema::EmailSection,
    ) -> Result<(), ConfigError> {
        match section.provider.as_deref().unwrap_or("memory") {
            "memory" => {
                let mut provider = email::MemoryProvider::new();
                if let Some(from) = &section.from {
                    provider = provider.with_default_from(from.clone());
                }
                email::configure(provider);
                Ok(())
            }
            "ses" => {
                #[cfg(feature = "ses")]
                {
                    let aws = config
                        .aws_for(
                            section.connection.as_deref(),
                            section.region.as_deref(),
                            section.profile.as_deref(),
                        )
                        .map_err(resolve_err("email"))?;
                    let sdk = crate::aws_sdk_config(&aws).await;
                    let mut provider = email::SesProvider::from_aws(&sdk);
                    if let Some(cs) = &section.configuration_set {
                        provider = provider.with_configuration_set(cs.clone());
                    }
                    if let Some(from) = &section.from {
                        provider = provider.with_default_from(from.clone());
                    }
                    email::configure(provider);
                    Ok(())
                }
                #[cfg(not(feature = "ses"))]
                Err(feature_off("email", "ses"))
            }
            other => Err(unknown("email", other, "memory|ses")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Write `files` (name → contents) into a fresh temp dir and return it.
    fn scratch(id: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idealyst-config-test-{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            let mut f = std::fs::File::create(dir.join(name)).unwrap();
            f.write_all(body.as_bytes()).unwrap();
        }
        dir
    }

    #[test]
    fn base_file_parses_connections_and_sections() {
        let dir = scratch(
            "base",
            &[(
                "idealyst.toml",
                r#"
                [connections.aws-main]
                kind = "aws"
                region = "us-east-1"
                profile = "prod"

                [email]
                provider = "ses"
                connection = "aws-main"
                from = "no-reply@app.dev"
                "#,
            )],
        );
        let cfg = load_from(&dir).unwrap();
        let email = cfg.email.as_ref().unwrap();
        assert_eq!(email.provider.as_deref(), Some("ses"));
        assert_eq!(email.connection.as_deref(), Some("aws-main"));
        let aws = cfg
            .aws_for(email.connection.as_deref(), None, None)
            .expect("aws connection resolves");
        assert_eq!(aws.region.as_deref(), Some("us-east-1"));
        assert_eq!(aws.profile.as_deref(), Some("prod"));
    }

    /// The motivating case: jobs (SQS) and email (SES) reference the SAME AWS
    /// connection → they share one account; pubsub references a different
    /// connection → separate. This is what a flat env namespace can't express.
    #[test]
    fn tools_share_or_separate_connections_by_name() {
        let dir = scratch(
            "share",
            &[(
                "idealyst.toml",
                r#"
                [connections.aws-main]
                kind = "aws"
                region = "us-east-1"

                [connections.aws-eu]
                kind = "aws"
                region = "eu-west-1"

                [jobs]
                backend = "sqs"
                connection = "aws-main"
                queue_url = "https://sqs/jobs"

                [email]
                provider = "ses"
                connection = "aws-main"

                [pubsub]
                backend = "redis"
                connection = "cache"

                [connections.cache]
                kind = "redis"
                url = "redis://localhost:6379"
                "#,
            )],
        );
        let cfg = load_from(&dir).unwrap();
        let jobs_aws = cfg.aws_for(cfg.jobs.as_ref().unwrap().connection.as_deref(), None, None).unwrap();
        let email_aws = cfg.aws_for(cfg.email.as_ref().unwrap().connection.as_deref(), None, None).unwrap();
        // Shared: identical region resolved from the one connection.
        assert_eq!(jobs_aws.region, email_aws.region);
        // Separate: pubsub is a redis URL connection, not the AWS one.
        let ps_url = cfg
            .url_for(cfg.pubsub.as_ref().unwrap().connection.as_deref(), None)
            .unwrap();
        assert_eq!(ps_url, "redis://localhost:6379");
    }

    /// A per-tool file (`email.toml`) overlays the base and can reference a
    /// connection defined in the base (auto-merge, no `extends` needed).
    #[test]
    fn per_tool_file_overlays_and_uses_base_connection() {
        let dir = scratch(
            "pertool",
            &[
                (
                    "idealyst.toml",
                    r#"
                    [connections.aws-main]
                    kind = "aws"
                    region = "us-east-1"

                    [email]
                    provider = "memory"
                    from = "base@app.dev"
                    "#,
                ),
                (
                    "email.toml",
                    r#"
                    [email]
                    provider = "ses"
                    connection = "aws-main"
                    "#,
                ),
            ],
        );
        let cfg = load_from(&dir).unwrap();
        let email = cfg.email.as_ref().unwrap();
        // email.toml overrode provider…
        assert_eq!(email.provider.as_deref(), Some("ses"));
        // …but the base's `from` survived (field-level overlay, not wholesale).
        assert_eq!(email.from.as_deref(), Some("base@app.dev"));
        // …and the connection defined in the base resolves from the tool file.
        assert!(cfg.aws_for(email.connection.as_deref(), None, None).is_ok());
    }

    /// `extends` inherits a parent file's connections, then overlays.
    #[test]
    fn extends_inherits_parent_connections() {
        let dir = scratch(
            "extends",
            &[
                (
                    "shared.toml",
                    r#"
                    [connections.aws-main]
                    kind = "aws"
                    region = "us-west-2"
                    "#,
                ),
                (
                    "idealyst.toml",
                    r#"
                    extends = "shared.toml"

                    [email]
                    provider = "ses"
                    connection = "aws-main"
                    "#,
                ),
            ],
        );
        let cfg = load_from(&dir).unwrap();
        let aws = cfg
            .aws_for(cfg.email.as_ref().unwrap().connection.as_deref(), None, None)
            .expect("inherited connection resolves");
        assert_eq!(aws.region.as_deref(), Some("us-west-2"));
    }

    /// A `[cache]` section parses, resolves its named redis connection, and
    /// carries the namespace through — the same shape as `[pubsub]`, so cache
    /// and pubsub can share one endpoint by name.
    #[test]
    fn cache_section_parses_and_resolves_connection() {
        let dir = scratch(
            "cache",
            &[(
                "idealyst.toml",
                r#"
                [connections.main]
                kind = "redis"
                url = "redis://localhost:6379"

                [cache]
                backend = "redis"
                connection = "main"
                namespace = "myapp"

                [pubsub]
                backend = "redis"
                connection = "main"
                "#,
            )],
        );
        let cfg = load_from(&dir).unwrap();
        let cache = cfg.cache.as_ref().unwrap();
        assert_eq!(cache.backend.as_deref(), Some("redis"));
        assert_eq!(cache.namespace.as_deref(), Some("myapp"));
        // Cache and pubsub resolve the SAME endpoint through the one profile.
        let cache_url = cfg.url_for(cache.connection.as_deref(), cache.url.as_deref()).unwrap();
        let ps = cfg.pubsub.as_ref().unwrap();
        let ps_url = cfg.url_for(ps.connection.as_deref(), ps.url.as_deref()).unwrap();
        assert_eq!(cache_url, ps_url);
    }

    /// End-to-end: `configure_from` wires the memory backends of every enabled
    /// SDK. Gated on the SDK features so the default test run stays dep-light;
    /// run with `--features "email jobs pubsub cache"`.
    #[cfg(all(feature = "email", feature = "jobs", feature = "pubsub", feature = "cache"))]
    #[tokio::test]
    async fn configure_from_wires_memory_backends() {
        let dir = scratch(
            "wire",
            &[(
                "idealyst.toml",
                r#"
                [jobs]
                backend = "memory"

                [pubsub]
                backend = "memory"

                [email]
                provider = "memory"
                from = "no-reply@app.dev"

                [cache]
                backend = "memory"
                "#,
            )],
        );
        let cfg = load_from(&dir).unwrap();
        configure_from(&cfg).await.expect("configure_from wires memory backends");

        // Each SDK now reports a configured backend/provider.
        assert!(jobs::configured_backend().is_some(), "jobs configured");
        assert!(pubsub::configured_backend().is_some(), "pubsub configured");
        assert!(email::configured_provider().is_some(), "email configured");
        assert!(cache::configured().is_some(), "cache configured");
        // The email default sender flowed through.
        let provider = email::configured_provider().unwrap();
        assert_eq!(
            provider.default_from().map(|m| m.address),
            Some("no-reply@app.dev".to_string())
        );
    }

    /// Referencing a missing / wrong-kind connection is a clear error.
    #[test]
    fn connection_resolution_errors_are_clear() {
        let cfg = Config::default();
        assert!(cfg.aws_for(Some("nope"), None, None).is_err());
        assert!(cfg.url_for(Some("nope"), None).is_err());
        // No reference + no inline → error for URL connections.
        assert!(cfg.url_for(None, None).is_err());
        // No reference + inline region → an ad-hoc AWS connection.
        assert_eq!(
            cfg.aws_for(None, Some("ap-south-1"), None).unwrap().region.as_deref(),
            Some("ap-south-1")
        );
    }
}
