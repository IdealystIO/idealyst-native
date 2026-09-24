//! `idealyst dev`'s page stream, served same-origin by the app's own
//! server.
//!
//! A full-stack page is served by the project's server, while the stream
//! that drives its livereload, overlay patches, hot patches and build
//! badge (`/__idealyst/reload`, plus `/__idealyst/events` and the page's
//! `/__idealyst/ack`) is served by the `idealyst dev` process on a port
//! of its own. Reaching that port directly works on a laptop and fails
//! everywhere a port has to be forwarded to be reachable — a devcontainer
//! forwards the app's port and nothing else, so the page's `EventSource`
//! retried a loopback port the browser could not see, forever, and the
//! author saw no overlay and no patches at all.
//!
//! So when the dev loop starts the server it names the stream in
//! [`ENV`] (`IDEALYST_DEV_STREAM=http://127.0.0.1:<port>`), and
//! [`crate::router`] then proxies the `/__idealyst/*` paths to it: the
//! page reaches the stream on the origin it was loaded from, which is
//! the one origin that is always forwarded. Without the variable — any
//! binary not started by `idealyst dev`, production included — there is
//! no route.
//!
//! The proxy is a byte pipe, deliberately: the stream sends its snapshot
//! and then live events on one long response, and every chunk is passed
//! on as it arrives (hyper flushes each body frame), so the page sees the
//! same stream in the same order as a direct connection would. The dev
//! loop probes [`PROBE_PATH`] after the server starts to learn which
//! route the page will use.

use std::net::SocketAddr;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use http_body_util::BodyExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Set by `idealyst dev` in the environment of the server it spawns: the
/// dev stream's origin, `http://127.0.0.1:<port>`.
pub const ENV: &str = "IDEALYST_DEV_STREAM";

/// Answers `204` with [`HEADER`] when the routes are installed — how the
/// dev loop tells a server that proxies the stream from one that does
/// not (whose fallback would hand out `index.html` with a `200`).
pub const PROBE_PATH: &str = "/__idealyst/stream";

/// Carried by every response the proxy gives, its value the proxy's
/// protocol version.
pub const HEADER: &str = "x-idealyst-dev-stream";

const VERSION: &str = "1";

/// The paths proxied to the dev stream.
const PROXIED: [&str; 3] = ["/__idealyst/reload", "/__idealyst/events", "/__idealyst/ack"];

/// Largest request body passed on (the page's ack is a few dozen bytes).
const MAX_BODY: usize = 64 * 1024;

/// Largest response head read from the stream.
const MAX_HEAD: usize = 16 * 1024;

/// `router` with the dev stream's routes, when [`ENV`] names one.
pub(crate) fn from_env(router: Router) -> Router {
    match upstream(std::env::var(ENV).ok().as_deref()) {
        Some(addr) => routes(router, addr),
        None => router,
    }
}

/// The stream's address from [`ENV`]'s value: `http://host:port`, with
/// an optional trailing slash. Anything else is ignored, with a line
/// saying so — a misspelt value should not silently cost the page its
/// stream.
fn upstream(value: Option<&str>) -> Option<SocketAddr> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let parsed = value
        .strip_prefix("http://")
        .map(|rest| rest.trim_end_matches('/'))
        .and_then(|hostport| {
            let hostport = hostport.replace("localhost", "127.0.0.1");
            hostport.parse::<SocketAddr>().ok()
        });
    if parsed.is_none() {
        eprintln!("[server] {ENV}={value:?} is not http://<ip>:<port>; the dev stream is not proxied");
    }
    parsed
}

/// `router` with [`PROBE_PATH`] and the proxied paths, forwarding to
/// `upstream`. Public for tests; servers get it through [`crate::router`].
pub fn routes(router: Router, upstream: SocketAddr) -> Router {
    let mut router = router.route(
        PROBE_PATH,
        get(|| async { (StatusCode::NO_CONTENT, [(HEADER, VERSION)]).into_response() }),
    );
    for path in PROXIED {
        let handler = move |request: Request| proxy(upstream, request);
        router = router.route(path, get(handler).post(handler));
    }
    router
}

async fn proxy(upstream: SocketAddr, request: Request) -> Response {
    match forward(upstream, request).await {
        Ok(response) => response,
        // The dev session is gone or restarting. The page's EventSource
        // retries on its own.
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            [(HEADER, VERSION)],
            format!("idealyst dev stream unreachable at {upstream}: {e}"),
        )
            .into_response(),
    }
}

