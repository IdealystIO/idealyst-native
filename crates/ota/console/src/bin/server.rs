//! The console's server: the `#[server]` API at `/_srv/*`, the page at `/`.
//!
//! Configured from the environment (see README.md):
//! - `OTA_CONSOLE_LOCATION` — the release location to manage:
//!   `s3://bucket/prefix` (with the AWS_* variables: keys, region, and
//!   `AWS_ENDPOINT_URL` for MinIO or another S3-compatible store), or a
//!   directory.
//! - `PORT` — default 3100.
//!
//! - `OTA_CONSOLE_BIND` — default 127.0.0.1: no sign-in yet, so the page
//!   listens locally unless told otherwise.
//!
//! Without a location, or with one it can't read, the server still starts
//! and the page says what's wrong.

use std::path::PathBuf;
use std::sync::Arc;

use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    // Configuration problems are reported on the page, not by exiting: the
    // server always comes up (see `Console`).
    let target = match std::env::var("OTA_CONSOLE_LOCATION") {
        Err(_) => Err("No release location is configured.".to_string()),
        Ok(location) => ota_publish::connect(&location).map_err(|e| format!("{location}: {e:#}.")),
    };
    match &target {
        Ok(t) => {
            // Read the index once, so a wrong key or a stopped store shows
            // here too — but keep going: the page retries, and says why.
            let probe = t.clone();
            match tokio::task::spawn_blocking(move || ota_publish::read_index(&probe)).await.expect("the probe ran") {
                Ok(index) => println!("ota-console: {t} — {} bundle(s)", index.bundles.len()),
                Err(e) => eprintln!("ota-console: warning: can't read {t} yet: {e:#}"),
            }
        }
        Err(why) => eprintln!("ota-console: warning: {why} {}", ota_console::SETUP),
    }
    server::install_state(Arc::new(ota_console::Console { target }));

    // Where the CLI staged the page (`idealyst dev --web` / `idealyst run
    // server` export WEB_DIST); `dist/web` for a plain `cargo run`.
    let dist = std::env::var_os("WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist").join("web"));
    if !dist.join("pkg").exists() {
        eprintln!("warning: {} has no page yet — run `idealyst build --web crates/ota/console`", dist.display());
    }
    let app = server::router()
        .nest_service("/pkg", ServeDir::new(dist.join("pkg")))
        .fallback_service(ServeDir::new(&dist).not_found_service(ServeDir::new(&dist).append_index_html_on_directories(true)));

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3100);
    let bind = std::env::var("OTA_CONSOLE_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let addr: std::net::SocketAddr = format!("{bind}:{port}").parse().expect("OTA_CONSOLE_BIND:PORT is an address");
    println!("ota-console: http://{addr}/");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
