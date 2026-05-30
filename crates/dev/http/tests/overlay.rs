//! Integration coverage for [`OverlayContext`].
//!
//! Two contracts:
//!
//! 1. A request whose path resolves under an overlay root (but NOT
//!    under the main serve root) gets served from the overlay. This
//!    is what makes `idealyst dev --web` deliver favicons that live
//!    under `target/idealyst/dev/web/` without polluting the project
//!    tree.
//!
//! 2. The main serve root wins when both roots have a file at the
//!    same path. Overlays are fallbacks, never overrides — a user
//!    asset always shadows a framework-generated one.
//!
//! Uses raw TCP for the same reason `sse_reload.rs` does: HTTP is
//! simple enough to parse inline, and no extra deps means the test
//! ships cheap and stays stable across reqwest/hyper churn.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use dev_http::{serve_static, OverlayContext};

fn pick_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn http_get(port: u16, path: &str) -> (u16, Vec<u8>) {
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < connect_deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("connect failed: {e}"),
        }
    };
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();

    // Parse the status line and split headers from body. We only
    // need the code + body bytes; header parsing stays minimal so
    // this helper doesn't grow legs.
    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response missing CRLF CRLF");
    let head = std::str::from_utf8(&buf[..head_end]).unwrap();
    let status_line = head.lines().next().unwrap();
    let status: u16 = status_line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = buf[head_end + 4..].to_vec();
    (status, body)
}

fn spawn_server(root: &Path, overlay_root: Option<&Path>) -> u16 {
    let port = pick_port();
    let root = root.to_path_buf();
    let overlay = overlay_root.map(|p| p.to_path_buf());
    thread::spawn(move || {
        let overlay_ctx = overlay.map(|root| OverlayContext { roots: vec![root] });
        let _ = serve_static(
            "127.0.0.1",
            port,
            &root,
            None,
            None,
            None,
            overlay_ctx,
            None,
        );
    });
    port
}

#[test]
fn overlay_root_serves_file_missing_from_main_root() {
    let tmp = tempfile::tempdir().unwrap();
    let main_root = tmp.path().join("project");
    let overlay = tmp.path().join("overlay");
    std::fs::create_dir_all(&main_root).unwrap();
    std::fs::create_dir_all(&overlay).unwrap();

    // The project tree carries an index.html (so the server has a
    // legitimate main root) but NO favicon — exactly the shape
    // `idealyst dev --web` produces before our overlay kicks in.
    std::fs::write(main_root.join("index.html"), b"<html></html>").unwrap();

    // The overlay carries a favicon.ico. After overlay support, a
    // GET /favicon.ico must hit it.
    let favicon_bytes = b"\x00\x00\x01\x00fake-icon-bytes";
    std::fs::write(overlay.join("favicon.ico"), favicon_bytes).unwrap();

    let port = spawn_server(&main_root, Some(&overlay));
    let (status, body) = http_get(port, "/favicon.ico");

    assert_eq!(status, 200, "overlay-served file must respond 200");
    assert_eq!(
        body, favicon_bytes,
        "overlay-served body must match the file bytes",
    );
}

#[test]
fn main_root_wins_when_both_have_the_same_path() {
    let tmp = tempfile::tempdir().unwrap();
    let main_root = tmp.path().join("project");
    let overlay = tmp.path().join("overlay");
    std::fs::create_dir_all(&main_root).unwrap();
    std::fs::create_dir_all(&overlay).unwrap();

    std::fs::write(main_root.join("index.html"), b"<html></html>").unwrap();
    // Both roots carry favicon.ico — the main root's content must
    // win so a user-committed asset is never silently shadowed by
    // an auto-generated one.
    let user_bytes = b"USER-COMMITTED-BYTES";
    let overlay_bytes = b"FRAMEWORK-GENERATED-BYTES";
    std::fs::write(main_root.join("favicon.ico"), user_bytes).unwrap();
    std::fs::write(overlay.join("favicon.ico"), overlay_bytes).unwrap();

    let port = spawn_server(&main_root, Some(&overlay));
    let (status, body) = http_get(port, "/favicon.ico");

    assert_eq!(status, 200);
    assert_eq!(
        body, user_bytes,
        "main root must win — overlay is a fallback, not an override",
    );
}

#[test]
fn missing_overlay_root_is_skipped_not_fatal() {
    // Mirrors the dev-time reality: the overlay dir may not exist
    // yet when the server starts (icon-gen hasn't run, or the user
    // ran `cargo clean`). The server must skip the missing root
    // and still serve files from the main root.
    let tmp = tempfile::tempdir().unwrap();
    let main_root = tmp.path().join("project");
    std::fs::create_dir_all(&main_root).unwrap();
    std::fs::write(main_root.join("index.html"), b"<html>main</html>").unwrap();

    let missing_overlay = tmp.path().join("does-not-exist");
    let port = spawn_server(&main_root, Some(&missing_overlay));

    let (status, body) = http_get(port, "/index.html");
    assert_eq!(status, 200);
    assert_eq!(body, b"<html>main</html>");
}
