//! Stdio MCP server over the framework's component catalog.
//!
//! Consumers wire this up by calling [`run_stdio`] from their own
//! binary's `tokio::main`. The catalog is read from the global
//! `framework_mcp::inventory` slice at server-start time. For live
//! catalog updates on source changes see [`run_stdio_with_watch`].
//!
//! See `docs/framework-mcp-spec.md` §5 for the MCP surface this
//! implements.

mod catalog_service;
pub mod lint;
mod watch;

pub use catalog_service::CatalogService;
pub use lint::{run as lint_catalog, LintFinding, Severity};

use anyhow::Result;
use rmcp::ServiceExt;

/// Start the MCP server on stdio and wait until the client
/// disconnects. The catalog is loaded once from
/// `framework_mcp::entries()` and stays static for the server's
/// lifetime. For live reload use [`run_stdio_with_watch`].
pub async fn run_stdio() -> Result<()> {
    init_tracing();
    tracing::info!("starting MCP catalog server (static catalog)");

    let service = CatalogService::new()
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;

    service.waiting().await?;
    Ok(())
}

/// Start the MCP server on stdio AND watch the supplied source
/// directories for changes. On any change, [`CatalogService`] is
/// signalled to re-extract the catalog from inventory and push
/// `notifications/resources/list_changed` to the client.
///
/// **Important**: rebuilding the catalog at runtime only picks up
/// components whose `inventory::submit!` ctors are already linked
/// into THIS process. Editing source files alone does not magically
/// surface new components; the recommended deployment pattern is to
/// spawn a separate "catalog extractor" subprocess on each rebuild,
/// pipe its catalog JSON back, and replace the in-memory catalog —
/// see watcher details in `crates/mcp-server/src/watch.rs`.
pub async fn run_stdio_with_watch(paths: Vec<std::path::PathBuf>) -> Result<()> {
    init_tracing();
    tracing::info!("starting MCP catalog server (live reload watching {:?})", paths);

    let service = CatalogService::new();
    let watcher_handle = watch::spawn(service.clone(), paths)?;

    let running = service
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;

    running.waiting().await?;
    drop(watcher_handle);
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
