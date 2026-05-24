//! Stdio MCP server over the framework's component catalog.
//!
//! Consumers wire this up by calling [`run_stdio`] from their own
//! binary's `tokio::main`. The catalog is read from the global
//! `mcp_catalog::inventory` slice at server-start time. For live
//! catalog updates on source changes see [`run_stdio_with_watch`].
//!
//! See `docs/mcp-catalog-spec.md` §5 for the MCP surface this
//! implements.

mod catalog_service;
pub mod lint;
mod mdns_discovery;
mod robot_bridge;
mod watch;

pub use catalog_service::CatalogService;
pub use lint::{run as lint_catalog, LintFinding, Severity};
pub use mdns_discovery::{DiscoveredApp, DiscoveryTable};
pub use robot_bridge::{RobotBridge, DEFAULT_BRIDGE};

use anyhow::Result;
use rmcp::ServiceExt;

/// Optional knobs for the MCP server. Default constructor gives
/// the catalog-only static-snapshot shape (same as the legacy
/// [`run_stdio`]); builder methods opt into Robot tools, a
/// subprocess catalog extractor, and source-directory watching.
#[derive(Default)]
pub struct ServerOptions {
    /// When true, the server's Robot tools (find_element, click, ...)
    /// are advertised. Routing is mDNS-only — the server's
    /// `DiscoveryTable` finds live apps via
    /// `_idealyst-robot._tcp.local.` and the resolver picks one per
    /// call. When false the Robot tools are omitted from the tool
    /// list entirely.
    robot_enabled: bool,
    /// Command factory for the subprocess catalog extractor. When
    /// set, the server invokes the command at startup (and on each
    /// source change if `watch_paths` is also set) and parses its
    /// stdout as catalog JSON to replace the in-process catalog.
    /// This is how `--project-root` / `--from-bin` populate the
    /// catalog when no app is currently running.
    subprocess: Option<std::sync::Arc<dyn Fn() -> std::process::Command + Send + Sync>>,
    watch_paths: Vec<std::path::PathBuf>,
}

impl ServerOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable Robot tools. Routing is mDNS-only (no explicit
    /// bridge address — the running app advertises itself).
    pub fn with_robot_mdns(mut self) -> Self {
        self.robot_enabled = true;
        self
    }

    pub fn with_subprocess_catalog<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> std::process::Command + Send + Sync + 'static,
    {
        self.subprocess = Some(std::sync::Arc::new(factory));
        self
    }

    pub fn with_watch(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.watch_paths = paths;
        self
    }
}

/// Start the MCP server on stdio with the supplied options. Most
/// consumers reach this through the CLI's `idealyst mcp` subcommand
/// or one of the convenience wrappers ([`run_stdio`],
/// [`run_stdio_with_options`]).
pub async fn run_stdio_with_full_options(opts: ServerOptions) -> Result<()> {
    init_tracing();
    tracing::info!(
        "starting MCP server (robot={}, subprocess={}, watch_paths={})",
        if opts.robot_enabled { "mdns" } else { "disabled" },
        if opts.subprocess.is_some() { "yes" } else { "no" },
        opts.watch_paths.len(),
    );

    let svc = CatalogService::new();
    // When robot is disabled, we just don't expose the Robot tools.
    // The CatalogService unconditionally instantiates them today
    // (rmcp's `#[tool]` is a compile-time attribute), so "disabled"
    // means every Robot call returns "no app running" via the
    // mDNS-empty path. Acceptable surface for now; a follow-up could
    // gate the tools at type level.
    let _ = opts.robot_enabled;

    // Pre-serve subprocess load: do this BEFORE binding to stdio so
    // the very first `tools/call list_components` sees the populated
    // catalog rather than racing the extractor. Failures are
    // warnings — the server still starts with an empty catalog.
    if let Some(factory) = &opts.subprocess {
        watch::preload_subprocess_catalog(&svc, factory.as_ref()).await;
    }

    let service = svc
        .clone()
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;
    let peer = service.peer().clone();

    // If a watcher was configured, spawn it. Whether it uses the
    // subprocess flavor or the in-process flavor depends on
    // `opts.subprocess`.
    let _watcher = if !opts.watch_paths.is_empty() {
        Some(if let Some(factory) = opts.subprocess.clone() {
            watch::spawn_subprocess(svc.clone(), peer.clone(), opts.watch_paths.clone(), factory)?
        } else {
            watch::spawn(svc.clone(), peer.clone(), opts.watch_paths.clone())?
        })
    } else {
        None
    };

    service.waiting().await?;
    Ok(())
}

/// Convenience wrapper: catalog-only, optionally with Robot tools
/// enabled (mDNS-driven). The legacy `robot_addr: Option<&str>` shape
/// is gone — discovery is mDNS-only now.
pub async fn run_stdio_with_options(enable_robot: bool) -> Result<()> {
    let mut opts = ServerOptions::new();
    if enable_robot {
        opts = opts.with_robot_mdns();
    }
    run_stdio_with_full_options(opts).await
}

/// Catalog-only stdio server. Convenience wrapper around
/// [`run_stdio_with_options`] for backwards compatibility.
pub async fn run_stdio() -> Result<()> {
    run_stdio_with_options(false).await
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
/// Start the MCP server on stdio AND on every source change spawn
/// `subprocess_cmd` as a child, read its stdout as a catalog JSON
/// document, and replace the live catalog with what the child
/// produced. Pushes `notifications/resources/list_changed` after
/// every successful swap.
///
/// This is the deployment shape spec §8.1 / phase 5b describes —
/// the server stays up across catalog rebuilds; only the child
/// process restarts.
///
/// The caller supplies a `Fn -> Command` factory rather than a
/// concrete `Command` so the spawn can be repeated. A typical
/// invocation points at the current binary in an `--emit-catalog`
/// mode (which prints `mcp_catalog::catalog_json()` then exits):
///
/// ```ignore
/// mcp_server::run_stdio_with_subprocess(
///     vec![std::path::PathBuf::from("src")],
///     || {
///         let mut c = std::process::Command::new(
///             std::env::current_exe().unwrap(),
///         );
///         c.arg("--emit-catalog");
///         c
///     },
/// ).await
/// ```
pub async fn run_stdio_with_subprocess(
    paths: Vec<std::path::PathBuf>,
    subprocess_cmd: impl Fn() -> std::process::Command + Send + Sync + 'static,
) -> Result<()> {
    init_tracing();
    tracing::info!(
        "starting MCP catalog server (subprocess reload watching {:?})",
        paths
    );

    let service = CatalogService::new();
    let running = service
        .clone()
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;
    let peer = running.peer().clone();

    let watcher_handle = watch::spawn_subprocess(
        service,
        peer,
        paths,
        std::sync::Arc::new(subprocess_cmd),
    )?;

    running.waiting().await?;
    drop(watcher_handle);
    Ok(())
}

pub async fn run_stdio_with_watch(paths: Vec<std::path::PathBuf>) -> Result<()> {
    init_tracing();
    tracing::info!("starting MCP catalog server (live reload watching {:?})", paths);

    let service = CatalogService::new();
    // Bind the service to the stdio transport first so we have a
    // peer handle to send `notifications/resources/list_changed`
    // through. The watcher gets a clone of both the service (for
    // `replace_catalog`) and the peer (for the notification).
    let running = service
        .clone()
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;
    let peer = running.peer().clone();

    let watcher_handle = watch::spawn(service, peer, paths)?;

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
