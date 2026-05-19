//! Tiny_http loop for the fiddle. Hand-rolled router covering four
//! route families:
//!
//! - `GET /`             → `webapp/index.html` (the editor UI shell)
//! - `GET /pkg/*`        → the webapp's wasm-pack output
//! - `POST /compile`     → JSON in `{ "source": "..." }`, JSON out
//!                          `{ "hash": "..." }` or `{ "error": "..." }`
//! - `GET /compiled/*`   → per-compile bundle from
//!                          `examples/fiddle/compiled/<hash>/...`
//!
//! Each request spawns a worker thread so a slow `/compile` doesn't
//! block static-file serving. Compilation itself is serialized by
//! [`crate::compile::compile`]'s internal lock.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::compile;

#[derive(Deserialize)]
struct CompileReq {
    source: String,
    /// Output mode: `"simulator"` or `"web"`. Defaults to simulator
    /// when omitted so older clients (and curl-driven sanity checks)
    /// keep working without specifying it.
    #[serde(default)]
    mode: compile::Mode,
}

pub fn run(host: &str, port: u16) -> Result<()> {
    let root = compile::fiddle_root();
    let addr = format!("{host}:{port}");
    let server = Arc::new(
        Server::http(&addr).map_err(|e| anyhow::anyhow!("tiny_http bind {addr}: {e}"))?,
    );
    eprintln!("[fiddle] serving on http://{addr}");
    eprintln!(
        "[fiddle] open http://127.0.0.1:{port} in a browser; \
         POST source to /compile to rebuild the simulator iframe"
    );

    // Worker-thread-per-request keeps slow compiles off the
    // accept loop. tiny_http's `Request` is `Send`, so handing it
    // off is straightforward.
    for request in server.incoming_requests() {
        let root = root.clone();
        thread::spawn(move || {
            if let Err(e) = dispatch(request, &root) {
                eprintln!("[fiddle] request error: {e:#}");
            }
        });
    }
    Ok(())
}

fn dispatch(request: Request, root: &Path) -> Result<()> {
    let url = request.url().to_string();
    let method = request.method().clone();

    // Strip the query string for path matching but keep the raw
    // url available for logs.
    let path = url.split('?').next().unwrap_or(&url).to_string();

    match (&method, path.as_str()) {
        (Method::Get, "/") => serve_file(request, &root.join("webapp/index.html")),
        (Method::Get, p) if p.starts_with("/pkg/") => {
            serve_static_under(request, root.join("webapp"), p)
        }
        (Method::Get, p) if p.starts_with("/compiled/") => {
            serve_static_under(request, root.into(), p)
        }
        (Method::Post, "/compile") => handle_compile(request, root),
        (Method::Post, "/clear-cache") => handle_clear_cache(request, root),
        _ => respond(request, 404, "not found"),
    }
}

/// `POST /clear-cache` — `rm -rf compiled/`. Returns `{ "cleared":
/// N }` where N is the number of cache entries dropped. Manual
/// escape valve for "I edited an upstream crate and don't want to
/// wait on the mtime detection to settle" or just nuking from a
/// browser button without dropping to a terminal.
fn handle_clear_cache(request: Request, root: &Path) -> Result<()> {
    let dir = root.join("compiled");
    let count = match fs::read_dir(&dir) {
        Ok(entries) => entries.flatten().count(),
        Err(_) => 0,
    };
    let _ = fs::remove_dir_all(&dir);
    let body = serde_json::to_string(&serde_json::json!({ "cleared": count }))?;
    respond_json(request, 200, &body)
}

fn handle_compile(mut request: Request, root: &Path) -> Result<()> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .context("reading /compile body")?;
    let req: CompileReq = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return respond(
                request,
                400,
                &format!("{{\"error\":\"bad request body: {e}\"}}"),
            );
        }
    };

    match compile::compile(&req.source, req.mode, root) {
        Ok(ok) => {
            let body = serde_json::to_string(&serde_json::json!({ "hash": ok.hash }))?;
            respond_json(request, 200, &body)
        }
        Err(e) => {
            let body = serde_json::to_string(&serde_json::json!({
                "error": format!("{e:#}")
            }))?;
            respond_json(request, 500, &body)
        }
    }
}

