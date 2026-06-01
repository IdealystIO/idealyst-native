//! Login-demo server: hosts the `#[server]` API at `/_srv/*`, serves the
//! wasm UI at `/`, and installs the auth + CSRF guards.
//!
//! ```
//! idealyst build --web examples/login-demo        # produce pkg/
//! cargo run -p login-demo --bin server --features server
//! # open http://127.0.0.1:3000/  — log in with demo / password
//! ```

use std::path::PathBuf;

use axum::Router;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // Auth guard: reads the httpOnly `session` cookie on every request and
    // injects `Principal` for `Auth<Principal>` handlers.
    login_demo::install_auth_guard();

    // CSRF defense-in-depth: reject any request whose Origin isn't this
    // dev host. (The SameSite=Lax cookie is the primary defense; this is
    // belt-and-suspenders.) In production, list your real web origin(s).
    server::install_middleware(server::csrf_guard([
        "http://127.0.0.1:3000",
        "http://localhost:3000",
    ]));

    // Absolute crate directory, baked in at compile time — robust to the CWD
    // the server is launched from (workspace root, the example folder, or
    // whatever `idealyst dev` uses). Resolving from `current_dir()` only works
    // when run from the workspace root, which is the 404-on-root trap.
    let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pkg_dir = project_dir.join("pkg");
    let static_dir = project_dir.clone();

    if !pkg_dir.exists() {
        eprintln!("warning: {} doesn't exist yet — run", pkg_dir.display());
        eprintln!("  idealyst build --web examples/login-demo");
        eprintln!("to produce the wasm bundle before opening the page.");
    }

    let app: Router = server::router()
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(
                ServeDir::new(&static_dir).append_index_html_on_directories(true),
            ),
        );

    let addr: std::net::SocketAddr = "127.0.0.1:3000".parse().unwrap();
    println!("login-demo:");
    println!("  UI  → http://{addr}/   (log in with demo / password)");
    println!("  API → http://{addr}/_srv/<fn-name>");
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
