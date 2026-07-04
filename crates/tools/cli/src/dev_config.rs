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
    /// Load `<dir>/dev.toml`. Missing file → `Default`; parse error
    /// is surfaced so the user can fix typos rather than silently
    /// ignoring config they think is being applied.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("dev.toml");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let parsed: DevConfig = toml::from_str(&raw)?;
        Ok(parsed)
    }
}
