//! Discover the running app's Robot bridge by walking the cwd up
//! to find `.idealyst/bridge.port`.
//!
//! Multi-project isolation: when `idealyst dev` runs an app it writes
//! the bound bridge port (and the project root) into
//! `<project>/.idealyst/bridge.port`. Each project gets its own port,
//! its own file, its own directory. The MCP server — launched by
//! Claude Code from the project directory — discovers the file by
//! walking up from cwd, verifies the `project_root` field matches its
//! own cwd, and uses the recorded port.
//!
//! **Safeguard**: if the port file's `project_root` doesn't match the
//! cwd-derived root (e.g. user pointed `.mcp.json` at a binary in a
//! different project), discovery refuses to use the bridge. That
//! stops a Claude session from accidentally driving a different
//! project's running app.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Where the bridge writes its connection info, relative to project root.
pub const PORT_FILE_RELATIVE: &str = ".idealyst/bridge.port";

/// Where the dev launcher writes the path to a binary supporting
/// `--emit-catalog`. The MCP server reads this and auto-applies it
/// as `--from-bin` so the catalog tools populate without the user
/// passing the flag.
pub const CATALOG_PATH_RELATIVE: &str = ".idealyst/catalog.path";

#[derive(Debug, Deserialize)]
struct PortFile {
    port: u16,
    project_root: String,
    #[serde(default)]
    #[allow(dead_code)]
    pid: u32,
}

/// Outcome of `discover`. The MCP server uses `Found` directly,
/// reports `Mismatch` as a warning + refusal, and treats
/// `NotFound` as "fall back to the default bridge address".
#[derive(Debug)]
pub enum Discovered {
    /// A valid port file was found and its `project_root` matches
    /// the cwd-derived root. Use this address.
    Found { addr: String, project_root: PathBuf },
    /// A port file was found but it points at a different project.
    /// The MCP server should NOT connect — that would route a
    /// Claude session in project A through to project B's app.
    Mismatch {
        file: PathBuf,
        file_project_root: String,
        actual_project_root: PathBuf,
    },
    /// No `.idealyst/bridge.port` anywhere on the way up to root.
    /// Usually means the app isn't running (or wasn't started via
    /// `idealyst dev`). The MCP server falls back to the default
    /// address.
    NotFound,
}

/// Walk `start` upward looking for `.idealyst/bridge.port`. When
/// found, parse it and validate that its `project_root` matches the
/// directory containing the `.idealyst/` folder (i.e. the file
/// hasn't been moved or doesn't refer to a different project).
pub fn discover(start: &Path) -> Result<Discovered> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join(PORT_FILE_RELATIVE);
        if candidate.is_file() {
            let raw = std::fs::read_to_string(&candidate)
                .with_context(|| format!("read {}", candidate.display()))?;
            let parsed: PortFile = serde_json::from_str(&raw).with_context(|| {
                format!("parse {} as bridge port JSON", candidate.display())
            })?;
            let expected_root = dir.to_path_buf();
            // Canonicalize both sides so /tmp vs /private/tmp etc.
            // don't cause false mismatches on macOS.
            let file_root_canon = std::fs::canonicalize(&parsed.project_root)
                .unwrap_or_else(|_| PathBuf::from(&parsed.project_root));
            let actual_canon = std::fs::canonicalize(&expected_root)
                .unwrap_or_else(|_| expected_root.clone());
            if file_root_canon != actual_canon {
                return Ok(Discovered::Mismatch {
                    file: candidate,
                    file_project_root: parsed.project_root,
                    actual_project_root: expected_root,
                });
            }
            return Ok(Discovered::Found {
                addr: format!("127.0.0.1:{}", parsed.port),
                project_root: expected_root,
            });
        }
        cur = dir.parent();
    }
    Ok(Discovered::NotFound)
}

/// Walk `start` upward looking for `.idealyst/catalog.path`. The
/// file contains the absolute path to a binary that supports
/// `--emit-catalog` (the platform wrapper, when built with the
/// `dev` feature). Returns `None` if no such file exists anywhere
/// on the walk to root.
pub fn discover_catalog_bin(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join(CATALOG_PATH_RELATIVE);
        if candidate.is_file() {
            let raw = std::fs::read_to_string(&candidate).ok()?;
            let path = PathBuf::from(raw.trim());
            if path.is_file() {
                return Some(path);
            } else {
                tracing::warn!(
                    "catalog.path at {:?} references missing binary {:?}; ignoring",
                    candidate,
                    path
                );
                return None;
            }
        }
        cur = dir.parent();
    }
    None
}

/// Resolve the catalog binary the MCP server should spawn, by
/// walking cwd. Convenience wrapper around [`discover_catalog_bin`].
pub fn resolve_catalog_bin() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let found = discover_catalog_bin(&cwd)?;
    tracing::info!("catalog binary discovered at {:?}", found);
    Some(found)
}

/// Resolve the bridge address the MCP server should use, with logs.
/// `default_addr` is used when no port file is found.
pub fn resolve_bridge_addr(default_addr: &str) -> Option<String> {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("could not read cwd, using default bridge {}: {}", default_addr, e);
            return Some(default_addr.to_string());
        }
    };
    match discover(&cwd) {
        Ok(Discovered::Found { addr, project_root }) => {
            tracing::info!(
                "robot bridge discovered at {} (project: {})",
                addr,
                project_root.display()
            );
            Some(addr)
        }
        Ok(Discovered::Mismatch {
            file,
            file_project_root,
            actual_project_root,
        }) => {
            // Hard refusal — surface a loud warning. The MCP server
            // returns the "robot tools disabled" error on every Robot
            // tool call rather than connecting to the wrong app.
            tracing::warn!(
                "REFUSING bridge connection — port file {:?} belongs to a different project ({:?}) than the current working directory ({:?}). The Robot tools will report disabled until this is resolved.",
                file,
                file_project_root,
                actual_project_root,
            );
            None
        }
        Ok(Discovered::NotFound) => {
            tracing::info!(
                "no .idealyst/bridge.port found; using default bridge {}",
                default_addr
            );
            Some(default_addr.to_string())
        }
        Err(e) => {
            tracing::warn!("bridge discovery failed ({}); using default {}", e, default_addr);
            Some(default_addr.to_string())
        }
    }
}
