//! Static-file HTTP server used by `idealyst dev`.
//!
//! Sync, single-threaded for the accept loop, intentionally minimal.
//! Dev mode binds to loopback or LAN so there's no TLS, no auth, no
//! compression — those belong in a CDN. What this serves:
//!
//! - Files under a configured root directory.
//! - `index.html` when the URL resolves to a directory.
//! - SPA fallback: a navigation request (Accept: text/html) for a
//!   missing path serves the root `index.html` so client-side routers
//!   keep working. Asset requests for missing files get a real 404.
//! - Correct `Content-Type` for the handful of extensions a typical
//!   idealyst app emits (HTML, JS, WASM, CSS, fonts, images).
//! - Optional livereload SSE stream + HTML script injection when a
//!   [`ReloadContext`] is supplied. The contract: callers (the
//!   `dev-reload` crate, typically) hand us a shared [`ReloadSignal`]
//!   and we stream `data: <gen>\n\n` events at [`RELOAD_SSE_URL`].
//!   Each SSE connection lives in its own thread so the main accept
//!   loop is never blocked.
//!
//! Path-traversal safety: every resolved file path is canonicalized
//! and verified to live under the canonicalized root before being
//! served. Symlinks pointing outside the root are rejected.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use dev_reload::ReloadSignal;
use tiny_http::{Header, Method, Request, Response, Server};

/// Wires the static server to a rebuild loop. When this is `Some`,
/// the server (a) streams generation events over SSE at
/// [`RELOAD_SSE_URL`], and (b) injects a short `EventSource` script
/// into served HTML responses.
///
/// Producers (e.g. `dev-reload`) bump the signal after every
/// successful rebuild; consumers (browsers) hold one SSE connection
/// and reload when an event arrives whose value differs from the
/// one they were initialized with.
#[derive(Clone)]
pub struct ReloadContext {
    pub signal: Arc<ReloadSignal>,
}

/// SSE endpoint advertised in [`ReloadContext`]. Each event is
/// `data: <decimal generation>\n\n`. Comment-only pings (`:\n\n`)
/// every [`SSE_KEEPALIVE`] keep proxies from idling the connection
/// out and surface dead-client TCP errors so the per-connection
/// thread can exit promptly.
pub const RELOAD_SSE_URL: &str = "/__idealyst/reload";

/// Idle interval between SSE keepalive comments. The browser's
/// `EventSource` will reconnect on its own if the TCP connection
/// dies, so this only needs to be short enough to detect dead clients
/// before they accumulate, not short enough to keep them alive
/// through every possible middlebox.
const SSE_KEEPALIVE: Duration = Duration::from_secs(30);

/// JSON endpoint published when an [`AasContext`] is supplied. Body
/// is `{"url": "<ws://...>"}` for a discovered server, or `{"url":
/// null}` while we're still browsing. Browsers can re-poll this on
/// WebSocket disconnect to pick up a server that restarted on a
/// different port.
pub const AAS_URL_URL: &str = "/__idealyst/aas_url";

/// Plumbed in by callers (typically `web-dev-host`) that have a
/// live mDNS browser running. The HTTP server reads `aas_url` for
/// each request, returns it via [`AAS_URL_URL`], and inlines a tiny
/// `<script>window.IDEALYST_RUNTIME_SERVER_URL = "..."</script>` into served
/// HTML so wasm bundles can pick the URL up synchronously on boot.
///
/// `Arc<Mutex<Option<String>>>` keeps the producer thread (mDNS
/// browse) and the consumer thread (HTTP serve) cleanly decoupled
/// — flipping the URL doesn't require touching the HTTP loop.
#[derive(Clone)]
pub struct AasContext {
    pub aas_url: Arc<std::sync::Mutex<Option<String>>>,
}

