//! Dev-server binary: hosts the components in `hot_reload_demo`'s
//! library crate and serves the resulting wire command stream over
//! a WebSocket.
//!
//! When source files in this crate change, the dev-server runs
//! `cargo build -p hot-reload-demo --bin dev-server` and replaces
//! its own process image with the newly-built binary (Unix only).
//! Just before `exec`, the current navigator URL stack is
//! serialized into `IDEALYST_AAS_NAV_STATE` so the freshly-started
//! server can restore the navigation hierarchy.
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
use dev_server::{
    serve_with_robot_bridge, spawn_rebuild_loop, NavStateSnapshot, RebuildCommand, RebuildConfig,
    WireRecordingBackend,
};

/// Bind on every interface so a phone on LAN can connect. Port 0
/// lets the OS assign — the actual port is advertised via mDNS so
/// clients don't need to know it ahead of time.
const DEFAULT_ADDR: &str = "0.0.0.0:0";

/// Service id clients filter on when browsing for our mDNS record.
/// Bumping this is the way to make two dev-servers on the same
/// machine target different apps.
const APP_ID: &str = "hot-reload-demo";

fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_ADDR.to_string());

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));

    // Drive the user's tree through the real walker once at startup.
    // The recorder accumulates the commands as an append-only log;
    // new clients catch up from offset 0, existing clients advance
    // a per-client cursor as events fire reactivity.
    let owner = render(backend_rc, app_root());
    // Keep the framework runtime alive for the lifetime of the
    // process — dropping `owner` would tear down every scope and
    // free every signal that backs reactive UI.
    std::mem::forget(owner);

    eprintln!("[dev-server] initial render done");

    // If we were exec'd by a previous instance after a source
    // change, the previous server passed its navigator stack
    // forward in `IDEALYST_AAS_NAV_STATE`. Read it now and replay
    // the pushes against the freshly-built navigators so the
    // hierarchy comes back exactly where it left off.
    if let Ok(json) = std::env::var("IDEALYST_AAS_NAV_STATE") {
        match serde_json::from_str::<NavStateSnapshot>(&json) {
            Ok(saved) if !saved.is_empty() => {
                eprintln!(
                    "[dev-server] restoring nav state for {} navigator(s)",
                    saved.len()
                );
                recorder.restore_nav_state(&saved);
            }
            Ok(_) => {}
            Err(e) => eprintln!("[dev-server] could not parse saved nav state: {}", e),
        }
    }

    // File watcher + auto-rebuild. The `before_exec` hook captures
    // a clone of the nav-state mirror so it can serialize the
    // current navigator hierarchy *just before* the process image
    // swap — that's how we survive across `exec`.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let watch_path = PathBuf::from(manifest_dir).join("src");
    let nav_mirror = recorder.nav_state_mirror();
    spawn_rebuild_loop(RebuildConfig {
        watch_paths: vec![watch_path],
        command: RebuildCommand::cargo_build("hot-reload-demo", "dev-server"),
        // Tight enough to feel snappy, wide enough to coalesce
        // editor save-bursts (file-watchers commonly see multiple
        // events for one save on macOS).
        debounce: std::time::Duration::from_millis(100),
        before_exec: Some(Box::new(move || {
            let snapshot = match nav_mirror.lock() {
                Ok(g) => g.clone(),
                Err(_) => return Vec::new(),
            };
            if snapshot.is_empty() {
                return Vec::new();
            }
            match serde_json::to_string(&snapshot) {
                Ok(json) => {
                    eprintln!(
                        "[dev-server] snapshotting nav state for {} navigator(s) before exec",
                        snapshot.len()
                    );
                    vec![("IDEALYST_AAS_NAV_STATE".to_string(), json)]
                }
                Err(_) => Vec::new(),
            }
        })),
    });

    // Start the Robot bridge on the standard port. The walker
    // (above, in `render(...)`) ran on this thread, so the
    // thread-local Robot registry it populated lives here too —
    // which is the thread `serve_with_robot_bridge` will drain the
    // bridge handle on, exactly as required.
    let bridge = framework_core::robot::bridge::start(
        framework_core::robot::bridge::DEFAULT_PORT,
    );

    serve_with_robot_bridge(addr, recorder, APP_ID, bridge)
}
