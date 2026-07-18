//! Demo server: serves the wasm UI at `/`, the `#[server]` API + the
//! `#[subscription]` WebSocket at `/_srv/*`, and configures the pub/sub backend.
//!
//! ```text
//! cargo run -p pubsub-demo --bin server --features server
//! ```
//!
//! With the default memory backend, fan-out is within this one process. Point
//! `dev.toml`'s `[pubsub]` at redis/postgres and run TWO instances on different
//! ports — a message sent to one instance reaches clients subscribed on the
//! other. That's the decentralized WebSocket fan-out.

use std::path::PathBuf;

use axum::Router;
// Force-link anchor: referencing the lib keeps the linker from dead-stripping
// the `#[server]`/`#[subscription]` `inventory::submit!` route statics.
use pubsub_demo::say as _force_link;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // Configure every SDK from `idealyst.toml` in one call (memory backend by
    // default; a `[pubsub] connection = "..."` block points at a shared broker
    // for cross-instance fan-out). Env vars still override.
    if let Err(e) = idealyst_config::configure_all().await {
        eprintln!("[server] config failed: {e}");
    }
    let _ = _force_link;

    let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pkg_dir = project_dir.join("pkg");
    let static_dir = project_dir.clone();
    if !pkg_dir.exists() {
        eprintln!("warning: {} doesn't exist yet — run", pkg_dir.display());
        eprintln!("  idealyst dev --web crates/sdk/server/pubsub/examples/pubsub-demo");
    }

    let app: Router = server::router()
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(
                ServeDir::new(&static_dir).append_index_html_on_directories(true),
            ),
        );

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    println!("pubsub-demo:");
    println!("  UI   → http://{addr}/");
    println!("  WS   → ws://{addr}/_srv/_ws/room_feed");
    println!("  API  → http://{addr}/_srv/say");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
