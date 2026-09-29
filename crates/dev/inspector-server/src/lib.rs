//! The Idealyst Inspector's debug server.
//!
//! The server owns all the debug information. It discovers running apps,
//! holds the one robot-bridge connection per inspected app, polls and
//! accumulates that app's state, and runs actions on it. A front end
//! (the Inspector, in a browser or as a desktop app) only loads from the
//! server and talks to it over [`inspector_protocol`]. It never opens a
//! connection to an app. That's what lets the Inspector run in a
//! browser, which can't open raw TCP sockets or read
//! `~/.idealyst/apps`.
//!
//! ```text
//!  app ──(bridge / relay TCP)──▶ inspector-server ──(HTTP + WebSocket)──▶ front end
//! ```
//!
//! One local port (127.0.0.1 only) serves:
//!
//! - `GET /ws` with `Upgrade: websocket`: the Inspector socket
//!   ([`inspector_protocol::WS_PATH`]);
//! - `GET /health`: `idealyst-inspector`, so a launcher can tell a
//!   running server from another process on the port ([`probe`]);
//! - everything else: the front end's static web bundle ([`Assets`]).
//!   The CLI embeds that bundle in its binary at build time.
//!
//! The server runs on plain threads (no async runtime) until the process
//! exits.
//!
//! ## Guarding the socket
//!
//! Browsers don't apply CORS to WebSockets, so any page the developer
//! visits could otherwise open `ws://127.0.0.1:9719/ws` and drive their
//! dev app. The upgrade is refused unless its `Origin` is this server's
//! own (`http://127.0.0.1:<port>` / `http://localhost:<port>`), or
//! absent: native clients such as the desktop Inspector send none.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inspector_protocol::{
    AppInfo, ClientMsg, Focus, ServerMsg, Snapshot, Status, DEFAULT_PORT, HEALTH_BODY, HEALTH_PATH, PROTOCOL_VERSION,
    WS_PATH,
};
use tungstenite::Message;

mod bridge;
pub mod discovery;
mod session;

use session::{Outbound, SessionMsg, TabId};

/// How long a request head may take to arrive.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest request head accepted.
const MAX_HEAD: usize = 16 * 1024;
/// How often a front end's socket thread wakes to flush queued frames.
const WS_POLL: Duration = Duration::from_millis(20);

/// The front end's static files, by path relative to the site root
/// (`index.html`, `pkg/inspector.js`, …).
#[derive(Clone, Debug)]
pub enum Assets {
    /// Compiled in: what the CLI serves (`include_bytes!` from build.rs).
    Embedded(&'static [(&'static str, &'static [u8])]),
    /// Read from a staged web bundle on each request: for working on the
    /// front end itself without rebuilding the CLI
    /// (`idealyst inspect --bundle-dir`).
    Dir(PathBuf),
}

impl Assets {
    pub const fn new(files: &'static [(&'static str, &'static [u8])]) -> Self {
        Assets::Embedded(files)
    }

    /// No front end: the socket works, pages 404.
    pub const EMPTY: Assets = Assets::Embedded(&[]);

    fn get(&self, path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
        match self {
            Assets::Embedded(files) => {
                files.iter().find(|(p, _)| *p == path).map(|(_, bytes)| std::borrow::Cow::Borrowed(*bytes))
            }
            Assets::Dir(root) => {
                // Only plain relative segments: a request can't climb out.
                if path.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
                    return None;
                }
                std::fs::read(root.join(path)).ok().map(std::borrow::Cow::Owned)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// 0 = ephemeral.
    pub port: u16,
    pub assets: Assets,
    /// Where apps register. `None` = discover nothing (connect by address
    /// only).
    pub apps_dir: Option<PathBuf>,
    /// How often the apps directory is rescanned for changes to push.
    pub scan_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: DEFAULT_PORT,
            assets: Assets::EMPTY,
            apps_dir: discovery::default_apps_dir(),
            scan_interval: Duration::from_secs(1),
        }
    }
}

/// A running server.
pub struct Server {
    pub addr: SocketAddr,
    hub: Arc<Hub>,
}

impl Server {
    /// `http://127.0.0.1:<port>`: where a browser opens the Inspector.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The apps currently discovered.
    pub fn apps(&self) -> Vec<AppInfo> {
        self.hub.apps.lock().unwrap().clone()
    }
}

/// Bind the port and start serving. Returns once the listener is bound.
pub fn start(config: Config) -> std::io::Result<Server> {
    let listener = TcpListener::bind(("127.0.0.1", config.port))?;
    let addr = listener.local_addr()?;
    let hub = Arc::new(Hub {
        port: addr.port(),
        assets: config.assets,
        apps_dir: config.apps_dir,
        apps: Mutex::new(Vec::new()),
        tabs: Mutex::new(HashMap::new()),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        next_tab: AtomicU64::new(1),
    });
    hub.rescan();
    {
        let hub = hub.clone();
        let every = config.scan_interval;
        std::thread::spawn(move || loop {
            std::thread::sleep(every);
            hub.rescan();
        });
    }
    {
        let hub = hub.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let hub = hub.clone();
                std::thread::spawn(move || serve_connection(stream, &hub));
            }
        });
    }
    Ok(Server { addr, hub })
}

