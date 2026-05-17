//! Dev-server binary: hosts the components in `hot_reload_demo`'s
//! library crate and serves the resulting wire command stream over
//! a WebSocket.
//!
//! When source files in this crate change, the dev-server runs
//! `cargo build -p hot-reload-demo --bin dev-server` and replaces
//! its own process image with the newly-built binary (Unix only).
//! The connected browser sees the WebSocket close, waits a moment,
//! and reloads — automatically picking up the new tree.
//!
//! Run:
//! ```text
//! cargo run -p hot-reload-demo --bin dev-server
//! ```

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use framework_core::render;
use hot_reload_demo::app_root;
use idealyst_dev_server::{
    serve, spawn_rebuild_loop, RebuildCommand, RebuildConfig, WireRecordingBackend,
};

const DEFAULT_ADDR: &str = "127.0.0.1:9001";

fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_ADDR.to_string());

    // File watch + auto-rebuild loop. The thread runs for the
    // lifetime of the process; on a successful rebuild it replaces
    // *this* process image with the new binary via exec.
    //
    // `CARGO_MANIFEST_DIR` is set at compile time to this crate's
    // root, so the src/ path is always correct regardless of where
    // the binary is invoked from.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let watch_path = PathBuf::from(manifest_dir).join("src");
    spawn_rebuild_loop(RebuildConfig {
        watch_paths: vec![watch_path],
        command: RebuildCommand::cargo_build("hot-reload-demo", "dev-server"),
        // Tight enough to feel snappy, wide enough to coalesce
        // editor save-bursts (file-watchers commonly see multiple
        // events for one save on macOS).
        debounce: std::time::Duration::from_millis(100),
    });

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Drive the user's tree through the real walker once at startup
    // so the recorder captures the initial mount.
    let owner = render(backend_rc, app_root());
    // Keep the framework runtime alive for the lifetime of the
    // process — dropping `owner` would tear down every scope.
    std::mem::forget(owner);

    eprintln!("[dev-server] initial render captured");
    serve(addr, recorder)
}
