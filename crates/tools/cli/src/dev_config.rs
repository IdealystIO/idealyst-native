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
