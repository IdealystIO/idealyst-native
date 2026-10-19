//! `screenshot` over the Chrome DevTools Protocol, for a tab in a browser
//! the dev session launched.
//!
//! A page can only rasterize itself by copying its DOM into an SVG
//! `<foreignObject>` (`backend-web`'s `robot_screenshot`), which drops
//! canvas pixels, plugin content (a PDF viewer) and some font rendering.
//! A browser the CLI launched with `--remote-debugging-port=0` (the
//! headless web client) also answers CDP, and `Page.captureScreenshot`
//! returns the compositor's own pixels. The relay is on the host, so it
//! can reach that endpoint where the page cannot.
//!
//! ## Finding the tab
//!
//! The relay knows the app by its WebSocket session, not by tab. Each page
//! puts a random nonce in its `hello` and on `window.__idealyst_robot_nonce`
//! ([`NONCE_GLOBAL`]); the relay evaluates that global in every page target
//! of every browser it can reach and captures the one that matches. Two
//! tabs of the same URL stay apart, and a tab that is not the driven app is
//! never captured.
//!
//! ## Finding the browser
//!
//! Chromium given `--remote-debugging-port=0` picks a port and writes it to
//! `<user-data-dir>/DevToolsActivePort` (first line: the port). The relay
//! scans the immediate subdirectories of one directory for that file —
//! the CLI points it at `<project>/.idealyst`, where every headless
//! profile lives. A file left by a browser that is gone points at a port
//! nobody answers, and is skipped.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tungstenite::Message;

/// The page global holding its pairing nonce. `backend-web`'s
/// `robot_transport::ROBOT_NONCE_GLOBAL` writes it; a test there holds the
/// two names together.
pub const NONCE_GLOBAL: &str = "__idealyst_robot_nonce";

/// Per-socket I/O bound. A stale `DevToolsActivePort` can name a port some
/// other process now holds; this keeps one such endpoint from stalling the
/// screenshot.
const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// A captured frame: PNG bytes and its pixel size.
pub struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The DevTools HTTP ports of the browsers whose profiles sit directly
/// under `dir`, in directory order.
pub fn browser_ports(dir: &Path) -> Vec<u16> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut profiles: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    profiles.sort();
    profiles
        .iter()
        .filter_map(|p| std::fs::read_to_string(p.join("DevToolsActivePort")).ok())
        .filter_map(|text| text.lines().next()?.trim().parse().ok())
        .collect()
}

/// Capture the page target whose [`NONCE_GLOBAL`] is `nonce`, searching the
/// browsers on `ports`. `Err` says what was searched, for the agent to
/// read.
pub fn capture(ports: &[u16], nonce: &str) -> Result<Shot, String> {
    if ports.is_empty() {
        return Err("no browser launched by this dev session answers CDP".into());
    }
    let mut searched = 0;
    for &port in ports {
        let Ok(pages) = page_targets(port) else { continue };
        for ws_url in pages {
            searched += 1;
            let Ok(mut page) = Page::connect(&ws_url) else { continue };
            if matches!(page.nonce(), Ok(Some(n)) if n == nonce) {
                return page.screenshot();
            }
        }
    }
    Err(format!(
        "the driven tab is not in a browser this dev session launched ({searched} tab(s) searched)"
    ))
}

/// The `webSocketDebuggerUrl` of every `page` target on the browser at
/// `port` (`GET /json/list`).
fn page_targets(port: u16) -> Result<Vec<String>, String> {
    let body = http_get(port, "/json/list")?;
    let targets: Value = serde_json::from_slice(&body).map_err(|e| format!("/json/list: {e}"))?;
    Ok(targets
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["type"] == "page")
        .filter_map(|t| t["webSocketDebuggerUrl"].as_str().map(str::to_string))
        .collect())
}

/// A minimal HTTP/1.1 GET to loopback, read by its `Content-Length`: DevTools
/// keeps the connection open after the body despite `Connection: close`,
/// so reading to end-of-stream would wait out the timeout.
fn http_get(port: u16, path: &str) -> Result<Vec<u8>, String> {
    let mut stream = connect(port)?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("DevTools answered without an HTTP head".into());
        }
        raw.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(format!("DevTools {path}: {}", head.lines().next().unwrap_or("")));
    }
    let length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .ok_or_else(|| format!("DevTools {path}: no Content-Length"))?;
    let mut body = raw.split_off(head_end + 4);
    while body.len() < length {
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("DevTools {path}: body cut short"));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(body)
}

fn connect(port: u16) -> Result<TcpStream, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&addr, IO_TIMEOUT).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(|e| e.to_string())?;
    Ok(stream)
}

/// One page target's CDP socket.
struct Page {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Page {
    fn connect(ws_url: &str) -> Result<Page, String> {
        let port = ws_url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .and_then(|host| host.rsplit(':').next())
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| format!("not a loopback DevTools URL: {ws_url}"))?;
        // No `Origin` header: DevTools rejects a WebSocket whose origin it
        // was not told to allow (`--remote-allow-origins`), and tungstenite
        // sends none.
        let (ws, _) = tungstenite::client(ws_url, connect(port)?).map_err(|e| e.to_string())?;
        Ok(Page { ws, next_id: 1 })
    }

    /// Send one command and wait for its reply, skipping the events the
    /// target interleaves.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({ "id": id, "method": method, "params": params }).to_string();
        self.ws.send(Message::Text(frame.into())).map_err(|e| e.to_string())?;
        loop {
            let msg = self.ws.read().map_err(|e| format!("{method}: {e}"))?;
            let Message::Text(text) = msg else { continue };
            let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(format!("{method}: {}", err["message"].as_str().unwrap_or("error")));
            }
            return Ok(v["result"].clone());
        }
    }

    /// The page's pairing nonce, `None` for a page that has none (not an
    /// idealyst app, or one whose robot client has not started).
    fn nonce(&mut self) -> Result<Option<String>, String> {
        let result = self.call(
            "Runtime.evaluate",
            json!({ "expression": format!("globalThis.{NONCE_GLOBAL}"), "returnByValue": true }),
        )?;
        Ok(result["result"]["value"].as_str().map(str::to_string))
    }

    fn screenshot(&mut self) -> Result<Shot, String> {
        let result = self.call("Page.captureScreenshot", json!({ "format": "png" }))?;
        let data = result["data"].as_str().ok_or("Page.captureScreenshot returned no data")?;
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("Page.captureScreenshot: {e}"))?;
        let (width, height) = png_size(&png).ok_or("Page.captureScreenshot returned a non-PNG")?;
        Ok(Shot { png, width, height })
    }
}

/// Width and height from a PNG's IHDR chunk, which the format puts first.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Some((be(16), be(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_size_reads_the_ihdr_chunk() {
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&720u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1280, 720)));
        assert_eq!(png_size(b"GIF89a not a png at all......"), None);
    }

    #[test]
    fn browser_ports_reads_each_profiles_devtools_port_file() {
        let dir = tempdir("browser_ports");
        std::fs::create_dir_all(dir.join("headless-client-profile-3000")).unwrap();
        std::fs::write(dir.join("headless-client-profile-3000/DevToolsActivePort"), "41234\n/devtools/browser/x\n")
            .unwrap();
        // A profile with no browser running, and a plain file: neither is a port.
        std::fs::create_dir_all(dir.join("screenshots")).unwrap();
        std::fs::write(dir.join("robot.json"), "{}").unwrap();
        assert_eq!(browser_ports(&dir), vec![41234]);
        assert_eq!(browser_ports(&dir.join("missing")), Vec::<u16>::new());
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("robot_relay_cdp_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
