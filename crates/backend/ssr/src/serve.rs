//! Minimal blocking HTTP server for the SSR backend (feature `serve`).
//!
//! Each navigation request renders the matching route to HTML via
//! [`render_path_with`](crate::render_path_with) — on a fresh thread,
//! because the reactive arena is thread-local and each render needs
//! clean state. Asset requests (the built wasm bundle, fonts, …) are
//! served from `static_dir`. Sync, single-connection accept loop —
//! intentionally minimal, for dev / preview, not production.
//!
//! Unlike the static dev server (`dev-http`, which SPA-falls-back to one
//! `index.html`), this renders a *distinct* document per URL — that's
//! the point of SSR. The served page then boots the real WebBackend
//! bundle, which replaces the DOM.

use crate::{render_document, render_path_with, SsrBackend};
use runtime_core::Element;
use std::path::{Path, PathBuf};
use tiny_http::{Header, Response, Server};

/// Configuration for [`serve`].
pub struct ServeConfig {
    /// JS module the served HTML boots to hydrate, e.g.
    /// `Some("/pkg/website.js")`. `None` serves the rendered screen with
    /// no script — a pure SSR preview (SEO / unfurl / look-at-the-page),
    /// no hydration, no risk of a stale bundle duplicating the DOM.
    pub bundle_module: Option<String>,
    /// Directory served for asset requests — fonts (`/fonts/*.ttf`,
    /// needed for the first paint to use the real font) and, when
    /// hydrating, the built web bundle (`/pkg/*.js`, `*.wasm`). `None`
    /// serves no files (text falls back to a system font).
    pub static_dir: Option<PathBuf>,
    /// Extra HTML spliced into the `<head>` of every rendered page.
    /// `build-ssr`'s wrapper template bakes
    /// `icon_gen::web_icon_link_tags()` in here when the project has
    /// an `[icon]` block, so the SSR-rendered HTML references the same
    /// favicon set the static-file path serves out of `static_dir`.
    /// `None` (or empty) suppresses the injection.
    pub extra_head: Option<String>,
}

/// Serve `app` over HTTP at `addr` (e.g. `"127.0.0.1:8080"`). Blocks
/// forever; stop with Ctrl-C. `register` installs navigator chrome
/// handlers per render (e.g. `|b| drawer_navigator::chrome::register(b)`).
pub fn serve<A, R>(addr: &str, config: ServeConfig, register: R, app: A) -> std::io::Result<()>
where
    A: Fn() -> Element + Send + Sync + Clone + 'static,
    R: Fn(&mut SsrBackend) + Send + Sync + Clone + 'static,
{
    let server = Server::http(addr)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    eprintln!("SSR server listening on http://{addr}");
    let canon_root = config.static_dir.as_ref().and_then(|d| d.canonicalize().ok());

    for request in server.incoming_requests() {
        let path = request
            .url()
            .split(|c| c == '?' || c == '#')
            .next()
            .unwrap_or("/")
            .to_string();

        // 1) Static asset (only when a dir is configured and the file
        //    resolves safely under it).
        if let (Some(root), Some(canon)) = (&config.static_dir, &canon_root) {
            if let Some((bytes, ctype)) = read_asset(root, canon, &path) {
                let header = Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
                let _ = request.respond(Response::from_data(bytes).with_header(header));
                continue;
            }
        }

        // 2) Otherwise render the route. Fresh thread = clean thread-local
        //    reactive arena per request.
        let app = app.clone();
        let register = register.clone();
        let bundle = config.bundle_module.clone();
        let extra_head = config.extra_head.clone();
        let req_path = path.clone();
        let html = std::thread::spawn(move || {
            let page = render_path_with(&req_path, register, app);
            render_document(&page, bundle.as_deref(), extra_head.as_deref())
        })
        .join()
        .unwrap_or_else(|_| {
            "<!DOCTYPE html><html><body><h1>500 — render panicked</h1></body></html>".to_string()
        });

        let header =
            Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
        let _ = request.respond(Response::from_string(html).with_header(header));
    }
    Ok(())
}

/// Read the file for `url_path` under `root` (canonicalized to `canon`),
/// rejecting path traversal. `None` when it isn't a servable file — the
/// caller then treats the request as a route to render.
fn read_asset(root: &Path, canon: &Path, url_path: &str) -> Option<(Vec<u8>, &'static str)> {
    let rel = url_path.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let resolved = root.join(rel).canonicalize().ok()?;
    // Traversal guard: the resolved path must stay under the root.
    if !resolved.starts_with(canon) || !resolved.is_file() {
        return None;
    }
    let bytes = std::fs::read(&resolved).ok()?;
    Some((bytes, content_type(&resolved)))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