async fn forward(upstream: SocketAddr, request: Request) -> std::io::Result<Response> {
    let (parts, body) = request.into_parts();
    let body = body
        .collect()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .to_bytes();
    if body.len() > MAX_BODY {
        return Ok((StatusCode::PAYLOAD_TOO_LARGE, [(HEADER, VERSION)]).into_response());
    }
    let target = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: {upstream}\r\nConnection: close\r\nContent-Length: {}\r\n",
        parts.method,
        body.len()
    );
    for name in ["content-type", "last-event-id", "accept"] {
        if let Some(v) = parts.headers.get(name).and_then(|v| v.to_str().ok()) {
            head.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    head.push_str("\r\n");

    let mut stream = TcpStream::connect(upstream).await?;
    stream.set_nodelay(true)?;
    stream.write_all(head.as_bytes()).await?;
    if parts.method == Method::POST || !body.is_empty() {
        stream.write_all(&body).await?;
    }

    // The response head, and whatever of the body came with it.
    let mut buf = Vec::with_capacity(1024);
    let end = loop {
        let mut chunk = [0u8; 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the stream closed before its response head",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(std::io::Error::other("response head too large"));
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let rest = Bytes::copy_from_slice(&buf[end + 4..]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .and_then(|c| StatusCode::from_u16(c).ok())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status).header(HEADER, VERSION);
    let mut chunked = false;
    let mut length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "content-length" => length = value.parse().ok(),
            // Hop-by-hop, or restated by hyper for this response.
            "connection" | "keep-alive" => {}
            _ => {
                if let Ok(v) = HeaderValue::from_str(value) {
                    builder = builder.header(name, v);
                }
            }
        }
    }
    // Never buffered on the way out: a reverse proxy in front of the app
    // server (nginx) holds an unmarked event stream until it fills.
    builder = builder.header("x-accel-buffering", "no");

    let body = if chunked {
        // Not how the stream answers (its responses are `Connection:
        // close` bodies), but a finite chunked body is still passed on
        // correctly rather than with its framing inside it.
        let mut all = rest.to_vec();
        stream.read_to_end(&mut all).await?;
        Body::from(dechunk(&all))
    } else if let Some(n) = length {
        let mut all = rest.to_vec();
        while all.len() < n {
            let mut chunk = vec![0u8; (n - all.len()).min(8192)];
            let got = stream.read(&mut chunk).await?;
            if got == 0 {
                break;
            }
            all.extend_from_slice(&chunk[..got]);
        }
        all.truncate(n);
        Body::from(all)
    } else {
        // The event streams: a body that ends when the connection does.
        // Each read becomes one body frame, sent as it arrives.
        let first = (!rest.is_empty()).then_some(rest);
        let stream = futures_util::stream::unfold(
            (stream, first),
            |(mut stream, pending)| async move {
                if let Some(bytes) = pending {
                    return Some((Ok::<Bytes, std::io::Error>(bytes), (stream, None)));
                }
                let mut chunk = vec![0u8; 8192];
                match stream.read(&mut chunk).await {
                    Ok(0) => None,
                    Ok(n) => {
                        chunk.truncate(n);
                        Some((Ok(Bytes::from(chunk)), (stream, None)))
                    }
                    Err(e) => Some((Err(e), (stream, None))),
                }
            },
        );
        Body::from_stream(stream)
    };
    builder.body(body).map_err(|e| std::io::Error::other(e.to_string()))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A complete chunked body's payload.
fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = find(raw, b"\r\n") {
        let size_text = String::from_utf8_lossy(&raw[..eol]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .unwrap_or(0);
        raw = &raw[eol + 2..];
        if size == 0 || raw.len() < size {
            break;
        }
        out.extend_from_slice(&raw[..size]);
        raw = raw.get(size + 2..).unwrap_or(&[]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_names_an_http_origin_or_nothing() {
        assert_eq!(upstream(None), None);
        assert_eq!(upstream(Some("")), None);
        assert_eq!(
            upstream(Some("http://127.0.0.1:4777")),
            Some("127.0.0.1:4777".parse().unwrap())
        );
        assert_eq!(
            upstream(Some("http://localhost:4777/")),
            Some("127.0.0.1:4777".parse().unwrap())
        );
        assert_eq!(upstream(Some("https://127.0.0.1:4777")), None, "the dev stream is plain http");
        assert_eq!(upstream(Some("127.0.0.1:4777")), None);
    }

    #[test]
    fn a_chunked_body_is_unframed() {
        assert_eq!(dechunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"), b"hello world");
        assert_eq!(dechunk(b"0\r\n\r\n"), b"");
    }
}
