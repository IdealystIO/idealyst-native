//! Browser-driven assertions. For each app: serve `dist/web/` over a
//! local ephemeral port (chrome won't ES-module-import over `file://`),
//! drive a headless Chrome subprocess with `--dump-dom`, then verify
//! the marker text appears in the rendered DOM and stderr is free of
//! `RuntimeError` / `Uncaught` / `ERROR:CONSOLE` lines.
//!
//! Why subprocess and not a CDP-driven crate? `headless_chrome` wedged
//! its pipe transport after the first `evaluate` round-trip on this
//! workload — Chrome would launch, navigate, return one body snapshot,
//! then go silent. `--dump-dom` is a single short-lived subprocess
//! call with no live connection to keep alive, which has been bullet-
//! proof across the runs we've made it withstand here.
//!
//! A clean run = marker appeared in the dumped DOM + Chrome stderr is
//! free of the runtime-error patterns we look for.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use crate::AppCfg;

const CHROME_BIN_MAC: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Substrings that, if seen in Chrome's stderr, indicate the page hit
/// a real JS-level problem during boot. Chrome routes uncaught
/// exceptions and `console.error` calls through stderr when
/// `--enable-logging=stderr --v=1` is set.
const ERROR_NEEDLES: &[&str] = &[
    "Uncaught",
    "RuntimeError",
    "ERROR:CONSOLE",
    // wasm-bindgen surfaces wbg-side init failures here.
    "wbg",
];

pub fn run_browser_check(dist_web: &Path, app: &AppCfg) -> Result<(), String> {
    let chrome_bin = resolve_chrome_bin()?;

    // ---- static server ---------------------------------------------------
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| format!("static-serve bind failed: {e}"))?;
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(sa) => sa.port(),
        other => return Err(format!("unexpected listen addr: {other:?}")),
    };
    let url = format!("http://127.0.0.1:{port}/");

    let dist = dist_web.to_path_buf();
    let server = Arc::new(server);
    let server_thread = Arc::clone(&server);
    let server_handle = thread::spawn(move || serve_loop(server_thread, dist));

    // ---- chrome subprocess ----------------------------------------------
    eprintln!("    [browser] chrome --headless --dump-dom {url}");
    // `--virtual-time-budget=N` tells headless Chrome to advance fake
    // time so timers + microtasks resolve as fast as possible up to N
    // ms of synthesized time, then dump. With wasm-fetch + instantiate
    // counted in real wall time too, give it 2× the per-app marker
    // budget plus a 3s floor.
    let budget_ms = (app.marker_wait_ms.saturating_mul(2)).max(3_000);
    let chrome = Command::new(&chrome_bin)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-dev-shm-usage",
            "--enable-logging=stderr",
            "--v=1",
            &format!("--virtual-time-budget={budget_ms}"),
            "--dump-dom",
            &url,
        ])
        .output();

    // Stop the static server. tiny_http's `incoming_requests()`
    // iterator only terminates when ALL `Arc<Server>` clones drop —
    // dropping our local handle alone leaves the thread blocked
    // forever. `Server::unblock()` is the supported way to break the
    // iterator without dropping references.
    server.unblock();
    let _ = server_handle.join();
    drop(server);

    let chrome = chrome.map_err(|e| format!("spawn chrome: {e}"))?;
    if !chrome.status.success() {
        return Err(format!(
            "chrome exited with {} (stderr {} bytes)",
            chrome.status,
            chrome.stderr.len()
        ));
    }

    let dom = String::from_utf8_lossy(&chrome.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&chrome.stderr).into_owned();

    // Strip HTML tags so a marker like `greet: hello hola bonjour`
    // matches even if Typography splits it across spans.
    let text = strip_html(&dom);

    let mut problems: Vec<String> = Vec::new();
    if !text.contains(app.expected_marker) {
        let preview: String = text.chars().take(400).collect();
        problems.push(format!(
            "marker `{}` not found in DOM (text preview: {preview:?})",
            app.expected_marker
        ));
    }

    let bad_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| ERROR_NEEDLES.iter().any(|n| l.contains(n)))
        // Chrome's logging itself contains "Uncaught" in benign banner
        // text on first run; we narrow to lines that look like actual
        // page-emitted errors (have a level marker like ERROR:/WARNING:
        // OR mention CONSOLE/RuntimeError).
        .filter(|l| {
            l.contains("RuntimeError")
                || l.contains("ERROR:CONSOLE")
                || (l.contains("Uncaught") && (l.contains("at ") || l.contains("wasm")))
        })
        .collect();

    if !bad_lines.is_empty() {
        let joined = bad_lines
            .iter()
            .map(|l| l.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n      - ");
        problems.push(format!(
            "{} runtime-error line(s) in chrome stderr:\n      - {}",
            bad_lines.len(),
            joined,
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n    "))
    }
}

fn resolve_chrome_bin() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("CHROME") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    let mac = PathBuf::from(CHROME_BIN_MAC);
    if mac.exists() {
        return Ok(mac);
    }
    // Fall through: try `chrome` on PATH.
    for name in ["google-chrome", "chrome", "chromium"] {
        if let Ok(out) = Command::new("which").arg(name).output() {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(PathBuf::from(path));
                }
            }
        }
    }
    Err("no Chrome/Chromium found (set $CHROME or install Google Chrome)".to_string())
}

/// Naive HTML tag stripper — collapses all `<…>` to spaces and
/// normalizes whitespace runs to single spaces. Good enough for
/// "does this substring appear in the rendered text?" assertions.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if in_tag => {
                let _ = c;
            }
            c => out.push(c),
        }
    }
    // Collapse whitespace runs so a tag-split marker like
    // `<span>greet: </span><span>hello…</span>` becomes one line with
    // a single space at the join.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_space = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(ch);
            prev_space = false;
        }
    }
    collapsed
}

fn serve_loop(server: Arc<tiny_http::Server>, root: PathBuf) {
    for req in server.incoming_requests() {
        let mut path = req.url().trim_start_matches('/');
        if path.is_empty() {
            path = "index.html";
        }
        let path = path.split('?').next().unwrap_or(path);
        let path = path.split('#').next().unwrap_or(path);

        let file_path = root.join(path);
        match std::fs::read(&file_path) {
            Ok(bytes) => {
                let mime = guess_mime(&file_path);
                let header = tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    mime.as_bytes(),
                )
                .expect("header");
                let _ = req.respond(
                    tiny_http::Response::from_data(bytes).with_header(header),
                );
            }
            Err(_) => {
                let _ = req.respond(tiny_http::Response::empty(404));
            }
        }
    }
}

fn guess_mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}
