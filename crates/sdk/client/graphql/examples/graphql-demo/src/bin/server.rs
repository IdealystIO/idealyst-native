//! Demo server: hosts the GraphQL endpoint at `/_srv/graphql_endpoint` AND
//! serves the wasm UI bundle at `/`.
//!
//! ```text
//! cargo run -p graphql-demo --bin server --features server
//! ```
//!
//! `idealyst dev --web crates/sdk/client/graphql/examples/graphql-demo` stages the wasm bundle into
//! `<crate>/pkg/`; the server serves that at `/pkg` and the crate root
//! (which holds the committed `index.html`) at `/`. Open
//! `http://127.0.0.1:3000/`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
// Referencing the app lib's schema here is also the force-link anchor that
// keeps the linker from dead-stripping the `#[server]` route's
// `inventory::submit!` static — without a reference into the lib,
// `server::router()` would register zero routes and `/_srv/*` would 404.
// (Same note as crates/sdk/client/sync/examples/todo-sync-demo/src/bin/server.rs.)
use graphql_demo::schema;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // The authoritative async-graphql schema, pre-seeded with one book.
    server::install_state(Arc::new(schema::build()));

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
        eprintln!("  idealyst dev --web crates/sdk/client/graphql/examples/graphql-demo");
        eprintln!("(or `idealyst build --web …`) to produce the wasm bundle.");
    }

    // /_srv/*  → the GraphQL endpoint; /pkg/* → the wasm bundle; everything
    // else → the crate root, falling back to index.html so `/` loads the SPA.
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
    println!("graphql-demo:");
    println!("  UI   → http://{addr}/");
    println!("  API  → http://{addr}/_srv/graphql_endpoint");
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
