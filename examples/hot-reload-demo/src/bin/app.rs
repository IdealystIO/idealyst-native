//! App binary: connects to the dev server, replays incoming wire
//! commands through a `PrintBackend` that logs each call to stdout.
//!
//! Run:
//! ```text
//! cargo run -p hot-reload-demo --bin app
//! ```
//!
//! With both `dev-server` and `app` running, the app prints the
//! initial mount the recorder captured. Interactions (button
//! presses, etc.) round-trip back to the dev side as
//! `AppToDev::Event` messages — in this demo there's no input
//! source, so the connection is one-way once mounted.

use std::sync::mpsc;

use hot_reload_demo::print_backend::PrintBackend;
use dev_client::{connect_and_run, WireBackend};

const DEFAULT_URL: &str = "ws://127.0.0.1:9001";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_URL.to_string());

    // Outbound channel: WireBackend pushes AppToDev events here; the
    // transport loop drains it and ships them over the WebSocket.
    let (tx, rx) = mpsc::channel();
    let mut wire = WireBackend::new(PrintBackend::new(), tx);

    eprintln!("[app] connecting to {}", url);
    connect_and_run(&url, &mut wire, rx)?;
    eprintln!("[app] disconnected");
    Ok(())
}
