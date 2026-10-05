//! Fetching a bundle from `stream-serve`.
//!
//! A deliberately minimal HTTP/1.1 GET over `std::net`: the spike talks
//! only to its own server on localhost, and a real HTTP client would be
//! the platform's (`URLSession`, `fetch`, OkHttp) behind a host capability
//! anyway — not something to pick in a spike.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Where the demo looks for the bridged RemoteCounter bundle by default.
pub const DEFAULT_REMOTE_URL: &str = "http://127.0.0.1:7878/remote.wasm";

/// A served bundle.
pub struct Fetched {
    pub wasm: Vec<u8>,
    /// The server's build counter (`X-Bundle-Version`).
    pub version: String,
}

/// GET `url` (`http://host:port/path` only). Errors carry the server's
/// message — on a failed build, the compiler output.
pub fn fetch(url: &str) -> Result<Fetched, String> {
    let rest = url.strip_prefix("http://").ok_or("only http:// URLs are supported")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = std::net::ToSocketAddrs::to_socket_addrs(authority)
        .map_err(|e| format!("bad address {authority}: {e}"))?
        .next()
        .ok_or_else(|| format!("{authority} resolves to nothing"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))
        .map_err(|e| format!("bundle server unreachable at {authority}: {e}"))?;
    // Generous: the server may be rebuilding the bundle before answering.
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;

    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("malformed response")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = raw[split + 4..].to_vec();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("malformed status line")?;
    if status != 200 {
        return Err(format!("server answered {status}: {}", String::from_utf8_lossy(&body)));
    }
    let version = head
        .lines()
        .find_map(|l| l.strip_prefix("X-Bundle-Version: "))
        .unwrap_or("?")
        .to_string();
    Ok(Fetched { wasm: body, version })
}
