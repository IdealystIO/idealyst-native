//! End-to-end test for the livereload SSE stream.
//!
//! Boots `serve_static` against a temporary directory, opens a raw
//! TCP connection to the SSE endpoint, parses the initial event,
//! triggers a bump via [`ReloadSignal::bump`], and verifies that a
//! second `data: <gen>\n\n` frame arrives.
//!
//! Uses raw `TcpStream` rather than a real HTTP client so the test
//! has no extra dependencies — the SSE protocol over chunked transfer
//! encoding is simple enough to parse inline.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use dev_http::{serve_static, ReloadContext, RELOAD_SSE_URL};
use dev_reload::ReloadSignal;

/// Bind to an ephemeral port, then immediately drop the listener so
/// `serve_static` can rebind it. There's a tiny TOCTOU window here
/// — another process could snipe the port — but in practice it's
/// fine for tests, and the alternative (parsing `serve_static`'s
/// log line) is way more fragile.
fn pick_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn read_until(stream: &mut TcpStream, needle: &[u8], deadline: Instant) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while Instant::now() < deadline {
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(needle.len()).any(|w| w == needle) {
                    return buf;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => panic!("read failed: {e}"),
        }
    }
    panic!(
        "deadline exceeded waiting for {:?}, got: {:?}",
        std::str::from_utf8(needle),
        String::from_utf8_lossy(&buf)
    );
}

#[test]
fn sse_stream_pushes_initial_and_bumped_events() {
    let port = pick_port();
    let signal = ReloadSignal::new();
    let signal_for_server = signal.clone();

    // Empty serve-root: requests for assets will 404, but the SSE
    // endpoint short-circuits before file resolution so it doesn't
    // matter. Use the current dir as a guaranteed-canonicalizable
    // root — `serve_static` canonicalizes the root at startup.
    let root = std::env::current_dir().unwrap();

    let _server = thread::spawn(move || {
        let ctx = ReloadContext {
            signal: signal_for_server,
        };
        // Will block forever; the test thread exits and tears down
        // the process when done. tiny_http doesn't expose a clean
        // shutdown handle.
        let _ = serve_static("127.0.0.1", port, &root, Some(ctx), None, None);
    });

    // Wait for the server to bind. tiny_http binds synchronously
    // inside `Server::http`, but the spawn means the thread may not
    // have started yet.
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < connect_deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("failed to connect after 5s: {e}"),
        }
    };

    // Issue the SSE request. HTTP/1.1, no body.
    let req = format!(
        "GET {RELOAD_SSE_URL} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: text/event-stream\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();

    // The server writes the response by hand (Connection: close, no
    // chunking — see `serve_sse` in dev-http) so the body is raw
    // `data: <gen>\n\n` text. Initial gen is 0 since the test
    // hasn't bumped yet.
    let deadline = Instant::now() + Duration::from_secs(5);
    let buf = read_until(&mut stream, b"data: 0\n\n", deadline);

    // Sanity-check the response headers we care about.
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&buf[..head_end]).unwrap();
    assert!(
        head.contains("Content-Type: text/event-stream"),
        "missing event-stream content type in:\n{head}"
    );
    assert!(
        head.to_lowercase().contains("connection: close"),
        "expected Connection: close (we hand-roll the response head; see serve_sse) in:\n{head}"
    );

    // Now bump and read again. The deadline restarts because the
    // server is now in the middle of waiting on the next event.
    let new = signal.bump();
    assert_eq!(new, 1);

    let deadline = Instant::now() + Duration::from_secs(5);
    let _ = read_until(&mut stream, b"data: 1\n\n", deadline);
}

#[test]
fn sse_stream_serves_disabled_state_without_blocking_other_requests() {
    // Regression coverage for the contract: even with reload
    // disabled, the SSE endpoint must respond (with `data: 0`) so
    // the inline `EventSource` doesn't error-loop the page.
    let port = pick_port();
    let root = std::env::current_dir().unwrap();
    let _server = thread::spawn(move || {
        let _ = serve_static("127.0.0.1", port, &root, None, None, None);
    });

    let connect_deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < connect_deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("failed to connect after 5s: {e}"),
        }
    };
    let req = format!(
        "GET {RELOAD_SSE_URL} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: text/event-stream\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let _ = read_until(&mut stream, b"data: 0\n\n", deadline);

    // Second connection must succeed in parallel — confirms the SSE
    // handler is on its own thread and not blocking the accept loop.
    let mut other = TcpStream::connect(("127.0.0.1", port))
        .expect("second connection blocked by first SSE stream");
    other
        .write_all(
            format!(
                "GET /does-not-exist HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 \r\n"
            )
            .as_bytes(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let buf = read_until(&mut other, b"\r\n\r\n", deadline);
    let head = std::str::from_utf8(&buf).unwrap();
    assert!(
        head.starts_with("HTTP/1.1 404"),
        "expected 404 for unknown path while SSE held open; got:\n{head}"
    );
}

#[allow(dead_code)]
fn touch_path(_p: &Path) {}
