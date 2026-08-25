//! Login-demo server: hosts the `#[server]` API at `/_srv/*`, serves the
//! wasm UI at `/`, and installs the auth + CSRF guards.
//!
//! ```
//! idealyst build --web examples/login-demo        # stage dist/web
//! cargo run -p login-demo --bin server --features server
//! # open http://127.0.0.1:3000/  — log in with demo / password
//! ```
//!
//! Or just `idealyst dev --web examples/login-demo`, which builds the
//! bundle, runs this bin, and rebuilds both on save.

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
    server_kit::install_middleware(server_kit::csrf_guard([
        "http://127.0.0.1:3000",
        "http://localhost:3000",
    ]));

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
