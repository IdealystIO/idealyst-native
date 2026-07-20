//! VS Code configuration — the shared, non-interactive core.
//!
//! Mirrors [`crate::devcontainer`]: both front-ends (the `idealyst configure
//! vscode` CLI wizard and the MCP `configure_vscode` tool) build a
//! [`ConfigureRequest`] of per-aspect actions and call [`apply`];
//! [`read_state`] reports which aspects are currently configured.
//!
//! Aspects (recommend the editor extensions, wire idealyst lint into
//! rust-analyzer, …) are declarative [`aspect::VscodeAspect`]s applied
//! surgically to the user's `.vscode/` — we set only our keys, union only our
//! array entries + recommendations, and generate our own files
//! (`ra-check.sh`), never taking ownership of the whole `settings.json`.

pub mod aspect;
pub mod aspects;
pub mod files;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use aspect::{registry, VscodeAspect};

/// What to do with an aspect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Apply the aspect (no-op + warning if already fully configured).
    Enable,
    /// Remove the aspect's settings / recommendations / files.
    Remove,
}

/// A single aspect instruction.
#[derive(Clone, Debug)]
pub struct AspectRequest {
    pub id: String,
    pub action: Action,
}

/// A batch of aspect instructions.
#[derive(Clone, Debug, Default)]
pub struct ConfigureRequest {
    pub aspects: Vec<AspectRequest>,
}

/// Snapshot of the current VS Code configuration.
#[derive(Clone, Debug, Default)]
pub struct VscodeState {
    /// Whether the project has any `.vscode/` JSON yet.
    pub exists: bool,
    /// Ids of aspects that are currently fully configured (for preselect).
    pub enabled: Vec<String>,
}

/// What `apply` changed.
#[derive(Clone, Debug, Default)]
pub struct ConfigureReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub warnings: Vec<String>,
    pub wrote: Vec<PathBuf>,
}

/// Convenience: request enabling every registered aspect (the default the CLI
/// and MCP use when the caller names none).
pub fn enable_all_request() -> ConfigureRequest {
    ConfigureRequest {
        aspects: registry()
            .iter()
            .map(|a| AspectRequest { id: a.id().to_string(), action: Action::Enable })
            .collect(),
    }
}

/// Read the current VS Code configuration for the project at `dir`.
pub fn read_state(dir: &Path) -> Result<VscodeState> {
    let ws = files::Workspace::load(dir)?;
    let enabled = registry()
        .iter()
        .filter(|a| ws.is_present(&a.contribution(dir)))
        .map(|a| a.id().to_string())
        .collect();
    Ok(VscodeState {
        exists: files::settings_path(dir).exists() || files::extensions_path(dir).exists(),
        enabled,
    })
}

/// Apply a batch of aspect instructions to a project's `.vscode/`.
pub fn apply(dir: &Path, req: &ConfigureRequest) -> Result<ConfigureReport> {
    let mut report = ConfigureReport::default();
    let mut ws = files::Workspace::load(dir)?;

    for r in &req.aspects {
        let aspect = aspect::find(&r.id).with_context(|| {
            format!(
                "unknown vscode aspect {:?}; known aspects: {}",
                r.id,
                registry().iter().map(|a| a.id()).collect::<Vec<_>>().join(", ")
            )
        })?;
        let contrib = aspect.contribution(dir);
        let present = ws.is_present(&contrib);
        match r.action {
            Action::Enable => {
                if present {
                    report.unchanged.push(r.id.clone());
                    report
                        .warnings
                        .push(format!("{} already configured — no-op", r.id));
                } else {
                    ws.apply(&contrib)?;
                    report.added.push(r.id.clone());
                }
            }
            Action::Remove => {
                if present {
                    ws.remove(&contrib)?;
                    report.removed.push(r.id.clone());
                } else {
                    report
                        .warnings
                        .push(format!("{} is not configured — nothing to remove", r.id));
                }
            }
        }
    }

    report.wrote = ws.finish()?;
    Ok(report)
}
