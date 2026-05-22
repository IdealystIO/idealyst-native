//! Phase 5: file-watch driven catalog reload.
//!
//! On any source file change under the watched paths, this thread
//! re-extracts the catalog from `framework_mcp::entries()` (the
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
pub(crate) fn spawn(service: CatalogService, paths: Vec<PathBuf>) -> Result<WatcherHandle> {
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
                    // TODO(phase-5b): swap this for a subprocess
                    // spawn-and-parse pipeline so newly-added
                    // components show up without relinking the
                    // server. For now, re-extract the current
                    // process's inventory — useful for verifying
                    // the swap mechanism end-to-end.
                    svc.reload_from_inventory().await;
                    // TODO(phase-5c): once the rmcp peer handle is
                    // accessible here, send
                    // `notifications/resources/list_changed`.
                    // The current rmcp 1.x API exposes that via
                    // `Service::peer().notify_*` but only after the
                    // service has been bound to a transport; we'd
                    // need to thread a `Peer<RoleServer>` clone in
                    // alongside the `CatalogService` clone above.
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
