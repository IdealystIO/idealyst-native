//! `idealyst mcp design` — the idea-ui design MCP.
//!
//! The base `idealyst mcp` server is project-aware and deliberately neutral:
//! it describes what the framework and the project's catalog *are*. This
//! server is the opinionated layer for apps built on idea-ui: how to choose
//! a shell and navigator, lay out a screen, size spacing, write forms,
//! handle empty/loading/error states, and carry a brand through theme
//! tokens and component restyles. The brand itself — colors, type, voice —
//! always comes from the user; the guidance is about structure and use.
//!
//! Content lives in `content/` and is embedded at build time:
//!
//! - `content/instructions.md` — the server instructions every session gets.
//! - `content/skills/*.md` — design skills (rules + workflow + idea-ui wiring).
//! - `content/recipes/*.rs` — whole app modules. `mcp-design-recipes-check`
//!   compiles every one against idea-ui and server-renders it, so a recipe
//!   that stops matching idea-ui's API fails CI instead of misleading an agent.
//!
//! See [`content`] for the file formats.

pub mod content;
mod service;

pub use service::DesignService;

use anyhow::Result;
use rmcp::ServiceExt;

/// Serve the design MCP on stdio until the client disconnects.
pub async fn run_stdio() -> Result<()> {
    // stdout is the MCP channel — logs go to stderr.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
    let service = DesignService::new()
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {e:?}"))?;
    service.waiting().await?;
    Ok(())
}
