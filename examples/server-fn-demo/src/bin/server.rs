//! Demo server: hosts the `#[server]` API at `/_srv/*` AND serves
//! the wasm UI bundle at `/`.
//!
//! ```
//! cargo run -p server-fn-demo --bin server --features server
//! ```
//!
//! Static files come from the `pkg/` directory the CLI populates
//! when you run `idealyst build --web examples/server-fn-demo`,
//! plus the hand-written `index.html` next to the package's
//! `Cargo.toml`. Open `http://127.0.0.1:3000/` to see the UI.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use server_fn_demo::state::AppState;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // App-level state. Real apps install a DB pool here.
    server::install_state(Arc::new(AppState::new()));

    // Absolute crate directory, baked in at compile time — robust to the CWD
    // the server is launched from (workspace root, the example folder, or
    // whatever `idealyst dev` uses). Resolving from `current_dir()` only works
    // when run from the workspace root, which is the 404-on-root trap.
    let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // The CLI writes the wasm bundle to `pkg/` next to the
    // crate's index.html.
    let pkg_dir = project_dir.join("pkg");
    let static_dir = project_dir.clone();

    if !pkg_dir.exists() {
        eprintln!("warning: {} doesn't exist yet — run", pkg_dir.display());
        eprintln!("  idealyst build --web examples/server-fn-demo");
        eprintln!("to produce the wasm bundle before opening the page.");
    }

    // Compose the router:
    //   /_srv/_batch and /_srv/*path  → server::router()
    //   /pkg/*                        → ServeDir(pkg_dir)
    //   everything else (e.g. /)      → ServeDir(project_dir) which
    //                                    serves index.html on a path
    //                                    miss because of `not_found_service`.
    let app: Router = server::router()
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(
                ServeDir::new(&static_dir).append_index_html_on_directories(true),
            ),
        );

    let addr: std::net::SocketAddr = "127.0.0.1:3000".parse().unwrap();
    println!("server-fn-demo:");
    println!("  UI       → http://{addr}/");
    println!("  API      → http://{addr}/_srv/<fn-name>");
    println!("  pkg/     → {}", pkg_dir.display());
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