/// Serve a file from disk by absolute path. Used for the editor
/// shell entry — every other static path goes through
/// [`serve_static_under`] for path-traversal safety.
fn serve_file(request: Request, abs: &Path) -> Result<()> {
    let bytes = match fs::read(abs) {
        Ok(b) => b,
        Err(_) => return respond(request, 404, "not found"),
    };
    let mime = mime_for(abs);
    // Files under `compiled/` are content-addressed by source hash,
    // *but* the dev loop can re-build under the same hash when a
    // workspace crate (render-wgpu, host-web, …) changes. The
    // browser would otherwise keep serving its previously-cached
    // snippet wasm for that URL and the user sees yesterday's bug.
    // `no-store` keeps the iframe wasm-refresh path honest.
    // Editor-shell + /pkg paths still get normal caching.
    let is_snippet = abs
        .components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("compiled"));
    let cache_header = if is_snippet { "no-store" } else { "public, max-age=0" };
    let response = Response::from_data(bytes)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
                .map_err(|_| anyhow::anyhow!("bad header"))?,
        )
        .with_header(
            Header::from_bytes(&b"Cache-Control"[..], cache_header.as_bytes())
                .map_err(|_| anyhow::anyhow!("bad header"))?,
        );
    request.respond(response).context("respond")?;
    Ok(())
}

/// Serve `<base>/<rest>` where `rest` is the request path minus its
/// leading `/`. Rejects any path that escapes `base` after
/// canonicalization — the only way `..` could leak through tiny_http
/// is via a manually-crafted URL, but the defense is cheap.
///
/// Trailing-slash + directory paths are resolved to `index.html`
/// inside the directory (browser convention; an iframe whose `src`
/// is `/compiled/<hash>/?t=...` lands here).
fn serve_static_under(request: Request, base: PathBuf, url_path: &str) -> Result<()> {
    let rel = url_path.trim_start_matches('/');
    let abs = base.join(rel);
    let canon = match abs.canonicalize() {
        Ok(p) => p,
        Err(_) => return respond(request, 404, "not found"),
    };
    let base_canon = match base.canonicalize() {
        Ok(p) => p,
        // Base dir doesn't exist yet (no compiles cached, no
        // wasm-pack output) — fall through to 404 with a hint.
        Err(_) => {
            return respond(
                request,
                404,
                "not found (base dir missing — did you `wasm-pack build webapp/`?)",
            );
        }
    };
    if !canon.starts_with(&base_canon) {
        return respond(request, 403, "forbidden");
    }
    // Directory → serve `index.html`. Without this an iframe URL
    // like `/compiled/<hash>/` 404s — the canonicalize succeeds
    // (the dir exists), but `fs::read` against a dir returns
    // `EISDIR`, which serve_file maps to 404 with no hint.
    let target = if canon.is_dir() {
        canon.join("index.html")
    } else {
        canon
    };
    serve_file(request, &target)
}

fn respond(request: Request, status: u16, body: &str) -> Result<()> {
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..])
                .map_err(|_| anyhow::anyhow!("bad header"))?,
        );
    request.respond(response).context("respond")?;
    Ok(())
}

fn respond_json(request: Request, status: u16, body: &str) -> Result<()> {
    let response = Response::from_data(body.as_bytes().to_vec())
        .with_status_code(status)
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                .map_err(|_| anyhow::anyhow!("bad header"))?,
        );
    request.respond(response).context("respond")?;
    Ok(())
}

/// Minimal MIME mapping. The fiddle serves wasm + JS + HTML + CSS;
/// anything else falls back to `application/octet-stream` and lets
/// the browser sniff (or fail loudly).
fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

