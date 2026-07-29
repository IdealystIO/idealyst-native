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

/// Resolve the boot-module URL for a bundle staged by `idealyst build
/// --web`. That build content-addresses `pkg/` — the entry shim is
/// named `<lib>.<16 hex>.js` so redeploys can't be served stale from
/// HTTP caches — and this finds the hashed entry under
/// `<static_dir>/pkg/`, returning e.g.
/// `Some("/pkg/website.3f9a12bc44d0e1a7.js")`. `None` when no
/// fingerprinted shim exists (a dev-loop `pkg/` keeps plain names —
/// callers fall back to `/pkg/<lib>.js`).
///
/// Matches on the file NAME only (never reads contents) so it works
/// against a `--gzip` bundle, whose bytes are gzipped in place.
/// `build_web::find_hashed_entry` is the build-time twin of this;
/// keep their matching rules in sync.
pub fn resolve_bundle_module(static_dir: &Path, lib_name: &str) -> Option<String> {
    let prefix = format!("{lib_name}.");
    let read = std::fs::read_dir(static_dir.join("pkg")).ok()?;
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(mid) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".js"))
        else {
            continue;
        };
        if mid.len() == 16 && mid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Some(format!("/pkg/{name}"));
        }
    }
    None
}

/// Serve `app` over HTTP at `addr` (e.g. `"127.0.0.1:8080"`). Blocks
/// forever; stop with Ctrl-C. `register` installs navigator chrome
/// handlers per render (e.g. `|b| drawer_navigator::chrome::register(b)`).
pub fn serve<A, R>(addr: &str, config: ServeConfig, register: R, app: A) -> std::io::Result<()>
where
    A: Fn() -> Element + Send + Sync + Clone + 'static,
    R: Fn(&mut SsrBackend) + Send + Sync + Clone + 'static,
{
    let bundle = config.bundle_module.clone();
    let extra_head = config.extra_head.clone();
    serve_loop(addr, config, move |path| {
        let page = render_path_with(path, register.clone(), app.clone());
        render_document(&page, bundle.as_deref(), extra_head.as_deref())
    })
}

/// The HTTP mechanism both cores' `serve` entries share: accept loop,
/// static-asset resolution under `static_dir`, and a fresh-thread
/// render per route request (a thread per request is the isolation
/// contract for the OLD core's thread-local reactive arena; the new
/// core's per-request `World` doesn't need it but is unaffected —
/// worlds are per-thread-table entries torn down with the render).
/// `render_page` maps a route path to the complete HTML document.
pub(crate) fn serve_loop<P>(addr: &str, config: ServeConfig, render_page: P) -> std::io::Result<()>
where
    P: Fn(&str) -> String + Send + Sync + Clone + 'static,
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
        //    reactive state per request (see the fn docs).
        let render_page = render_page.clone();
        let req_path = path.clone();
        let html = std::thread::spawn(move || render_page(&req_path))
            .join()
            .unwrap_or_else(|_| {
                "<!DOCTYPE html><html><body><h1>500 — render panicked</h1></body></html>"
                    .to_string()
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

#[cfg(test)]
mod resolve_bundle_module_tests {
    //! `resolve_bundle_module` is what keeps the generated SSR
    //! wrapper's hydration script pointed at the content-hashed
    //! bundle `idealyst build --web` stages. A miss here silently
    //! degrades to the unhashed `/pkg/<lib>.js` default, which a
    //! fingerprinted bundle doesn't contain — so the matching rules
    //! get direct coverage.

    use super::resolve_bundle_module;
    use std::fs;

    #[test]
    fn finds_the_fingerprinted_entry_shim() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("website.3f9a12bc44d0e1a7.js"), b"").unwrap();
        fs::write(pkg.join("website_bg.3f9a12bc44d0e1a7.wasm"), b"").unwrap();
        fs::write(pkg.join("__wasm_split.3f9a12bc44d0e1a7.js"), b"").unwrap();
        assert_eq!(
            resolve_bundle_module(tmp.path(), "website").as_deref(),
            Some("/pkg/website.3f9a12bc44d0e1a7.js"),
        );
    }

    #[test]
    fn dev_shaped_pkg_and_decoys_resolve_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        // Dev-loop pkg: plain names, no fingerprint — callers keep
        // their `/pkg/<lib>.js` default.
        fs::write(pkg.join("website.js"), b"").unwrap();
        fs::write(pkg.join("website_bg.wasm"), b"").unwrap();
        // Wrong hash width / non-hex middles must not match.
        fs::write(pkg.join("website.abc.js"), b"").unwrap();
        fs::write(pkg.join("website.not-a-hash-here.js"), b"").unwrap();
        assert_eq!(resolve_bundle_module(tmp.path(), "website"), None);
        // Missing pkg/ entirely (e.g. --static preview with no web
        // build) must be a graceful None, not an error.
        assert_eq!(resolve_bundle_module(&tmp.path().join("nope"), "website"), None);
    }
}
