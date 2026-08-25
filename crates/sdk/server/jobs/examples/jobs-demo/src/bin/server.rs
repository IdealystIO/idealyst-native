//! Demo server: serves the wasm UI at `/`, the `#[server]` API at `/_srv/*`,
//! and runs the jobs worker **in-process** so `idealyst dev` works end-to-end
//! with no external broker.
//!
//! ```text
//! cargo run -p jobs-demo --bin server --features server
//! ```
//!
//! With the default in-memory backend the worker must live in this process
//! (a separate process couldn't see the same queue). Point `dev.toml`'s
//! `[jobs]` block at a shared broker to instead run a dedicated worker
//! (`src/bin/worker.rs`, which `idealyst dev` then auto-spawns).

use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use axum::Router;
// Referencing the app lib here is the force-link anchor that keeps the linker
// from dead-stripping the `#[server]`/`#[job]` `inventory::submit!` statics —
// without a reference into the lib, `router()` registers zero routes and the
// worker finds zero handlers.
use jobs_demo::Mailer;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // Install the state job handlers inject via `State<Mailer>`.
    server::install_state(Mailer {
        from: "hello@jobs-demo.dev".to_string(),
        sent: Arc::new(AtomicU32::new(0)),
    });

    // Configure every SDK from `idealyst.toml` in one call (memory backend by
    // default; a `[jobs] connection = "..."` block points at a shared broker).
    // Env vars still override. This replaces the per-SDK `configure_from_env`.
    if let Err(e) = idealyst_config::configure_all().await {
        eprintln!("[server] config failed: {e}");
    }

    // Run the worker in-process. Held for the server's lifetime.
    let _worker = jobs::worker().spawn();

    // Force-link anchor into the app lib (see note above).
    let _anchor = jobs_demo::signup;

    // Where the CLI staged the web bundle.
    //
    // `WEB_DIST` is exported by `idealyst dev --web` and `idealyst run
    // server`; the baked `dist/web` is the fallback for a plain `cargo
    // run`. Reading the env FIRST is the load-bearing part — it is what
    // keeps this bin correct if the CLI's staging layout moves again.
    //
    // It moved once already, and this file didn't follow: it served
    // `<crate>/pkg` (the pre-`dist/web` layout) while the CLI staged into
    // `<crate>/dist/web`, so every `idealyst dev --web` session 404'd at
    // `/` while reporting a perfectly successful build on every save.
    //
    // The absolute crate dir is baked at compile time rather than derived
    // from `current_dir()`, which only works when run from the workspace
    // root — the other 404-on-root trap.
    let dist_dir = std::env::var_os("WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("dist")
                .join("web")
        });
    let pkg_dir = dist_dir.join("pkg");
    let static_dir = dist_dir.clone();
    if !pkg_dir.exists() {
        eprintln!("warning: {} doesn't exist yet — run", dist_dir.display());
        eprintln!("  idealyst dev --web crates/sdk/server/jobs/examples/jobs-demo");
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
    println!("jobs-demo:");
    println!("  UI     → http://{addr}/");
    println!("  API    → http://{addr}/_srv/signup");
    println!("  worker → in-process (memory backend)");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
