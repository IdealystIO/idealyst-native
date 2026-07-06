//! Per-project `dev.toml` — local dev-mode configuration that
//! `idealyst dev` reads at startup.
//!
//! Lives at `<project>/dev.toml`. Optional — absence is fine, every
//! field has a default. Today's surface is small (just bridge_port);
//! the file exists so future dev knobs have a home that doesn't
//! pollute `Cargo.toml`'s metadata table.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct DevConfig {
    /// Optional fixed port for the Robot bridge. When set, the
    /// running app tries to bind exactly this port (and falls back
    /// to ephemeral with a warning if it's already taken). When
    /// unset, the bridge always picks ephemeral. Pin this only if
    /// an external tool needs a stable target — for normal Claude
    /// workflows the ephemeral + `.idealyst/bridge.port` discovery
    /// path is preferable.
    #[serde(default)]
    pub bridge_port: Option<u16>,

    /// Local job-queue backend for `idealyst dev` / `idealyst worker`. Absent
    /// means the in-process `memory` backend: `dev` runs workers inside the
    /// server process (no separate worker process is spawned, since two
    /// processes can't share an in-memory queue). Point this at a real broker
    /// (redis / postgres / sqs) to have `dev` auto-spawn a dedicated worker
    /// process that shares the queue with the server.
    ///
    /// ```toml
    /// [jobs]
    /// backend = "redis"
    /// url = "redis://127.0.0.1:6379"
    /// ```
    #[serde(default)]
    pub jobs: Option<JobsConfig>,

    /// Local publish/subscribe backend for `idealyst dev`. Absent → the
    /// in-process `memory` backend (single instance). Point at redis/postgres
    /// for cross-instance fan-out (and to make multi-instance dev meaningful).
    ///
    /// ```toml
    /// [pubsub]
    /// backend = "redis"
    /// url = "redis://127.0.0.1:6379"
    /// ```
    #[serde(default)]
    pub pubsub: Option<PubsubConfig>,
}

/// The `[pubsub]` block in `dev.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct PubsubConfig {
    /// `memory` (default) | `redis` | `postgres`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Connection string for the broker (redis URL / Postgres URL). Unused for
    /// `memory`.
    #[serde(default)]
    pub url: Option<String>,
}

/// The `[jobs]` block in `dev.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct JobsConfig {
    /// `memory` (default) | `redis` | `postgres` | `sqs`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Connection string for the broker (redis URL, Postgres URL, SQS queue
    /// URL). Unused for `memory`.
    #[serde(default)]
    pub url: Option<String>,
}

impl JobsConfig {
    /// Whether this backend is shared across processes (so a dedicated worker
    /// process can drain the same queue the server enqueues to). `memory` is
    /// per-process and therefore not shared.
    pub fn is_shared(&self) -> bool {
        matches!(
            self.backend.as_deref(),
            Some("redis") | Some("postgres") | Some("sqs")
        )
    }
}

impl DevConfig {
    /// Load `<dir>/dev.toml`, then overlay the unified `idealyst.toml`
    /// (+ per-tool files) via the shared config loader.
    ///
    /// `dev.toml` keeps the dev-only knobs (`bridge_port`) and, for backward
    /// compatibility, still supplies `[jobs]`/`[pubsub]`. The newer
    /// `idealyst.toml` is the primary surface: when it declares `[jobs]` /
    /// `[pubsub]`, those win (with any `connection` reference resolved to a URL
    /// here so the worker-spawn decision + env bridge keep working). A migrated
    /// app reads `idealyst.toml` itself via `idealyst_config::configure_all()`;
    /// the CLI reads it only to decide whether to spawn a dedicated worker.
    ///
    /// Missing files → `Default`; parse errors are surfaced so typos don't
    /// silently no-op.
    pub fn load(dir: &Path) -> Result<Self> {
        // 1. Legacy dev.toml (dev-only knobs + optional jobs/pubsub).
        let mut cfg = {
            let path = dir.join("dev.toml");
            if path.is_file() {
                let raw = std::fs::read_to_string(&path)?;
                toml::from_str::<DevConfig>(&raw)?
            } else {
                DevConfig::default()
            }
        };

        // 2. Unified idealyst.toml (+ per-tool files) — overlays jobs/pubsub.
        let unified = idealyst_config::load_from(dir)?;
        if let Some(jobs) = &unified.jobs {
            // Resolve a `connection` ref (redis/postgres URL) or fall back to an
            // inline `url` / the SQS `queue_url`, so the env bridge has a URL.
            let url = unified
                .url_for(jobs.connection.as_deref(), jobs.url.as_deref())
                .ok()
                .or_else(|| jobs.queue_url.clone());
            cfg.jobs = Some(JobsConfig { backend: jobs.backend.clone(), url });
        }
        if let Some(pubsub) = &unified.pubsub {
            let url = unified
                .url_for(pubsub.connection.as_deref(), pubsub.url.as_deref())
                .ok();
            cfg.pubsub = Some(PubsubConfig { backend: pubsub.backend.clone(), url });
        }

        Ok(cfg)
    }
}
