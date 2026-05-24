//! Phase 5: file-watch driven catalog reload.
//!
//! On any source file change under the watched paths, this thread
//! re-extracts the catalog from `mcp_catalog::entries()` (the
//! current process's inventory slice) and swaps it into
//! [`CatalogService`] via [`replace_catalog`]. The MCP client gets
//! a `notifications/resources/list_changed` so it knows to refetch.
//!
//! **In-process limitation.** Re-extracting from this process's
//! inventory only picks up components linked into THIS binary. A
//! user editing `examples/mcp-demo/src/components.rs` and saving
//! does NOT cause new ctors to fire in the running server — the
//! linker section is fixed at link time. Two practical paths:
//!
//! 1. **Server-restart model**: a controlling process (typically
//!    `cargo idealyst mcp --watch`, phase 6) rebuilds + relaunches
//!    the entire server on source change. The server-side reload
//!    here is then redundant. Simplest, but the MCP client
//!    reconnects.
//!
//! 2. **Subprocess catalog model**: the long-running server spawns
//!    a short-lived catalog-extractor child on each change, parses
//!    its stdout JSON, replaces the catalog. The server's process
//!    stays up; the client never reconnects. Matches the user's
//!    "server doesn't go down" property.
//!
//! This module is the **in-process** flavor — useful as a building
//! block and for unit-testable reload behaviour. The subprocess
//! flavor is a thin layer on top: replace the `reload_from_inventory`
//! call below with "spawn child, read stdout, deserialize, build
//! ResolvedCatalog::build_from(...)". Marked `TODO(phase-5b)` inline.

use std::{
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use notify_debouncer_mini::{
    new_debouncer, notify::RecursiveMode, DebouncedEvent, Debouncer,
};
use rmcp::{service::Peer, RoleServer};
use tokio::sync::mpsc;

use crate::CatalogService;

/// Handle for the watcher thread; dropping it stops the watch.
pub struct WatcherHandle {
    _debouncer: Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>,
    _runtime_thread: std::thread::JoinHandle<()>,
}

/// Spawn the file-watch + reload pipeline. Watches every path in
/// `paths` recursively; debounces events at 250ms so a burst of
/// saves (rustfmt-on-save, IDE temp-file shuffles) collapses into
/// one reload.
pub(crate) fn spawn(
    service: CatalogService,
    peer: Peer<RoleServer>,
    paths: Vec<PathBuf>,
) -> Result<WatcherHandle> {
    // Bridge the (sync) notify thread → (async) tokio thread via
    // an unbounded mpsc. The notify callback can't await; the
    // consumer task owns `service` and does the async swap.
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<DebouncedEvent>>();

    let mut debouncer = new_debouncer(Duration::from_millis(250), move |res| {
        match res {
            Ok(events) => {
                if tx.send(events).is_err() {
                    // Receiver dropped — server is shutting down.
                }
            }
            Err(e) => tracing::warn!("watch error: {:?}", e),
        }
    })?;

    for p in &paths {
        if !p.exists() {
            tracing::warn!("watch path {:?} does not exist; skipping", p);
            continue;
        }
        debouncer
            .watcher()
            .watch(p, RecursiveMode::Recursive)?;
        tracing::info!("watching {:?}", p);
    }

    // The reload loop needs a tokio context to (a) drive the async
    // RwLock swap and (b) eventually drive the rmcp peer's
    // notification send. The simplest portable answer that doesn't
    // assume the caller's runtime layout: spin up a single-thread
    // tokio runtime on a dedicated OS thread for this work.
    let svc = Arc::new(service);
    let runtime_thread = std::thread::Builder::new()
        .name("mcp-catalog-watch".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("watch thread runtime");
            rt.block_on(async move {
                while let Some(events) = rx.recv().await {
                    if !is_meaningful(&events) {
                        continue;
                    }
                    tracing::info!(
                        "source change detected ({} events); reloading catalog",
                        events.len()
                    );
                    crate::watch::reload_and_notify(&svc, &peer).await;
                }
            });
        })?;

    Ok(WatcherHandle {
        _debouncer: debouncer,
        _runtime_thread: runtime_thread,
    })
}

/// Filter out events that don't change source content — editor
/// temp-files, metadata-only touches, etc. We accept anything that
/// isn't obviously noise.
fn is_meaningful(events: &[DebouncedEvent]) -> bool {
    events
        .iter()
        .any(|e| !e.path.to_string_lossy().contains("/.git/"))
}

