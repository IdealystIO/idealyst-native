//! Regression coverage for the synthesized fallback `index.html`.
//!
//! The bug: `idealyst dev --web` serves straight from the project root,
//! so a project that ships no `index.html` (e.g. a freshly-written
//! example) used to 404 at `/` — nothing to serve. `serve_static` now
//! accepts a `fallback_index` HTML string and serves it whenever the
//! root has no `index.html`, so dev works without hand-authored
//! boilerplate. A project's own `index.html` still wins, and real asset
//! requests for missing files must still 404 (not get the HTML shell).
//!
//! Raw TCP, same rationale as `overlay.rs` / `sse_reload.rs`: no extra
//! deps, stable across HTTP-client churn.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use dev_http::serve_static;

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
    // Accept: text/html so the SPA-fallback branch is exercised for
    // unknown paths too.
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: text/html\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();

    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response missing CRLF CRLF");
    let head = std::str::from_utf8(&buf[..head_end]).unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = buf[head_end + 4..].to_vec();
    (status, body)
}

fn spawn_server(root: &Path, fallback: Option<String>) -> u16 {
    let port = pick_port();
    let root = root.to_path_buf();
    thread::spawn(move || {
        let _ = serve_static(
            "127.0.0.1",
            port,
            &root,
            None,
            None,
            None,
            None,
            None,
            fallback,
        );
    });
    port
}

const SENTINEL: &str = "<!-- synthesized-fallback-index -->";

#[test]
fn serves_fallback_index_at_root_when_project_has_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    // No index.html written — exactly the index-less project shape.

    let fallback = format!("<!DOCTYPE html><html><head>{SENTINEL}</head><body></body></html>");
    let port = spawn_server(&root, Some(fallback));

    let (status, body) = http_get(port, "/");
    assert_eq!(status, 200, "GET / must serve the fallback, not 404");
    assert!(
        String::from_utf8_lossy(&body).contains(SENTINEL),
        "GET / body must be the synthesized fallback",
    );
}

#[test]
fn project_index_wins_over_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("index.html"), b"<html>real</html>").unwrap();

    let fallback = format!("<!DOCTYPE html><html><head>{SENTINEL}</head><body></body></html>");
    let port = spawn_server(&root, Some(fallback));

    let (status, body) = http_get(port, "/");
    assert_eq!(status, 200);
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("real"), "the project's own index.html must win");
    assert!(
        !body.contains(SENTINEL),
        "the fallback must not be served when a real index.html exists",
    );
}

#[test]
fn missing_asset_still_404s_with_fallback_present() {
    // A non-HTML asset request for a missing file must get a real 404 —
    // the fallback is for the HTML shell, never for JS/WASM/images.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();

    let fallback = format!("<!DOCTYPE html><html><head>{SENTINEL}</head><body></body></html>");
    let port = spawn_server(&root, Some(fallback));

    // No Accept: text/html → asset request → must 404, not serve HTML.
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < connect_deadline => thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("connect failed: {e}"),
        }
    };
    let req = format!(
        "GET /pkg/missing.wasm HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&buf[..head_end]).unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(status, 404, "missing asset must 404, not get the HTML shell");
}