/// Project-relative font paths the dev launcher reads from
/// `[package.metadata.idealyst.app.web].preload_fonts` (parsed by
/// `build-ios`'s manifest reader). The HTTP server splices one
/// `<link rel="preload" as="font" crossorigin>` tag per path right
/// before `</head>` on every served HTML response — same set the
/// deployed bundle ships (build-web's `stage_bundle` injects the same
/// tags into the staged `index.html`), so the dev loop's first paint
/// matches production.
///
/// Empty `Vec` is fine — the injection helper no-ops, so callers can
/// pass this through unconditionally without checking.
#[derive(Clone, Default)]
pub struct PreloadContext {
    pub font_paths: Vec<String>,
}

/// Additional read-only directories overlaid on top of the main
/// serve root. Used by `idealyst dev` to expose framework-managed
/// generated assets (favicons today; other dev-time outputs as they
/// land) from `target/idealyst/dev/web/` without polluting the
/// project tree. Each request walks the main root first, then the
/// overlay roots in order; the first hit wins.
///
/// The SPA fallback (`index.html` for unknown text/html paths) is
/// still served from the main root only — overlays don't shadow
/// the user's application shell.
///
/// Path-traversal safety: every overlay is canonicalized at
/// startup, and every resolved request path is verified to live
/// under one of the canonical roots before being served.
#[derive(Clone, Default)]
pub struct OverlayContext {
    pub roots: Vec<PathBuf>,
}

/// HTML snippet spliced into the `<head>` of every served HTML
/// response, after font preloads and before the closing `</head>`.
/// `idealyst dev` populates this with `icon_gen::web_icon_link_tags()`
/// when the project declares an icon block, so the dev loop's tag
/// set matches what `build-web` ships in the deployed bundle.
///
/// Empty string = no injection. The deliberately generic field name
/// (rather than `icon_link_tags`) reflects the future direction —
/// other manifest-driven head injections (meta viewport, OG tags,
/// etc.) would chain into the same context.
#[derive(Clone, Default)]
pub struct HeadInjectionContext {
    pub html: String,
}

/// Inlined into the `<body>` of every served HTML response when
/// reload is active. Holds an `EventSource` open against
/// [`RELOAD_SSE_URL`]; reloads the page when the generation in the
/// stream differs from the one received on connect. `EventSource`
/// auto-reconnects on its own (default ~3s backoff), so a server
/// restart or transient network blip recovers without any glue here.
const RELOAD_SCRIPT: &str = r#"<script>
(function () {
  var baseline = null;
  var es = new EventSource("/__idealyst/reload");
  es.onmessage = function (e) {
    if (baseline === null) {
      baseline = e.data;
    } else if (e.data !== baseline) {
      location.reload();
    }
  };
})();
</script>"#;

pub fn serve_static(
    host: &str,
    port: u16,
    root: &Path,
    reload: Option<ReloadContext>,
    aas: Option<AasContext>,
    preload: Option<PreloadContext>,
    overlay: Option<OverlayContext>,
    head: Option<HeadInjectionContext>,
) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;

    let root = fs::canonicalize(root)
        .with_context(|| format!("cannot canonicalize serve root {}", root.display()))?;

    // Canonicalize each overlay root once at startup. Missing
    // overlay paths are dropped with a warning rather than failing
    // the whole launch — a project that doesn't have icons yet
    // shouldn't 500 the dev server.
    let overlay_roots: Vec<PathBuf> = overlay
        .as_ref()
        .map(|o| o.roots.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| match fs::canonicalize(&p) {
            Ok(canonical) => Some(canonical),
            Err(e) => {
                eprintln!(
                    "[dev-http] overlay root {} skipped: {e}",
                    p.display()
                );
                None
            }
        })
        .collect();

    let mut extras = Vec::new();
    if reload.is_some() {
        extras.push("livereload".to_string());
    }
    if aas.is_some() {
        extras.push("aas-url".to_string());
    }
    if preload
        .as_ref()
        .map(|p| !p.font_paths.is_empty())
        .unwrap_or(false)
    {
        extras.push("font-preload".to_string());
    }
    if !overlay_roots.is_empty() {
        extras.push(format!("{} overlay(s)", overlay_roots.len()));
    }
    if head.as_ref().map(|h| !h.html.is_empty()).unwrap_or(false) {
        extras.push("head-inject".to_string());
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
        if let Err(e) = handle(
            &root,
            &overlay_roots,
            reload.as_ref(),
            aas.as_ref(),
            preload.as_ref(),
            head.as_ref(),
            request,
        ) {
            eprintln!("[dev-http] request error: {e}");
        }
    }

    Ok(())
}

