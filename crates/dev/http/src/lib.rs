//! Static-file HTTP server used by `idealyst dev`.
//!
//! Sync, single-threaded, intentionally minimal. Dev mode binds to
//! loopback or LAN so there's no TLS, no auth, no compression — those
//! belong in a CDN. What this serves:
//!
//! - Files under a configured root directory.
//! - `index.html` when the URL resolves to a directory.
//! - SPA fallback: a navigation request (Accept: text/html) for a
//!   missing path serves the root `index.html` so client-side routers
//!   keep working. Asset requests for missing files get a real 404.
//! - Correct `Content-Type` for the handful of extensions a typical
//!   idealyst app emits (HTML, JS, WASM, CSS, fonts, images).
//! - Optional livereload polling endpoint + HTML script injection
//!   when a [`ReloadContext`] is supplied. The contract: callers (the
//!   `dev-reload` crate, typically) hand us a shared generation
//!   counter and we expose it at [`RELOAD_GEN_URL`] for browsers to
//!   poll.
//!
//! Path-traversal safety: every resolved file path is canonicalized
//! and verified to live under the canonicalized root before being
//! served. Symlinks pointing outside the root are rejected.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tiny_http::{Header, Method, Request, Response, Server};

/// Wires the static server to a rebuild loop. When this is `Some`,
/// the server (a) answers the [`RELOAD_GEN_URL`] polling endpoint
/// with the current generation counter, and (b) injects a short
/// polling script into served HTML responses.
///
/// Producers (e.g. `dev-reload`) bump `gen` after every successful
/// rebuild; consumers (browsers) poll the endpoint and reload when
/// the value advances.
#[derive(Clone)]
pub struct ReloadContext {
    pub gen: Arc<AtomicU64>,
}

/// Polling endpoint advertised in [`ReloadContext`]. Plaintext body:
/// the current generation counter as a decimal integer.
pub const RELOAD_GEN_URL: &str = "/__idealyst/gen";

/// JSON endpoint published when an [`AasContext`] is supplied. Body
/// is `{"url": "<ws://...>"}` for a discovered server, or `{"url":
/// null}` while we're still browsing. Browsers can re-poll this on
/// WebSocket disconnect to pick up a server that restarted on a
/// different port.
pub const AAS_URL_URL: &str = "/__idealyst/aas_url";

/// Plumbed in by callers (typically `web-dev-host`) that have a
/// live mDNS browser running. The HTTP server reads `aas_url` for
/// each request, returns it via [`AAS_URL_URL`], and inlines a tiny
/// `<script>window.IDEALYST_AAS_URL = "..."</script>` into served
/// HTML so wasm bundles can pick the URL up synchronously on boot.
///
/// `Arc<Mutex<Option<String>>>` keeps the producer thread (mDNS
/// browse) and the consumer thread (HTTP serve) cleanly decoupled
/// — flipping the URL doesn't require touching the HTTP loop.
#[derive(Clone)]
pub struct AasContext {
    pub aas_url: Arc<std::sync::Mutex<Option<String>>>,
}

/// Inlined into the `<body>` of every served HTML response when
/// reload is active. Polls every 400ms; reloads when the generation
/// counter advances. Tiny on purpose — the page reloads on every
/// rebuild anyway, so there's no state to preserve here.
const RELOAD_SCRIPT: &str = r#"<script>
(function () {
  var last = null;
  setInterval(function () {
    fetch("/__idealyst/gen", { cache: "no-store" })
      .then(function (r) { return r.text(); })
      .then(function (text) {
        if (last !== null && text !== last) {
          location.reload();
        }
        last = text;
      })
      .catch(function () { /* server is rebuilding; try again next tick */ });
  }, 400);
})();
</script>"#;

pub fn serve_static(
    host: &str,
    port: u16,
    root: &Path,
    reload: Option<ReloadContext>,
    aas: Option<AasContext>,
) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;

    let root = fs::canonicalize(root)
        .with_context(|| format!("cannot canonicalize serve root {}", root.display()))?;

    let mut extras = Vec::new();
    if reload.is_some() {
        extras.push("livereload");
    }
    if aas.is_some() {
        extras.push("aas-url");
    }
    eprintln!(
        "[dev-http] serving {} on http://{}{}",
        root.display(),
        addr,
        if extras.is_empty() {
            String::new()
        } else {
            format!(" ({})", extras.join(", "))
        },
    );

    for request in server.incoming_requests() {
        if let Err(e) = handle(&root, reload.as_ref(), aas.as_ref(), request) {
            eprintln!("[dev-http] request error: {e}");
        }
    }

    Ok(())
}

