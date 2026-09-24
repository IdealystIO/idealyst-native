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
use dev_reload::{PatchKind, ReloadSignal};
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

/// Where a page reports what it did with a pushed event: a `POST` whose
/// body is one JSON `dev_events::PageAck` (`{"kind":"hot_patch",
/// "redirected":3,"carried":12}`). Served beside [`RELOAD_SSE_URL`] on
/// both servers, handed to [`ReloadSignal::page_ack`].
///
/// This is how the terminal learns a save LANDED rather than merely that
/// it was sent: the facts used to exist only in the browser's console.
/// The page posts with `navigator.sendBeacon` — a CORS "simple" request,
/// so the cross-origin full-stack stream needs no preflight, and a
/// beacon sent from a page that is about to reload still goes out.
pub const ACK_URL: &str = "/__idealyst/ack";

/// The session's whole event stream, as Server-Sent Events: one
/// versioned JSON object per `data:` line — the same objects
/// `idealyst dev --events-file` writes (schema: `idealyst dev
/// --events-schema`). A subscriber first gets a snapshot of the session
/// as it is now (see `dev_events::snapshot`), then every event live.
/// Served with `Access-Control-Allow-Origin: *` on every dev server that
/// has the stream, and on its own port by [`serve_events`] — the URL is
/// in `<project>/.idealyst/events.url` — so an editor, a script, or
/// another tab can subscribe with nothing but the port.
pub const EVENTS_URL: &str = "/__idealyst/events";