/// `true` if an Inspector server answers on `127.0.0.1:<port>`.
pub fn probe(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(500))
    else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET {HEALTH_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut body = String::new();
    let _ = s.read_to_string(&mut body);
    body.starts_with("HTTP/1.1 200") && body.ends_with(HEALTH_BODY)
}

struct Hub {
    port: u16,
    assets: Assets,
    apps_dir: Option<PathBuf>,
    apps: Mutex<Vec<AppInfo>>,
    /// Every connected front end's outbound queue, for app-list pushes.
    tabs: Mutex<HashMap<TabId, Outbound>>,
    sessions: session::Registry,
    next_tab: AtomicU64,
}

impl Hub {
    /// Re-read the apps directory; push the list to every front end if it
    /// changed.
    fn rescan(&self) {
        let now = self.apps_dir.as_deref().map(discovery::list).unwrap_or_default();
        let mut apps = self.apps.lock().unwrap();
        if *apps == now {
            return;
        }
        *apps = now.clone();
        drop(apps);
        let frame = ServerMsg::Apps { apps: now }.to_json();
        self.tabs.lock().unwrap().retain(|_, out| out.send(frame.clone()).is_ok());
    }

    fn apps_frame(&self) -> String {
        ServerMsg::Apps { apps: self.apps.lock().unwrap().clone() }.to_json()
    }

    fn find_app(&self, id: &str) -> Option<AppInfo> {
        self.apps.lock().unwrap().iter().find(|a| a.id == id).cloned()
    }

    /// Join (or start) the session for `key`.
    fn attach(&self, key: String, app: String, addr: String, tab: TabId, out: Outbound, focus: Focus) -> Sender<SessionMsg> {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(tx) = sessions.get(&key) {
            if tx.send(SessionMsg::AddTab { tab, out: out.clone(), focus }).is_ok() {
                return tx.clone();
            }
        }
        let tx = session::spawn(key.clone(), app, addr, self.sessions.clone());
        let _ = tx.send(SessionMsg::AddTab { tab, out, focus });
        sessions.insert(key, tx.clone());
        tx
    }
}

// =============================================================================
// HTTP
// =============================================================================

