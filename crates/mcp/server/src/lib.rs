//! Stdio MCP server over the framework's component catalog.
//!
//! Consumers wire this up by calling [`run_stdio`] from their own
//! binary's `tokio::main`. The catalog is read from the global
//! `mcp_catalog::inventory` slice at server-start time. For live
//! catalog updates on source changes see [`run_stdio_with_watch`].
//!
//! See `docs/mcp-catalog-spec.md` §5 for the MCP surface this
//! implements.

mod adb;
mod app_discovery;
mod catalog_service;
pub mod lint;
mod robot_bridge;
mod watch;

pub use app_discovery::{DiscoveredApp, DiscoveryTable};
pub use catalog_service::CatalogService;
pub use lint::{run as lint_catalog, run_with as lint_catalog_with, LintFinding, LintOptions, Severity};
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
    /// are advertised. Routing is file-discovery only — the server's
    /// `DiscoveryTable` scans `~/.idealyst/apps/<name>-<pid>.json`
    /// registration files written by the running app's Robot bridge,
    /// and the resolver picks one per call. When false, robot control
    /// is fully off: no discovery thread, no bridge contact (control
    /// OR catalog), and the Robot tools return a clear error.
    robot_enabled: bool,
    /// Explicit `host:port` of a Robot bridge to target directly,
    /// skipping `~/.idealyst/apps/` discovery. Set via
    /// [`Self::with_robot_address`] (the CLI's `--robot-port` /
    /// `--robot-host`). Implies `robot_enabled`. `None` → discovery.
    robot_addr: Option<String>,
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

    /// Enable Robot tools. Routing reads `~/.idealyst/apps/`
    /// registration files written by the running app's bridge —
    /// no explicit bridge address needed.
    pub fn with_robot_discovery(mut self) -> Self {
        self.robot_enabled = true;
        self
    }

    /// Enable Robot tools against an explicit bridge `host:port`,
    /// skipping discovery. Use when the connection is known up front
    /// (the CLI can establish it for you). Implies robot enabled.
    pub fn with_robot_address(mut self, addr: impl Into<String>) -> Self {
        self.robot_enabled = true;
        self.robot_addr = Some(addr.into());
        self
    }

    /// Deprecated alias for [`Self::with_robot_discovery`] kept so
    /// out-of-tree consumers don't break across the discovery
    /// transport change.
    #[deprecated(note = "renamed to with_robot_discovery; mDNS no longer used")]
    pub fn with_robot_mdns(self) -> Self {
        self.with_robot_discovery()
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
    let robot_desc = match (opts.robot_enabled, opts.robot_addr.as_deref()) {
        (false, _) => "disabled".to_string(),
        (true, Some(addr)) => format!("explicit({addr})"),
        (true, None) => "discovery".to_string(),
    };
    tracing::info!(
        "starting MCP server (robot={}, subprocess={}, watch_paths={})",
        robot_desc,
        if opts.subprocess.is_some() { "yes" } else { "no" },
        opts.watch_paths.len(),
    );

    // rmcp's `#[tool]` is a compile-time attribute, so the Robot tools
    // are always present in the tool list. The robot MODE gates their
    // behavior: Disabled → they return a "robot is off" error and no
    // discovery thread runs; Explicit → they hit the pinned bridge;
    // Discovery → per-call `~/.idealyst/apps/` routing.
    let svc = CatalogService::with_robot_mode(opts.robot_enabled, opts.robot_addr.clone());

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
/// enabled. Discovery scans `~/.idealyst/apps/` for live registrations.
pub async fn run_stdio_with_options(enable_robot: bool) -> Result<()> {
    let mut opts = ServerOptions::new();
    if enable_robot {
        opts = opts.with_robot_discovery();
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