/// Largest ack body accepted. An ack is a few dozen bytes; anything near
/// this is not an ack.
const MAX_ACK_BYTES: u64 = 16 * 1024;
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
///
/// The same stream also carries `patch` events — a saved edit the dev
/// loop decided needs no rebuild. Those go to the page's overlay entry
/// point instead of reloading it, which is the entire point: a reload
/// throws away scroll position, form state and every signal in the app,
/// and a changed label does not need any of that thrown away. A page
/// whose bundle was built without the overlay has no entry point, so
/// the patch is ignored and the next real rebuild carries the edit.
///
/// And `dev-state` events — the session's build state (building, with
/// progress; patched; failed, with the rustc errors), which the status
/// overlay ([`STATUS_OVERLAY_JS`]) renders over the page. A page that
/// connects gets the current state first, so one that loads after a
/// failed build still shows the error.
///
/// And `hot-patch` events — a body edit, which needs new code rather
/// than new data. The payload is `{url, table}`: where the patch module
/// is served from, and a `subsecond_types::JumpTable` pairing the base's
/// function table slots to the patch's. The page applies it and rebuilds
/// the tree in place, carrying every signal value across.
///
/// The fallback differs from the overlay's on purpose. An overlay patch
/// that cannot be applied is ignored, because the next rebuild carries
/// the edit anyway. A hot patch that cannot be applied means the dev loop
/// has ALREADY decided not to rebuild — so the page would sit there
/// running code the source no longer describes. It reloads instead.
///
/// Every outcome is also reported back to [`ACK_URL`] (see there): the
/// connect, a reload, an applied overlay patch with the applier's counts,
/// and any failure. A hot patch applies asynchronously, so its ack comes
/// from the bundle itself once the rebuilt tree is mounted — through
/// `window.__idealyst_dev_ack`, which this script publishes.
const RELOAD_SCRIPT: &str = r#"<script>
(function () {
  var baseline = null;
  // Where the stream may be, in order: `[stream, ack]` pairs. One pair on
  // the static path; on the full-stack path the app server's same-origin
  // proxy first, then the dev session's own port (see `reload_script_tag_with_fallback`).
  var SOURCES = __SOURCES__;
  var ACK = "__ACK_URL__";
  function ack(o) {
    try {
      var body = JSON.stringify(o);
      if (!(navigator.sendBeacon && navigator.sendBeacon(ACK, body))) {
        fetch(ACK, { method: "POST", body: body, keepalive: true }).catch(function () {});
      }
    } catch (_) {}
  }
  window.__idealyst_dev_ack = ack;
__STATUS_OVERLAY__
  var status = idealystStatusOverlay(document);
  function wire(es) {
  es.onmessage = function (e) {
    if (baseline === null) {
      baseline = e.data;
      ack({ kind: "connected", gen: Number(e.data) });
    } else if (e.data !== baseline) {
      ack({ kind: "reloading", gen: Number(e.data) });
      location.reload();
    }
  };
  es.addEventListener("patch", function (e) {
    var apply = window.__idealyst_overlay_patch;
    if (typeof apply !== "function") {
      console.info("[idealyst] overlay patch ignored: this bundle has no overlay");
      ack({ kind: "failed", what: "overlay", error: "this bundle has no overlay" });
      return;
    }
    try {
      var r = apply(e.data);
      ack(r && typeof r === "object"
        ? { kind: "overlay", applied: r.applied, refused: r.refused }
        : { kind: "overlay" });
    } catch (err) {
      console.error("[idealyst] overlay patch failed, reloading", err);
      ack({ kind: "failed", what: "overlay", error: String(err) });
      location.reload();
    }
  });
  es.addEventListener("hot-patch", function (e) {
    var apply = window.__idealyst_hot_patch;
    if (typeof apply !== "function") {
      console.info("[idealyst] hot patch ignored: this bundle has no patch applier");
      ack({ kind: "failed", what: "hot_patch", error: "this bundle has no patch applier" });
      location.reload();
      return;
    }
    var payload;
    try {
      payload = JSON.parse(e.data);
    } catch (err) {
      console.error("[idealyst] hot patch: unreadable event, reloading", err);
      ack({ kind: "failed", what: "hot_patch", error: "unreadable event" });
      location.reload();
      return;
    }
    try {
      apply(payload.url, JSON.stringify(payload.table));
    } catch (err) {
      console.error("[idealyst] hot patch failed, reloading", err);
      ack({ kind: "failed", what: "hot_patch", error: String(err) });
      location.reload();
    }
  });
  es.addEventListener("dev-state", function (e) {
    try {
      status.apply(JSON.parse(e.data));
    } catch (err) {
      console.warn("[idealyst] dev-state event not shown", err);
    }
  });
  }
  // A source that answers with something other than the stream (a 404,
  // the app's index.html) closes the EventSource for good before it ever
  // opens: try the next. One that is merely down (a server restarting)
  // stays CONNECTING and retries on its own, so it is kept.
  function connect(i) {
    var es = new EventSource(SOURCES[i][0]);
    ACK = SOURCES[i][1];
    var opened = false;
    es.addEventListener("open", function () { opened = true; });
    es.addEventListener("error", function () {
      if (!opened && es.readyState === 2 && i + 1 < SOURCES.length) {
        connect(i + 1);
      }
    });
    wire(es);
  }
  connect(0);
})();
</script>"#;

/// The in-page build-state overlay, spliced into [`RELOAD_SCRIPT`]. See
/// the file's header for what it shows and why it is plain JS.
pub const STATUS_OVERLAY_JS: &str = include_str!("status_overlay.js");

/// The livereload + overlay `<script>`, pointed at `sse_url`.
///
/// Parameterised because the page and the stream do not always share an
/// origin. In the static shape `dev-http` serves both and a relative
/// path is right; in the FULL-STACK shape the app's own server hands out
/// `index.html` and this stream runs beside it on a CLI-owned port, so
/// the page needs an absolute URL — and the stream needs CORS, which is
/// why [`serve_signal_only`] sends it.
///
/// The URL is escaped rather than interpolated: it is assembled from a
/// port number today, but a `"` reaching it would close the string
/// literal and leave the rest of the page's `<head>` executable.
pub fn reload_script_tag(sse_url: &str) -> String {
    reload_script_tag_with_fallback(sse_url, None)
}

