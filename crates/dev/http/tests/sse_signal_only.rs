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

/// The same agreement, for the hot-patch tier. The page publishes
/// `window.__idealyst_hot_patch`; the script has to call exactly that.
///
/// And the FALLBACK differs from the overlay's on purpose. An overlay
/// patch that cannot be applied is ignored, because the next rebuild
/// carries the edit anyway. A hot patch that cannot be applied means the
/// dev loop has already decided NOT to rebuild, so the page would sit
/// there running code the source no longer describes — it reloads.
#[test]
fn the_script_calls_the_hot_patch_name_and_reloads_when_it_is_missing() {
    let script = dev_http::reload_script_tag("/__idealyst/reload");
    assert!(script.contains("window.__idealyst_hot_patch"), "{script}");
    assert!(script.contains(r#"addEventListener("hot-patch""#), "{script}");

    let handler = script
        .split(r#"addEventListener("hot-patch""#)
        .nth(1)
        .expect("the hot-patch handler");
    // The branch taken when the page has no applier, up to its `return`.
    // (It also acks the failure to the dev session first.)
    let missing = handler
        .split(r#"typeof apply !== "function""#)
        .nth(1)
        .expect("the no-applier branch");
    let branch = &missing[..missing.find("return;").expect("the branch returns")];
    assert!(
        branch.contains("location.reload()"),
        "a bundle with no applier must reload, not carry on: {branch}"
    );
}

/// The two tiers travel on one ordered channel under different event
/// names, so a save that produced both reaches the page in the order the
/// dev loop decided them.
#[test]
fn the_two_patch_tiers_are_distinct_sse_events() {
    use dev_reload::PatchKind;
    assert_eq!(PatchKind::Overlay.sse_event(), "patch");
    assert_eq!(PatchKind::Hot.sse_event(), "hot-patch");
    assert_ne!(
        PatchKind::Overlay.sse_event(),
        PatchKind::Hot.sse_event(),
        "one name for both would route every hot patch into the overlay applier"
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
