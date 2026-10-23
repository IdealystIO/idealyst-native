//! The fiddle's HTTP routes, on a sync `tiny_http` loop with a thread per
//! request so a build in progress never blocks serving bundles:
//!
//! - `GET  /health`          → `{ "ok": true }`, for a client to probe
//! - `POST /compile`         → [`crate::compile::Outcome`] as JSON
//! - `POST /clear-cache`     → empties `compiled/`
//! - `GET  /compiled/<hash>/…` → a built bundle
//! - `GET  /`, `/pkg/…`      → the standalone editor (`webapp/dist/web`)
//!
//! Every response carries `Access-Control-Allow-Origin: *` and `OPTIONS`
//! answers the preflight, so a page on another origin can call
//! `/compile` and frame `/compiled/…`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::compile::Compiler;

#[derive(Deserialize)]
struct CompileRequest {
    /// Submitted path (`app.rs`, `widgets.rs`) → contents.
    files: BTreeMap<String, String>,
}

pub fn run(addr: &str, root: PathBuf, compiler: Compiler) -> Result<()> {
    let server = Server::http(addr).map_err(|e| anyhow::anyhow!("binding {addr}: {e}"))?;
    eprintln!("[fiddle] serving on http://{addr}");
    let state = Arc::new((root, compiler));
    for request in server.incoming_requests() {
        let state = state.clone();
        thread::spawn(move || {
            let (root, compiler) = &*state;
            if let Err(e) = dispatch(request, root, compiler) {
                eprintln!("[fiddle] request failed: {e:#}");
            }
        });
    }
    Ok(())
}

fn dispatch(request: Request, root: &Path, compiler: &Compiler) -> Result<()> {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or(&url).to_string();
    match (request.method().clone(), path.as_str()) {
        (Method::Options, _) => respond(request, 204, Vec::new(), "text/plain", None),
        (Method::Get, "/health") => respond_json(request, 200, &serde_json::json!({ "ok": true })),
        (Method::Post, "/compile") => handle_compile(request, compiler),
        (Method::Post, "/clear-cache") => {
            let dir = compiler.compiled_dir();
            let cleared = fs::read_dir(&dir).map(|e| e.count()).unwrap_or(0);
            let _ = fs::remove_dir_all(&dir);
            respond_json(request, 200, &serde_json::json!({ "cleared": cleared }))
        }
        (Method::Get, p) if p.starts_with("/compiled/") => {
            // Content-addressed: a bundle's bytes never change under its URL.
            serve_under(request, &compiler.compiled_dir(), &p["/compiled/".len()..], Some("public, max-age=31536000, immutable"))
        }
        (Method::Get, "/") => serve_under(request, &editor_dir(root), "index.html", Some("no-cache")),
        // Content-hashed by the build, like a bundle.
        (Method::Get, p) if p.starts_with("/pkg/") => {
            serve_under(request, &editor_dir(root), &p[1..], Some("public, max-age=31536000, immutable"))
        }
        _ => respond(request, 404, b"not found".to_vec(), "text/plain", None),
    }
}

/// The standalone editor's build: `idealyst build --web` in `webapp/`.
fn editor_dir(root: &Path) -> PathBuf {
    root.join("webapp/dist/web")
}

fn handle_compile(mut request: Request, compiler: &Compiler) -> Result<()> {
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body).context("reading the request body")?;
    let req: CompileRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return respond_json(request, 400, &serde_json::json!({ "error": format!("bad request body: {e}") }));
        }
    };
    match compiler.compile(&req.files) {
        Ok(outcome) => respond_json(request, 200, &serde_json::to_value(&outcome)?),
        // A refused submission (no `app.rs`, a bad path) or a server-side
        // failure to run the build at all.
        Err(e) => respond_json(request, 400, &serde_json::json!({ "error": format!("{e:#}") })),
    }
}

/// Serve `<base>/<rel>`, refusing anything that resolves outside `base`.
/// A directory serves its `index.html`, which is how `/compiled/<hash>/`
/// loads.
fn serve_under(request: Request, base: &Path, rel: &str, cache: Option<&str>) -> Result<()> {
    let not_found = |request: Request| respond(request, 404, b"not found".to_vec(), "text/plain", None);
    let (Ok(base), Ok(mut target)) = (base.canonicalize(), base.join(rel).canonicalize()) else {
        return not_found(request);
    };
    if !target.starts_with(&base) {
        return respond(request, 403, b"forbidden".to_vec(), "text/plain", None);
    }
    if target.is_dir() {
        target = target.join("index.html");
    }
    match fs::read(&target) {
        Ok(bytes) => {
            let cache = if target.extension().is_some_and(|e| e == "html") { Some("no-cache") } else { cache };
            respond(request, 200, bytes, mime_for(&target), cache)
        }
        Err(_) => not_found(request),
    }
}

fn respond_json(request: Request, status: u16, body: &serde_json::Value) -> Result<()> {
    respond(request, status, body.to_string().into_bytes(), "application/json; charset=utf-8", Some("no-store"))
}

fn respond(request: Request, status: u16, body: Vec<u8>, mime: &str, cache: Option<&str>) -> Result<()> {
    let mut response = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", mime))
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Access-Control-Allow-Headers", "content-type"));
    if let Some(cache) = cache {
        response = response.with_header(header("Cache-Control", cache));
    }
    request.respond(response).context("writing the response")
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header names and values are valid")
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}