/// [`reload_script_tag`] with a second place to find the stream: the page
/// tries `sse_url` first and moves to `fallback` only if `sse_url`
/// answers with something that is not the stream (a `404`, or the app's
/// `index.html` from a catch-all). A source that is merely unreachable —
/// a server restarting — is kept: the `EventSource` retries it.
///
/// The full-stack shape: `sse_url` is the relative `/__idealyst/reload`,
/// which the app server proxies to the dev session when it is built on
/// the framework's server router (`server::dev_stream`) — the one origin
/// that is always reachable, forwarded container port or not. `fallback`
/// is the dev session's own port, for a server that does not proxy.
pub fn reload_script_tag_with_fallback(sse_url: &str, fallback: Option<&str>) -> String {
    // The ack endpoint lives beside the stream, on whichever origin
    // serves it — so it is derived from the stream's URL, absolute or not.
    let ack_for = |sse: &str| match sse.strip_suffix(RELOAD_SSE_URL) {
        Some(origin) => format!("{origin}{ACK_URL}"),
        None => ACK_URL.to_string(),
    };
    let mut sources = vec![(sse_url.to_string(), ack_for(sse_url))];
    if let Some(f) = fallback {
        sources.push((f.to_string(), ack_for(f)));
    }
    // JSON-encoded, then `<` escaped: the array sits in an inline
    // `<script>`, where a `</script>` in a URL would end it. Nothing in a
    // port-built URL has one, but the encoder is what guarantees it.
    let list = serde_json::to_string(&sources)
        .expect("strings serialize")
        .replace('<', "\\u003c");
    let ack = serde_json::to_string(&sources[0].1).expect("a string serializes").replace('<', "\\u003c");
    RELOAD_SCRIPT
        .replace("__SOURCES__", &list)
        .replace("\"__ACK_URL__\"", &ack)
        .replace("__STATUS_OVERLAY__", STATUS_OVERLAY_JS)
}

/// Serve ONLY the session's event stream ([`EVENTS_URL`]) on `port`.
///
/// Every `idealyst dev` session runs one, so a session with no web
/// target — an iOS-only one — is subscribable too.
pub fn serve_events(
    host: &str,
    port: u16,
    events: Arc<dev_events::broadcast::Broadcast>,
) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
    for request in server.incoming_requests() {
        let url_path = request.url().split('?').next().unwrap_or("/").to_string();
        if url_path != EVENTS_URL {
            let _ = request.respond(cors(Response::empty(404)));
            continue;
        }
        stream_events(request, events.clone());
    }
    Ok(())
}

/// Hand `request` a stream of `events` on its own thread.
fn stream_events(request: Request, events: Arc<dev_events::broadcast::Broadcast>) {
    if let Err(e) = thread::Builder::new()
        .name("dev-http-events".into())
        .spawn(move || serve_event_stream(request, &events))
    {
        dev_events::global().error("dev-http", format!("cannot spawn events thread: {e}"));
    }
}

