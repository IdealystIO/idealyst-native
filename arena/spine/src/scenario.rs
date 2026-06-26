//! The **public** half of a benchmark task. A scenario's `prompt` is the
//! only task framing the isolated implementation agent ever sees — the
//! rubric that scores it ([`crate::rubric`]) stays secret and physically
//! separate so it can never leak into the agent's context.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Web,
    Android,
    Ios,
    Macos,
    Terminal,
}

fn default_platforms() -> Vec<Platform> {
    // Web is the high-priority target: if the framework is behaving, a
    // working web build is strong evidence every backend works, and it's
    // the cheapest to drive automatically (Playwright).
    vec![Platform::Web]
}

fn default_runs() -> u32 {
    5
}

fn default_budget() -> u64 {
    2_000_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    /// Stable id; must match the rubric's `scenario_id`.
    pub id: String,
    /// The task handed to the implementation agent, verbatim. Requirements
    /// only — never hints about *how* to use the MCP (that would mask doc
    /// deficiencies the arena exists to find).
    pub prompt: String,
    /// Which platforms this scenario verifies its outcome items on.
    #[serde(default = "default_platforms")]
    pub platforms: Vec<Platform>,
    /// Hard ceiling on agent tokens; the harness aborts a runaway run here.
    #[serde(default = "default_budget")]
    pub token_budget: u64,
    /// Statistical samples to run for this scenario.
    #[serde(default = "default_runs")]
    pub runs: u32,
}

impl Scenario {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading scenario {}: {e}", path.display()))?;
        Ok(toml::from_str(&raw)?)
    }
}