fn handle(
    root: &Path,
    overlay_roots: &[PathBuf],
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
    preload: Option<&PreloadContext>,
    head: Option<&HeadInjectionContext>,
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

    // Livereload SSE stream. Detaches to its own thread so the accept
    // loop isn't blocked — `respond` writes chunks for the lifetime
    // of the connection. When reload is off we serve a one-shot
    // event-stream that emits `data: 0\n\n` and ends; the client's
    // `EventSource` will sit on it without ever firing a reload, and
    // the connection costs nothing.
    if url_path == RELOAD_SSE_URL {
        let signal = reload.map(|r| r.signal.clone());
        thread::Builder::new()
            .name("idealyst-sse".into())
            .spawn(move || serve_sse(request, signal))
            .map(|_| ())
            .context("spawn SSE thread")?;
        return Ok(());
    }

    // runtime-server-URL endpoint. JSON body `{"url": "<ws://...>"}` or
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

    // Walk the main root first, then each overlay in order. First
    // match wins. Overlay hits stream like any other file response —
    // same content-type sniff, same HTML head injection — so dev
    // and build paths emit identical bytes for the same source.
    let resolved = resolve_in_roots(root, overlay_roots, url_path);

    match resolved {
        Some(path) if path.is_dir() => {
            let index = path.join("index.html");
            if index.is_file() {
                respond_with_file(request, &index, reload, aas, preload, head)
            } else {
                not_found(request)
            }
        }
        Some(path) if path.is_file() => {
            respond_with_file(request, &path, reload, aas, preload, head)
        }
        _ if wants_html => {
            // SPA fallback. Unknown route, but the browser is asking
            // for HTML — serve the root index so a client-side router
            // can decide what to render. Asset requests (JS, WASM,
            // images) bypass this branch and get a real 404. The SPA
            // shell always comes from the main root; overlays don't
            // get to shadow the user's application entry point.
            let index = root.join("index.html");
            if index.is_file() {
                respond_with_file(request, &index, reload, aas, preload, head)
            } else {
                not_found(request)
            }
        }
        _ => not_found(request),
    }
}

/// Hold an SSE connection open and push one event per generation
/// bump. Runs on its own thread so it doesn't block the accept loop.
///
/// Implementation note: tiny_http wraps every response writer in a
/// 1 KiB `BufWriter` and only flushes when the response ends. The
/// public `Request::respond` API doesn't expose mid-stream flushes,
/// so SSE events would sit in the buffer until either the buffer
/// fills (long after they're useful) or the connection closes
/// (defeating the point). We take ownership of the raw writer via
/// `Request::into_writer`, write a `Connection: close` HTTP head by
/// hand, then push raw `data: <gen>\n\n` frames and flush after
/// each. No chunked encoding needed because `Connection: close`
/// uses end-of-stream to terminate the body.
///
/// When `signal` is `None` (reload disabled) the stream emits one
/// `data: 0` and then sits idle on keepalive pings forever — the
/// inline `EventSource` reconnects on any TCP close, so this stays
/// cheap.
fn serve_sse(request: Request, signal: Option<Arc<ReloadSignal>>) {
    let mut writer = request.into_writer();

    // HTTP/1.1 head. `Connection: close` keeps us out of tiny_http's
    // keep-alive accounting (we're holding the writer for the
    // lifetime of the stream anyway), and lets the browser detect
    // disconnects via TCP close.
    let head = b"HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Cache-Control: no-store\r\n\
                 Connection: close\r\n\
                 X-Accel-Buffering: no\r\n\
                 \r\n";
    if writer.write_all(head).is_err() || writer.flush().is_err() {
        return;
    }

    // Seed with the current generation so the inline script gets a
    // baseline event immediately on connect. Without this the page
    // would sit on an empty stream until the next rebuild.
    let mut last_seen = signal.as_ref().map(|s| s.current()).unwrap_or(0);
    if write_event(&mut writer, last_seen).is_err() {
        return;
    }

    loop {
        match &signal {
            Some(sig) => {
                let new = sig.wait_past(last_seen, SSE_KEEPALIVE);
                if new > last_seen {
                    last_seen = new;
                    if write_event(&mut writer, new).is_err() {
                        return;
                    }
                } else if write_ping(&mut writer).is_err() {
                    return;
                }
            }
            None => {
                thread::sleep(SSE_KEEPALIVE);
                if write_ping(&mut writer).is_err() {
                    return;
                }
            }
        }
    }
}

