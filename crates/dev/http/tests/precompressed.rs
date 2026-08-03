//! `serve_static` precompressed-sidecar negotiation (`idealyst serve
//! --precompressed`).
//!
//! Release web builds stage `<file>.br` next to every compressible
//! bundle file (and hosts may add `.gz`). With `precompressed` on, a
//! request whose `Accept-Encoding` allows it gets the sidecar's bytes
//! with `Content-Encoding` + `Vary` and the ORIGINAL file's
//! Content-Type — the same contract as nginx `brotli_static` / Caddy
//! `precompressed`, so browser measurements match deployment. With it
//! off (every other `serve_static` consumer), sidecars are ignored
//! entirely.
//!
//! Raw TCP, same rationale as `overlay.rs` / `fallback_index.rs`: no
//! extra deps, stable across HTTP-client churn.

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

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// GET `path` with an optional `Accept-Encoding` header value.
fn http_get(port: u16, path: &str, accept_encoding: Option<&str>) -> Reply {
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
    let ae = accept_encoding
        .map(|v| format!("Accept-Encoding: {v}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         {ae}Connection: close\r\n\
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
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let body = buf[head_end + 4..].to_vec();
    Reply { status, headers, body }
}

fn spawn_server(root: &Path, precompressed: bool) -> u16 {
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
            None,
            precompressed,
        );
    });
    port
}

const ORIGINAL: &[u8] = b"(function(){ /* the real bundle bytes */ })()";
// Sidecar fixtures don't need real compression — the server's contract
// is byte-for-byte sidecar passthrough + headers, not decompression.
const BR_SIDECAR: &[u8] = b"\x00brotli-sidecar-bytes";
const GZ_SIDECAR: &[u8] = b"\x1f\x8bgzip-sidecar-bytes";

fn write_bundle(root: &Path) {
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/app.js"), ORIGINAL).unwrap();
    std::fs::write(root.join("pkg/app.js.br"), BR_SIDECAR).unwrap();
    std::fs::write(root.join("pkg/app.js.gz"), GZ_SIDECAR).unwrap();
    // A file with no sidecars at all.
    std::fs::write(root.join("pkg/plain.js"), ORIGINAL).unwrap();
}

#[test]
fn brotli_sidecar_wins_when_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    write_bundle(tmp.path());
    let port = spawn_server(tmp.path(), true);

    let r = http_get(port, "/pkg/app.js", Some("gzip, deflate, br, zstd"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, BR_SIDECAR, "brotli sidecar bytes must stream verbatim");
    assert_eq!(r.header("Content-Encoding"), Some("br"));
    assert_eq!(r.header("Vary"), Some("Accept-Encoding"));
    assert_eq!(
        r.header("Content-Type"),
        Some("application/javascript; charset=utf-8"),
        "sidecar keeps the ORIGINAL file's content type"
    );
}

#[test]
fn gzip_sidecar_serves_when_brotli_not_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    write_bundle(tmp.path());
    let port = spawn_server(tmp.path(), true);

    let r = http_get(port, "/pkg/app.js", Some("gzip"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, GZ_SIDECAR);
    assert_eq!(r.header("Content-Encoding"), Some("gzip"));

    // `br;q=0` is an explicit rejection — must fall to gzip too.
    let r = http_get(port, "/pkg/app.js", Some("br;q=0, gzip"));
    assert_eq!(r.body, GZ_SIDECAR);
    assert_eq!(r.header("Content-Encoding"), Some("gzip"));
}

#[test]
fn original_serves_without_accept_encoding_or_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    write_bundle(tmp.path());
    let port = spawn_server(tmp.path(), true);

    // No Accept-Encoding header at all → identity.
    let r = http_get(port, "/pkg/app.js", None);
    assert_eq!(r.body, ORIGINAL);
    assert_eq!(r.header("Content-Encoding"), None);

    // Encoding accepted but the file has no sidecars → identity.
    let r = http_get(port, "/pkg/plain.js", Some("br, gzip"));
    assert_eq!(r.body, ORIGINAL);
    assert_eq!(r.header("Content-Encoding"), None);
}

#[test]
fn flag_off_ignores_sidecars_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    write_bundle(tmp.path());
    let port = spawn_server(tmp.path(), false);

    let r = http_get(port, "/pkg/app.js", Some("br, gzip"));
    assert_eq!(
        r.body, ORIGINAL,
        "without --precompressed the original bytes serve even when \
         sidecars exist and the client accepts them"
    );
    assert_eq!(r.header("Content-Encoding"), None);
}

#[test]
fn sidecar_is_not_directly_addressable_conclusion_unchanged() {
    // Requesting the sidecar PATH still works like any static file —
    // it exists on disk; nothing hides it. This pins that the feature
    // only changes negotiated responses, not the file namespace.
    let tmp = tempfile::tempdir().unwrap();
    write_bundle(tmp.path());
    let port = spawn_server(tmp.path(), true);

    let r = http_get(port, "/pkg/app.js.br", None);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, BR_SIDECAR);
    // Direct fetch is NOT a negotiated response — no Content-Encoding.
    assert_eq!(r.header("Content-Encoding"), None);
}
