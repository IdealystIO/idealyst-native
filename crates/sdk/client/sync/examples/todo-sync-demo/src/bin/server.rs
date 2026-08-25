//! Demo server: hosts the `pull_todos` / `push_todos` sync API at
//! `/_srv/*` AND serves the wasm UI bundle at `/`.
//!
//! ```text
//! cargo run -p todo-sync-demo --bin server --features server
//! ```
//!
//! `idealyst dev --web crates/sdk/client/sync/examples/todo-sync-demo` stages the wasm bundle into
//! `<crate>/pkg/`; the server serves that at `/pkg` and the crate root
//! (which holds the committed `index.html`) at `/`. Open
//! `http://127.0.0.1:3000/`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
// Using `AppState` here is also the force-link reference into the app lib
// that keeps the linker from dead-stripping its `inventory::submit!` route
// statics — without a reference into the lib, `server::router()` would
// register zero routes and every `/_srv/<fn>` would 404. (See the same note
// in crates/api/server/examples/server-fn-demo/src/bin/server.rs.)
use todo_sync_demo::state::AppState;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // The authoritative store, pre-seeded with a couple of tasks. A real
    // app installs a DB pool here instead.
    server::install_state(Arc::new(AppState::new()));

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
        eprintln!("  idealyst dev --web crates/sdk/client/sync/examples/todo-sync-demo");
        eprintln!("(or `idealyst build --web …`) to produce the wasm bundle.");
    }

    // /_srv/*  → the sync API; /pkg/* → the wasm bundle; everything else →
    // the crate root, falling back to index.html so `/` loads the SPA.
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
    println!("todo-sync-demo:");
    println!("  UI   → http://{addr}/");
    println!("  API  → http://{addr}/_srv/<pull_todos|push_todos>");
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
