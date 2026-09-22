//! The reload/overlay stream, standing alone beside an app's own server.
//!
//! In the full-stack shape a project's own server hands out
//! `index.html` and `dev-http` never runs as a file server, so the page
//! has no push channel at all unless this stream is running. Two things
//! have to hold for it to reach that page:
//!
//! - the stream answers on its own port with nothing else served there;
//! - it sends `Access-Control-Allow-Origin`, because the page is on the
//!   app server's origin and this is on another port. Without the
//!   header the browser drops the `EventSource` — and drops it
//!   SILENTLY, retrying forever while the author watches a dev loop
//!   that never pushes anything.
//!
//! Raw `TcpStream` rather than an HTTP client, for the same reason
//! `sse_reload.rs` uses one: no extra dependency, and the protocol is
//! simple enough to read inline.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use dev_http::{serve_signal_only, RELOAD_SSE_URL};
use dev_reload::ReloadSignal;

fn pick_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn read_some(stream: &mut TcpStream, deadline: Instant) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while Instant::now() < deadline {
        stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:3100\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    read_some(&mut stream, Instant::now() + Duration::from_secs(3))
}

fn start(port: u16) -> std::sync::Arc<ReloadSignal> {
    let signal = ReloadSignal::new();
    let for_server = signal.clone();
    thread::spawn(move || {
        let _ = serve_signal_only("127.0.0.1", port, for_server);
    });
    // Give the listener a moment to bind.
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    signal
}

/// The injected script and the bundle have to agree on ONE name.
///
/// `#[wasm_bindgen]` puts an export on the MODULE, not on `window`, so
/// `backend-web` explicitly publishes `window.__idealyst_overlay_patch`
/// at boot. This pins the script's half of that agreement; the bundle's
/// half is `backend_web::newcore::install_overlay_patch_entry`.
///
/// Getting it wrong is quiet: the script looks, finds nothing, logs
/// "this bundle has no overlay" and does nothing — on a bundle that has
/// one. That is exactly how it failed the first time, and only looking
/// at a real page caught it.
#[test]
fn the_injected_script_calls_the_name_the_bundle_publishes() {
    let script = dev_http::reload_script_tag("http://127.0.0.1:1234/__idealyst/reload");
    assert!(
        script.contains("window.__idealyst_overlay_patch"),
        "{script}"
    );
    assert!(
        script.contains("http://127.0.0.1:1234/__idealyst/reload"),
        "{script}"
    );
}

#[test]
fn the_sse_route_allows_a_cross_origin_page() {
    let port = pick_port();
    let _signal = start(port);

    let head = get(port, RELOAD_SSE_URL);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("access-control-allow-origin: *"),
        "the page is on another origin; without this the browser drops the stream:\n{head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("content-type: text/event-stream"),
        "{head}"
    );
}

/// It serves the stream and NOTHING else. A full-stack project's files
/// come from its own server, and quietly answering for them here would
/// be a second source of truth for the same page.
#[test]
fn nothing_else_is_served() {
    let port = pick_port();
    let _signal = start(port);

    let head = get(port, "/index.html");
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("access-control-allow-origin: *"),
        "even the 404 is cross-origin-readable, so a misconfigured URL \
         surfaces as a 404 rather than an opaque CORS error:\n{head}"
    );
}
