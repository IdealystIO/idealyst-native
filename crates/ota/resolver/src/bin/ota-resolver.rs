//! The resolution service as a long-running server (a container, a VM).
//! `OTA_LOCATION` and the settings in the crate docs; `PORT` (3200) and
//! `OTA_RESOLVE_BIND` (127.0.0.1; `0.0.0.0` in a container).

use std::sync::Arc;

#[tokio::main]
async fn main() {
    let resolver = ota_resolver::Resolver::from_env().unwrap_or_else(|e| {
        eprintln!("ota-resolver: {e:#}");
        std::process::exit(2)
    });
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3200);
    let bind = std::env::var("OTA_RESOLVE_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let addr: std::net::SocketAddr = format!("{bind}:{port}").parse().expect("OTA_RESOLVE_BIND:PORT is an address");
    println!("ota-resolver: {} — http://{addr}/v1/resolve", resolver.target());
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, ota_resolver::router(Arc::new(resolver))).await.expect("serve");
}