fn handle(
    root: &Path,
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
    request: Request,
) -> Result<()> {
    // GET / HEAD only. Anything else (POST, PUT, …) isn't meaningful
    // for a static-file dev server.
    if !matches!(request.method(), Method::Get | Method::Head) {
        return request
            .respond(Response::empty(405))
            .map_err(Into::into);
    }

    let url_path = request.url().split('?').next().unwrap_or("/");

    // Livereload generation endpoint. Always answers — even when
    // reload is off it returns 0, which means clients that polled
    // once and got a non-zero value won't reload on accident if the
    // server is restarted in static mode.
    if url_path == RELOAD_GEN_URL {
        let gen = reload
            .map(|r| r.gen.load(Ordering::Relaxed))
            .unwrap_or(0);
        return request
            .respond(
                Response::from_string(gen.to_string())
                    .with_header(header("Content-Type", "text/plain"))
                    .with_header(header("Cache-Control", "no-store")),
            )
            .map_err(Into::into);
    }

    // AAS-URL endpoint. JSON body `{"url": "<ws://...>"}` or
    // `{"url": null}` while discovery hasn't found a match. The
    // wasm side reads this on disconnect to pick up a server that
    // restarted on a different port. Even when no AasContext is
    // wired up we answer (with `null`) so wasm clients that poll
    // it don't have to special-case the dev-mode-off case.
    if url_path == AAS_URL_URL {
        let url = aas
            .and_then(|c| c.aas_url.lock().ok().and_then(|g| g.clone()));
        let body = match url {
            Some(u) => format!("{{\"url\":\"{}\"}}", json_escape(&u)),
            None => "{\"url\":null}".to_string(),
        };
        return request
            .respond(
                Response::from_string(body)
                    .with_header(header("Content-Type", "application/json"))
                    .with_header(header("Cache-Control", "no-store")),
            )
            .map_err(Into::into);
    }

    let wants_html = request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Accept") && h.value.as_str().contains("text/html"));

    let resolved = resolve(&root, url_path);

    match resolved {
        Some(path) if path.is_dir() => {
            let index = path.join("index.html");
            if index.is_file() {
                respond_with_file(request, &index, reload, aas)
            } else {
                not_found(request)
            }
        }
        Some(path) if path.is_file() => respond_with_file(request, &path, reload, aas),
        _ if wants_html => {
            // SPA fallback. Unknown route, but the browser is asking
            // for HTML — serve the root index so a client-side router
            // can decide what to render. Asset requests (JS, WASM,
            // images) bypass this branch and get a real 404.
            let index = root.join("index.html");
            if index.is_file() {
                respond_with_file(request, &index, reload, aas)
            } else {
                not_found(request)
            }
        }
        _ => not_found(request),
    }
}

/// Resolve a URL path to an on-disk path under `root`. Returns `None`
/// if the path escapes the root (via `..` or symlinks) or doesn't
/// exist. Callers must still check `is_file` / `is_dir`.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let trimmed = url_path.trim_start_matches('/');
    let candidate = if trimmed.is_empty() {
        root.to_path_buf()
    } else {
        root.join(trimmed)
    };
    let canonical = fs::canonicalize(&candidate).ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn respond_with_file(
    request: Request,
    path: &Path,
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
) -> Result<()> {
    let ct = content_type(path);
    let is_html = matches!(ct, "text/html; charset=utf-8");

    // HTML responses get script tags injected (livereload + AAS
    // URL). Everything else streams straight from disk — wasm
    // bundles can be large (hello-web's release wasm is ~13 MB),
    // and `Response::from_file` sets up chunked transfer for us.
    let needs_injection = is_html && (reload.is_some() || aas.is_some());
    if needs_injection {
        let mut body = String::new();
        fs::File::open(path)
            .with_context(|| format!("open {}", path.display()))?
            .read_to_string(&mut body)
            .with_context(|| format!("read {}", path.display()))?;
        if let Some(ctx) = aas {
            body = inject_aas_url(body, ctx);
        }
        if reload.is_some() {
            body = inject_reload_script(body);
        }
        let response = Response::from_string(body)
            .with_header(header("Content-Type", ct))
            .with_header(header("Cache-Control", "no-store"));
        request.respond(response).map_err(Into::into)
    } else {
        let file = fs::File::open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut response = Response::from_file(file);
        response.add_header(header("Content-Type", ct));
        // Dev mode should never see stale HTML/JS/WASM — disable
        // caching globally. The browser refetches on every reload,
        // which is what you want while iterating.
        response.add_header(header("Cache-Control", "no-store"));
        request.respond(response).map_err(Into::into)
    }
}

/// Insert `<script>window.IDEALYST_AAS_URL = "..."</script>` right
/// inside the `<head>` so it executes before any wasm init. wasm
/// reads the global synchronously on boot — no async fetch round
/// trip. When discovery hasn't found a server yet, the value is
/// `null` and the wasm waits / polls `AAS_URL_URL`.
fn inject_aas_url(html: String, ctx: &AasContext) -> String {
    let url = ctx.aas_url.lock().ok().and_then(|g| g.clone());
    let value = match url {
        Some(u) => format!("\"{}\"", json_escape(&u)),
        None => "null".to_string(),
    };
    let snippet = format!(
        "<script>window.IDEALYST_AAS_URL = {};</script>\n",
        value
    );
    if let Some(idx) = html.find("</head>") {
        let (head, tail) = html.split_at(idx);
        let mut out = String::with_capacity(html.len() + snippet.len());
        out.push_str(head);
        out.push_str(&snippet);
        out.push_str(tail);
        out
    } else {
        // No `</head>` — prepend so it's still first to execute.
        format!("{snippet}{html}")
    }
}

/// Minimal JSON string escape for the values we actually produce —
/// ws URLs, which are ASCII + `:` + `/`. Escape backslashes and
/// double-quotes; control chars don't appear in these URLs.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

fn inject_reload_script(html: String) -> String {
    // Inject just before `</body>` so the script executes after the
    // page's own scripts have started. Fall back to appending when
    // there's no `</body>` (single-line or fragment HTML); the
    // browser is forgiving about scripts after the closing tag.
    if let Some(idx) = html.rfind("</body>") {
        let (head, tail) = html.split_at(idx);
        let mut out = String::with_capacity(html.len() + RELOAD_SCRIPT.len() + 1);
        out.push_str(head);
        out.push_str(RELOAD_SCRIPT);
        out.push('\n');
        out.push_str(tail);
        out
    } else {
        format!("{html}\n{RELOAD_SCRIPT}")
    }
}

fn not_found(request: Request) -> Result<()> {
    request
        .respond(Response::from_string("404 not found").with_status_code(404))
        .map_err(Into::into)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("header constructed from static-known valid bytes")
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" | "map" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