/// The body of [`EVENTS_URL`]: the snapshot, then live events, then a
/// keepalive comment whenever the session is quiet.
fn serve_event_stream(request: Request, events: &dev_events::broadcast::Broadcast) {
    let mut writer = request.into_writer();
    let head = b"HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Cache-Control: no-store\r\n\
                 Connection: close\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 X-Accel-Buffering: no\r\n\
                 \r\n";
    if writer.write_all(head).is_err() || writer.flush().is_err() {
        return;
    }
    let sub = events.subscribe();
    for json in &sub.snapshot {
        if write_data(&mut writer, json).is_err() {
            return;
        }
    }
    loop {
        match sub.events.recv_timeout(SSE_KEEPALIVE) {
            Ok(json) => {
                if write_data(&mut writer, &json).is_err() {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if write_ping(&mut writer).is_err() {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn write_data(w: &mut Box<dyn Write + Send + 'static>, json: &str) -> std::io::Result<()> {
    w.write_all(format!("data: {json}\n\n").as_bytes())?;
    w.flush()
}

/// Take a page's ack off `request` and hand it to `signal`. Always
/// answers 204: the page fires and forgets, and has nothing to do with a
/// refusal.
fn receive_ack(mut request: Request, signal: Option<&ReloadSignal>) -> std::io::Result<()> {
    let mut body = String::new();
    let _ = request.as_reader().take(MAX_ACK_BYTES).read_to_string(&mut body);
    if let Some(signal) = signal {
        signal.page_ack(&body);
    }
    request.respond(cors(Response::empty(204)))
}

/// Serve ONLY the livereload/overlay SSE stream, on `port`.
///
/// For the full-stack shape, where the app's own server serves the page
/// and `dev-http` has no files to hand out. Without this a full-stack
/// project gets neither livereload nor overlay patches — the push
/// channel simply does not exist for it, because the SSE endpoint lives
/// here and nothing was running here.
///
/// Every route answers with `Access-Control-Allow-Origin: *`: the page
/// is on the app server's origin and this is on another port, so the
/// browser treats the `EventSource` as cross-origin.
pub fn serve_signal_only(host: &str, port: u16, signal: Arc<ReloadSignal>) -> Result<()> {
    let addr = format!("{host}:{port}");
    let server = Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}"))?;
    dev_events::global().emit(dev_events::DevEvent::ServerReady {
        target: "web".into(),
        kind: dev_events::ServerKind::ReloadStream,
        url: format!("http://{addr}{RELOAD_SSE_URL}"),
    });

    for request in server.incoming_requests() {
        let url_path = request.url().split('?').next().unwrap_or("/").to_string();
        if url_path == ACK_URL && *request.method() == Method::Post {
            let _ = receive_ack(request, Some(&signal));
            continue;
        }
        if url_path == EVENTS_URL {
            match signal.events() {
                Some(events) => stream_events(request, events),
                None => {
                    let _ = request.respond(cors(Response::empty(404)));
                }
            }
            continue;
        }
        if url_path != RELOAD_SSE_URL {
            let _ = request.respond(cors(Response::empty(404)));
            continue;
        }
        let signal = Some(signal.clone());
        if let Err(e) = thread::Builder::new()
            .name("dev-http-sse".into())
            .spawn(move || serve_sse(request, signal))
        {
            dev_events::global().error("dev-http", format!("cannot spawn SSE thread: {e}"));
        }
    }
    Ok(())
}

/// Add the permissive CORS header every route on the signal-only server
/// carries. See [`serve_signal_only`].
fn cors<R: std::io::Read>(response: Response<R>) -> Response<R> {
    response.with_header(
        tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..])
            .expect("static header"),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn serve_static(
    host: &str,
    port: u16,
    root: &Path,
    reload: Option<ReloadContext>,
    aas: Option<AasContext>,
    preload: Option<PreloadContext>,
    overlay: Option<OverlayContext>,
    head: Option<HeadInjectionContext>,
    fallback_index: Option<String>,
    precompressed: bool,
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
                dev_events::global()
                    .warn("dev-http", format!("overlay root {} skipped: {e}", p.display()));
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
    if precompressed {
        extras.push("precompressed".to_string());
    }
    dev_events::global().log(
        "dev-http",
        format!(
            "serving {} on http://{}{}",
            root.display(),
            addr,
            if extras.is_empty() {
                String::new()
            } else {
                format!(" ({})", extras.join(", "))
            },
        ),
    );

    for request in server.incoming_requests() {
        if let Err(e) = handle(
            &root,
            &overlay_roots,
            reload.as_ref(),
            aas.as_ref(),
            preload.as_ref(),
            head.as_ref(),
            fallback_index.as_deref(),
            precompressed,
            request,
        ) {
            dev_events::global().warn("dev-http", format!("request error: {e}"));
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle(
    root: &Path,
    overlay_roots: &[PathBuf],
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
    preload: Option<&PreloadContext>,
    head: Option<&HeadInjectionContext>,
    fallback_index: Option<&str>,
    precompressed: bool,
    request: Request,
) -> Result<()> {
    // A page's ack: the one POST this server takes.
    if *request.method() == Method::Post
        && request.url().split('?').next() == Some(ACK_URL)
    {
        return receive_ack(request, reload.map(|r| &*r.signal)).map_err(Into::into);
    }

    // GET / HEAD only. Anything else (PUT, DELETE, …) isn't meaningful
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
    if url_path == EVENTS_URL {
        if let Some(events) = reload.and_then(|r| r.signal.events()) {
            stream_events(request, events);
            return Ok(());
        }
        return request.respond(cors(Response::empty(404))).map_err(Into::into);
    }

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
                respond_with_file(request, &index, reload, aas, preload, head, precompressed)
            } else if let Some(html) = fallback_index {
                // Project ships no index.html — serve the synthesized
                // default so `idealyst dev --web` works without the user
                // hand-authoring boilerplate.
                respond_with_html_string(request, html, reload, aas, preload, head)
            } else {
                not_found(request)
            }
        }
        Some(path) if path.is_file() => {
            respond_with_file(request, &path, reload, aas, preload, head, precompressed)
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
                respond_with_file(request, &index, reload, aas, preload, head, precompressed)
            } else if let Some(html) = fallback_index {
                respond_with_html_string(request, html, reload, aas, preload, head)
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
    // `Access-Control-Allow-Origin` unconditionally: in the full-stack
    // shape the page is served by the app's own server on another port,
    // so the browser treats this stream as cross-origin and drops it
    // without the header. Harmless on the same-origin static path, and
    // this is a dev-only server that binds localhost.
    let head = b"HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Cache-Control: no-store\r\n\
                 Connection: close\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 X-Accel-Buffering: no\r\n\
                 \r\n";
    if writer.write_all(head).is_err() || writer.flush().is_err() {
        return;
    }

    // Seed with the current generation so the inline script gets a
    // baseline event immediately on connect. Without this the page
    // would sit on an empty stream until the next rebuild.
    let mut last_seen = signal.as_ref().map(|s| s.current()).unwrap_or(0);
    // Patches already decided are NOT replayed to a page connecting
    // now: it just loaded the bundle, which was built from the current
    // source. Replaying would re-apply edits that are already in it.
    //
    // The build STATE is: a page that loads mid-build, or after a build
    // failed, must show that without having seen it happen. Snapshot and
    // sequence come from one lock, so the live events that follow start
    // exactly where the snapshot ends.
    let (state, mut last_patch) = match signal.as_ref() {
        Some(s) => s.dev_state_snapshot(),
        None => (Vec::new(), 0),
    };
    if write_event(&mut writer, last_seen).is_err() {
        return;
    }
    for json in &state {
        if write_patch(&mut writer, PatchKind::DevState.sse_event(), json).is_err() {
            return;
        }
    }

    loop {
        match &signal {
            Some(sig) => {
                let (new, patch_seq) = sig.wait_past_either(last_seen, last_patch, SSE_KEEPALIVE);
                if patch_seq > last_patch {
                    for patch in sig.patches_since(last_patch) {
                        if write_patch(&mut writer, patch.kind.sse_event(), &patch.json).is_err()
                        {
                            return;
                        }
                        last_patch = last_patch.max(patch.seq);
                    }
                    last_patch = last_patch.max(patch_seq);
                }
                if new > last_seen {
                    last_seen = new;
                    if write_event(&mut writer, new).is_err() {
                        return;
                    }
                } else if patch_seq == last_patch && new == last_seen && write_ping(&mut writer).is_err() {
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

/// One patch, as a named SSE event so the page's handler can tell it
/// from a generation bump — and an overlay patch from a hot patch —
/// without parsing the payload first.
///
/// The JSON is emitted on a single `data:` line — SSE splits a payload
/// on newlines and the client would have to rejoin them, so the
/// serializer that produced this must not pretty-print. Asserted rather
/// than hoped: a newline here would silently truncate the patch.
fn write_patch(
    w: &mut Box<dyn Write + Send + 'static>,
    event: &str,
    json: &str,
) -> std::io::Result<()> {
    debug_assert!(
        !json.contains('\n'),
        "a patch must be one SSE data line; serialize it compactly"
    );
    w.write_all(format!("event: {event}\ndata: {json}\n\n").as_bytes())?;
    w.flush()
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
    precompressed: bool,
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
        let body = inject_html_head(body, reload, aas, preload, head);
        let response = Response::from_string(body)
            .with_header(header("Content-Type", ct))
            .with_header(header("Cache-Control", "no-store"));
        request.respond(response).map_err(Into::into)
    } else {
        // Precompressed-sidecar negotiation (`idealyst serve
        // --precompressed`): release builds stage `<file>.br` next to
        // every compressible file (and `.gz` works the same way if
        // present). When the client accepts the encoding and the
        // sidecar exists, stream it with the ORIGINAL's Content-Type
        // plus Content-Encoding — that's what a production host with
        // `brotli_static`/`precompressed` does, so bundle-performance
        // numbers measured here match deployment. Injection-decorated
        // HTML never reaches this branch (the mutated body can't come
        // from a sidecar).
        if precompressed {
            let (br, gz) = accepted_encodings(&request);
            for (accepted, encoding, ext) in [(br, "br", "br"), (gz, "gzip", "gz")] {
                if !accepted {
                    continue;
                }
                // `path` is canonicalized inside the serve root, so
                // appending an extension cannot escape it.
                let mut os = path.as_os_str().to_owned();
                os.push(format!(".{ext}"));
                let sidecar = PathBuf::from(os);
                if !sidecar.is_file() {
                    continue;
                }
                let file = fs::File::open(&sidecar)
                    .with_context(|| format!("open {}", sidecar.display()))?;
                let mut response = Response::from_file(file);
                response.add_header(header("Content-Type", ct));
                response.add_header(header("Content-Encoding", encoding));
                response.add_header(header("Vary", "Accept-Encoding"));
                response.add_header(header("Cache-Control", "no-store"));
                return request.respond(response).map_err(Into::into);
            }
        }
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

/// Which of (brotli, gzip) the request's `Accept-Encoding` accepts.
/// Token-level parse honoring `;q=0` exclusions (`br;q=0` means "do
/// NOT send brotli"); quality ORDERING beyond zero/non-zero is ignored
/// — brotli sidecars are strictly smaller, so preferring them when
/// both are acceptable is always right.
fn accepted_encodings(request: &Request) -> (bool, bool) {
    let mut br = false;
    let mut gz = false;
    let mut gz_explicit = false;
    for h in request.headers() {
        if !h.field.equiv("Accept-Encoding") {
            continue;
        }
        for token in h.value.as_str().split(',') {
            let mut parts = token.trim().split(';');
            let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let rejected = parts.any(|p| {
                let p = p.trim();
                p.strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q == 0.0)
            });
            match name.as_str() {
                "br" => br = !rejected,
                "gzip" => {
                    gz = !rejected;
                    gz_explicit = true;
                }
                // `*` accepts anything not otherwise listed — an
                // explicit `gzip` token beats it in either order.
                // Treating the wildcard as gzip-only keeps us
                // conservative (browsers list `br` outright).
                "*" if !gz_explicit => gz = !rejected,
                _ => {}
            }
        }
    }
    (br, gz)
}

/// Apply the dev-time `<head>`/script injections (runtime-server URL,
/// livereload, font preloads, free-form head) to an HTML string. Shared
/// by the on-disk index path ([`respond_with_file`]) and the synthesized
/// fallback index ([`respond_with_html_string`]) so both emit identical
/// dev decorations.
fn inject_html_head(
    mut body: String,
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
    preload: Option<&PreloadContext>,
    head: Option<&HeadInjectionContext>,
) -> String {
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
    body
}

/// Serve an in-memory HTML string (the synthesized fallback index for a
/// project that ships no `index.html`), with the same dev head/script
/// injection an on-disk `index.html` gets.
fn respond_with_html_string(
    request: Request,
    html: &str,
    reload: Option<&ReloadContext>,
    aas: Option<&AasContext>,
    preload: Option<&PreloadContext>,
    head: Option<&HeadInjectionContext>,
) -> Result<()> {
    let body = inject_html_head(html.to_string(), reload, aas, preload, head);
    let response = Response::from_string(body)
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"));
    request.respond(response).map_err(Into::into)
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
        let script = reload_script_tag(RELOAD_SSE_URL);
        let mut out = String::with_capacity(html.len() + script.len() + 1);
        out.push_str(head);
        out.push_str(&script);
        out.push('\n');
        out.push_str(tail);
        out
    } else {
        format!("{html}\n{}", reload_script_tag(RELOAD_SSE_URL))
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