/// Phase 5b: subprocess-driven reload. Same shape as [`spawn`]
/// but on every file change the watcher spawns `cmd_factory()`,
/// reads its stdout, parses as catalog JSON, and swaps the
/// service's in-memory catalog. The MCP server stays up; only the
/// short-lived extractor process restarts.
///
/// `cmd_factory` is called once per reload to produce a fresh
/// `Command`. The child is expected to print
/// `mcp_catalog::catalog_json()` to stdout and exit zero.
/// Non-zero exits and JSON parse failures are logged but
/// non-fatal — the previous good catalog stays in place.
pub(crate) fn spawn_subprocess(
    service: CatalogService,
    peer: Peer<RoleServer>,
    paths: Vec<PathBuf>,
    cmd_factory: Arc<dyn Fn() -> std::process::Command + Send + Sync + 'static>,
) -> Result<WatcherHandle> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<DebouncedEvent>>();
    let mut debouncer = new_debouncer(Duration::from_millis(250), move |res| match res {
        Ok(events) => {
            let _ = tx.send(events);
        }
        Err(e) => tracing::warn!("watch error: {:?}", e),
    })?;

    for p in &paths {
        if !p.exists() {
            tracing::warn!("watch path {:?} does not exist; skipping", p);
            continue;
        }
        debouncer.watcher().watch(p, RecursiveMode::Recursive)?;
        tracing::info!("watching {:?}", p);
    }

    let svc = Arc::new(service);
    let runtime_thread = std::thread::Builder::new()
        .name("mcp-catalog-subprocess-watch".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("watch thread runtime");
            rt.block_on(async move {
                // Do an initial subprocess extract so the live
                // catalog matches what the *extractor* sees, not
                // what this server process's own inventory holds.
                // Useful when the server binary itself doesn't
                // depend on the user crate.
                run_subprocess_reload(&svc, &peer, cmd_factory.as_ref()).await;

                while let Some(events) = rx.recv().await {
                    if !is_meaningful(&events) {
                        continue;
                    }
                    tracing::info!(
                        "source change detected ({} events); spawning extractor",
                        events.len()
                    );
                    run_subprocess_reload(&svc, &peer, cmd_factory.as_ref()).await;
                }
            });
        })?;

    Ok(WatcherHandle {
        _debouncer: debouncer,
        _runtime_thread: runtime_thread,
    })
}

/// Pre-serve subprocess load — used at server startup to populate
/// the catalog from the user's binary before the service is bound
/// to stdio. No peer is involved, so no notification is sent;
/// callers handle notification themselves once the peer exists.
pub(crate) async fn preload_subprocess_catalog(
    svc: &CatalogService,
    cmd_factory: &(dyn Fn() -> std::process::Command + Send + Sync + 'static),
) {
    let mut cmd = cmd_factory();
    let output = match tokio::task::spawn_blocking(move || cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!("subprocess spawn failed: {:?}", e);
            return;
        }
        Err(e) => {
            tracing::warn!("subprocess join failed: {:?}", e);
            return;
        }
    };
    if !output.status.success() {
        tracing::warn!(
            "subprocess exited with non-zero ({}); stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let json = String::from_utf8_lossy(&output.stdout);
    match mcp_catalog::ResolvedCatalog::build_from_json(&json) {
        Ok(c) => {
            svc.replace_catalog(c).await;
            tracing::info!("loaded catalog from subprocess");
        }
        Err(e) => tracing::warn!("subprocess output not valid catalog JSON: {}", e),
    }
}

/// Spawn the extractor, read its stdout, parse as catalog JSON,
/// swap the live catalog, push notifications. Logs and bails on
/// failure — the previous catalog stays untouched.
async fn run_subprocess_reload(
    svc: &Arc<CatalogService>,
    peer: &Peer<RoleServer>,
    cmd_factory: &(dyn Fn() -> std::process::Command + Send + Sync + 'static),
) {
    let mut cmd = cmd_factory();
    let output = match tokio::task::spawn_blocking(move || cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!("subprocess spawn failed: {:?}", e);
            return;
        }
        Err(e) => {
            tracing::warn!("subprocess join failed: {:?}", e);
            return;
        }
    };
    if !output.status.success() {
        tracing::warn!(
            "subprocess exited with non-zero ({}); stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let json = String::from_utf8_lossy(&output.stdout);
    let new_cat = match mcp_catalog::ResolvedCatalog::build_from_json(&json) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("subprocess output not valid catalog JSON: {}", e);
            return;
        }
    };
    svc.replace_catalog(new_cat).await;

    if let Err(e) = peer.notify_resource_list_changed().await {
        tracing::warn!("failed to notify resource list_changed: {:?}", e);
    }
    if let Err(e) = peer.notify_tool_list_changed().await {
        tracing::warn!("failed to notify tool list_changed: {:?}", e);
    }
}

/// Atomic "reload + notify" step. Pulled out so phase 5b's
/// subprocess flavor can replace just the reload half without
/// rewriting the watch loop.
///
/// Notification failures are warnings, not errors — the catalog
/// has already been swapped, and a missed `list_changed` only
/// means MCP clients see stale data until they ask again.
async fn reload_and_notify(svc: &Arc<CatalogService>, peer: &Peer<RoleServer>) {
    // TODO(phase-5b): replace `reload_from_inventory` with a
    // subprocess spawn-and-parse so newly-added components show up
    // without relinking the server. The current implementation
    // re-reads THIS process's inventory, which is fine for
    // verifying the swap pipeline but won't surface components
    // added since the binary linked.
    svc.reload_from_inventory().await;

    if let Err(e) = peer.notify_resource_list_changed().await {
        tracing::warn!("failed to notify resource list_changed: {:?}", e);
    }
    if let Err(e) = peer.notify_tool_list_changed().await {
        tracing::warn!("failed to notify tool list_changed: {:?}", e);
    }
}