struct Request {
    method: String,
    /// Without the query string.
    path: String,
    headers: HashMap<String, String>,
    /// Bytes of the head, terminator included.
    head_len: usize,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Peek (not read) the request head, so a WebSocket upgrade can hand the
/// untouched stream to the handshake.
fn peek_head(stream: &TcpStream) -> Option<Request> {
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    let deadline = Instant::now() + HEAD_TIMEOUT;
    let mut buf = vec![0u8; MAX_HEAD];
    let n = loop {
        let n = stream.peek(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        if let Some(end) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        if n == buf.len() || Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?;
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some(Request { method, path, headers, head_len: n })
}

fn serve_connection(mut stream: TcpStream, hub: &Arc<Hub>) {
    let Some(req) = peek_head(&stream) else { return };
    let upgrade = req.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if req.path == WS_PATH && upgrade {
        if !origin_allowed(req.header("origin"), hub.port) {
            let _ = respond(&mut stream, "403 Forbidden", "text/plain", b"cross-origin Inspector socket refused");
            return;
        }
        let _ = stream.set_read_timeout(None);
        if let Ok(ws) = tungstenite::accept(stream) {
            tab_session(ws, hub);
        }
        return;
    }
    // Consume the head we peeked.
    let mut head = vec![0u8; req.head_len];
    if stream.read_exact(&mut head).is_err() {
        return;
    }
    if req.method != "GET" && req.method != "HEAD" {
        let _ = respond(&mut stream, "405 Method Not Allowed", "text/plain", b"");
        return;
    }
    if req.path == HEALTH_PATH {
        let _ = respond(&mut stream, "200 OK", "text/plain", HEALTH_BODY.as_bytes());
        return;
    }
    let rel = match req.path.trim_start_matches('/') {
        "" => "index.html",
        p => p,
    };
    match hub.assets.get(rel) {
        Some(bytes) => {
            let body = if req.method == "HEAD" { &[][..] } else { &bytes[..] };
            let _ = respond(&mut stream, "200 OK", content_type(rel), body);
        }
        None if rel == "index.html" => {
            let _ = respond(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"This Inspector server has no front end bundled. The desktop Inspector can still connect to its socket.",
            );
        }
        // The browser asks for one unprompted; a bundle without an icon
        // answers "none" rather than logging a 404 on every load.
        None if rel == "favicon.ico" => {
            let _ = respond(&mut stream, "204 No Content", "image/x-icon", b"");
        }
        None => {
            let _ = respond(&mut stream, "404 Not Found", "text/plain", b"not found");
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> std::io::Result<()> {
    // `no-cache`: the bundle changes with every CLI build, and the
    // browser must not run a stale front end against a newer server.
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

/// See the crate docs: same-origin pages and native clients only.
fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else { return true };
    let origin = origin.trim_end_matches('/');
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| origin == format!("http://{host}:{port}"))
}

// =============================================================================
// One front end
// =============================================================================

/// What a front end is attached to.
struct Attached {
    session: Sender<SessionMsg>,
}

fn tab_session(mut ws: tungstenite::WebSocket<TcpStream>, hub: &Arc<Hub>) {
    let tab = hub.next_tab.fetch_add(1, Ordering::Relaxed);
    let (out_tx, out_rx) = mpsc::channel::<String>();
    let _ = out_tx.send(ServerMsg::Hello { protocol: PROTOCOL_VERSION }.to_json());
    // Queue the current list and register under one lock, so a rescan
    // between the two can't slip a newer list in ahead of an older one.
    {
        let mut tabs = hub.tabs.lock().unwrap();
        let _ = out_tx.send(hub.apps_frame());
        tabs.insert(tab, out_tx.clone());
    }
    let _ = ws.get_ref().set_read_timeout(Some(WS_POLL));
    let mut attached: Option<Attached> = None;
    let mut focus = Focus::default();

    'conn: loop {
        let mut wrote = false;
        while let Ok(frame) = out_rx.try_recv() {
            if ws.send(Message::Text(frame.into())).is_err() {
                break 'conn;
            }
            wrote = true;
        }
        if wrote && ws.flush().is_err() {
            break;
        }
        let text = match ws.read() {
            Ok(Message::Text(t)) => t.as_str().to_string(),
            Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                continue
            }
            Err(_) => break,
        };
        let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) else { continue };
        match msg {
            ClientMsg::Attach { app } => {
                detach(&mut attached, tab);
                let info = hub.find_app(&app).or_else(|| {
                    hub.rescan();
                    hub.find_app(&app)
                });
                match info {
                    Some(info) => {
                        let session = hub.attach(format!("app:{app}"), app, info.addr(), tab, out_tx.clone(), focus);
                        attached = Some(Attached { session });
                    }
                    None => {
                        let snap = Snapshot {
                            status: Status::Down(format!("no running app registered as `{app}`")),
                            ..Snapshot::default()
                        };
                        let _ = out_tx.send(ServerMsg::Snapshot { app, snapshot: Box::new(snap) }.to_json());
                    }
                }
            }
            ClientMsg::AttachAddr { addr } => {
                detach(&mut attached, tab);
                let session = hub.attach(format!("addr:{addr}"), addr.clone(), addr, tab, out_tx.clone(), focus);
                attached = Some(Attached { session });
            }
            ClientMsg::Detach => detach(&mut attached, tab),
            ClientMsg::Focus { component, signal } => {
                focus = Focus { component, signal };
                if let Some(a) = &attached {
                    let _ = a.session.send(SessionMsg::Focus(tab, focus));
                }
            }
            ClientMsg::Action { label, cmd, args } => {
                if let Some(a) = &attached {
                    let _ = a.session.send(SessionMsg::Action { tab, label, cmd, args });
                }
            }
            ClientMsg::Rescan => {
                hub.rescan();
                let _ = out_tx.send(hub.apps_frame());
            }
        }
    }
    detach(&mut attached, tab);
    hub.tabs.lock().unwrap().remove(&tab);
}

fn detach(attached: &mut Option<Attached>, tab: TabId) {
    if let Some(a) = attached.take() {
        let _ = a.session.send(SessionMsg::RemoveTab(tab));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_guard_admits_own_origin_and_native_clients_only() {
        assert!(origin_allowed(None, 9719));
        assert!(origin_allowed(Some("http://127.0.0.1:9719"), 9719));
        assert!(origin_allowed(Some("http://localhost:9719/"), 9719));
        assert!(!origin_allowed(Some("http://127.0.0.1:9720"), 9719));
        assert!(!origin_allowed(Some("https://evil.example"), 9719));
        assert!(!origin_allowed(Some("null"), 9719));
    }

    #[test]
    fn a_bundle_dir_serves_its_files_and_nothing_outside_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/a.js"), "js").unwrap();
        std::fs::write(dir.path().join("secret"), "no").unwrap();
        let assets = Assets::Dir(dir.path().join("pkg"));
        assert_eq!(assets.get("a.js").as_deref(), Some(&b"js"[..]));
        assert_eq!(assets.get("../secret"), None);
        assert_eq!(assets.get("/etc/passwd"), None);
        assert_eq!(assets.get("missing.js"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("pkg/app_bg.wasm"), "application/wasm");
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("pkg/app.js"), "text/javascript; charset=utf-8");
    }
}