fn write_event(w: &mut Box<dyn Write + Send + 'static>, gen: u64) -> std::io::Result<()> {
    let line = format!("data: {gen}\n\n");
    w.write_all(line.as_bytes())?;
    w.flush()
}

fn write_ping(w: &mut Box<dyn Write + Send + 'static>) -> std::io::Result<()> {
    // SSE comment line: starts with `:` and is discarded by the
    // browser's `EventSource`. Forces a write so dead clients
    // surface as a write error and the thread exits.
    w.write_all(b":\n\n")?;
    w.flush()
}

/// Resolve a URL path against the main root and then each overlay
/// root in order. Returns the canonical path of the first hit, or
/// `None` if no root has a matching file (and the path didn't
/// escape any root via `..` or symlinks). Callers must still check
/// `is_file` / `is_dir`.
fn resolve_in_roots(
    root: &Path,
    overlay_roots: &[PathBuf],
    url_path: &str,
) -> Option<PathBuf> {
    if let Some(hit) = resolve_under(root, url_path) {
        return Some(hit);
    }
    for overlay in overlay_roots {
        if let Some(hit) = resolve_under(overlay, url_path) {
            return Some(hit);
        }
    }
    None
}

fn resolve_under(root: &Path, url_path: &str) -> Option<PathBuf> {
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
    preload: Option<&PreloadContext>,
    head: Option<&HeadInjectionContext>,
) -> Result<()> {
    let ct = content_type(path);
    let is_html = matches!(ct, "text/html; charset=utf-8");

    // HTML responses get script tags injected (livereload + runtime-server
    // URL + font preloads + head-inject). Everything else streams
    // straight from disk — wasm bundles can be large (hello-web's
    // release wasm is ~13 MB), and `Response::from_file` sets up
    // chunked transfer for us.
    let preload_active = preload
        .map(|p| !p.font_paths.is_empty())
        .unwrap_or(false);
    let head_active = head.map(|h| !h.html.is_empty()).unwrap_or(false);
    let needs_injection =
        is_html && (reload.is_some() || aas.is_some() || preload_active || head_active);
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
        if let Some(ctx) = preload {
            // Same helpers `build-web`'s `stage_bundle` calls — same tag
            // set lands in the response as ships in the deployed bundle.
            let snippet = build_ios::font_preload_tags(&ctx.font_paths);
            body = build_ios::inject_into_head(body, &snippet);
        }
        if let Some(ctx) = head {
            // Free-form `<head>` injection (favicon link tags today;
            // other manifest-driven head metadata as it lands).
            body = build_ios::inject_into_head(body, &ctx.html);
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

/// Insert `<script>window.IDEALYST_RUNTIME_SERVER_URL = "..."</script>` right
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
        "<script>window.IDEALYST_RUNTIME_SERVER_URL = {};</script>\n",
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
