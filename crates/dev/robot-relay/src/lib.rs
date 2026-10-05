//! Robot bridge **relay**.
//!
//! A browser tab can't bind a TCP listener, so a web app can't host the Robot
//! bridge the way a native app does. The relay inverts the direction: the app
//! **dials out** to the relay over a WebSocket, and the relay exposes the
//! ordinary newline-delimited-JSON **TCP bridge** that the MCP server (and the
//! arena's evaluator) already know how to discover and drive. Mechanism
//! diverges (dial vs listen), behavior converges — the MCP side is identical to
//! a native app.
//!
//! ## Designed as the universal registry (web now, native next)
//!
//! Although web is the only platform that *dials in* today, nothing here is
//! web-specific. An app announces its identity on connect (a `hello` frame),
//! and the relay keeps an app table. When native moves to dial-out, it speaks
//! the same WS protocol and lands in the same registry — no second transport.
//!
//! ## Protocol
//!
//! App side (WebSocket, text frames):
//! ```text
//! app → relay   {"hello":{"platform":"web","label":"Chrome 131"}}               (once, on connect)
//! relay → app   {"id":7,"cmd":"find_element","args":{…}}                          (a forwarded request)
//! app → relay   {"id":7,"ok":{…}}  |  {"id":7,"err":"…"}                          (its response)
//! app → relay   {"event":"changed","rev":42}                                      (a push, when subscribed)
//! ```
//!
//! `hello` fields are all optional: `platform` (`web`, `macos`, `ios`, …),
//! `label` (what tells two apps of one platform apart — a browser's name and
//! version), and the older `name` / `project_root`, which the relay ignores
//! (its [`Identity`] comes from the dev session).
//!
//! ## Several apps
//!
//! Every open connection is tracked ([`RelayHandle::apps`],
//! [`RelayConfig::on_apps`]), but requests go to ONE app: the newest, until
//! it disconnects, then the newest still connected. Two tabs of one page
//! are both listed; the one opened last is driven.
//!
//! MCP side (TCP, newline-delimited JSON): the existing protocol, unchanged.
//! The relay multiplexes: it rewrites each forwarded request's `id` to a private
//! monotonic id, routes the matching response back to the originating TCP
//! connection, and fans `changed` pushes out to every subscribed TCP client.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::Message;

/// How long a TCP-side request waits for an app to be connected before failing.
const APP_WAIT: Duration = Duration::from_secs(3);
/// How long to wait for the app's response to a forwarded request.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
/// WS read poll slice — bounds outbound-frame latency to the app.
const WS_POLL: Duration = Duration::from_millis(20);

/// Identity used for the `~/.idealyst/apps` registration the MCP server
/// discovers. For web the sidecar knows these (it scaffolded/served the app),
/// so they come from config rather than relying on in-browser identity.
#[derive(Clone, Debug)]
pub struct Identity {
    pub name: String,
    pub bundle_id: Option<String>,
    pub project_root: Option<String>,
}

/// One app connected to the relay, as [`RelayHandle::apps`] and the
/// [`RelayConfig::on_apps`] observer see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// The connection's id: unique for the relay's lifetime, so an app
    /// that redials is a new id.
    pub id: u64,
    /// From the app's `hello`; `None` until it has said one.
    pub platform: Option<String>,
    /// From the app's `hello` (`label`), when it sends one: what tells two
    /// apps of one platform apart, like a browser's name and version.
    pub label: Option<String>,
    /// Whether requests are routed to it — the app that connected last.
    pub active: bool,
}

/// Called with every connected app each time the set changes (an app
/// connects, says `hello`, or disconnects). Calls are serialized and in
/// order, so the latest call is the present; the observer must not call
/// back into the relay.
#[derive(Clone)]
pub struct OnApps(pub Arc<dyn Fn(&[AppInfo]) + Send + Sync>);

impl std::fmt::Debug for OnApps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnApps(..)")
    }
}

#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// WebSocket port the app dials. 0 = ephemeral.
    pub ws_port: u16,
    /// TCP port the MCP server connects to. 0 = ephemeral.
    pub tcp_port: u16,
    /// The interface the TCP bridge listens on. Loopback by default; a
    /// dev session in a container whose bridge port is forwarded to the
    /// host binds every interface, since the forward arrives on the
    /// container's own address, not its loopback. The WebSocket side
    /// always stays on loopback: a page reaches it through its own
    /// origin (`/__idealyst/relay`), never directly.
    pub tcp_host: IpAddr,
    /// Write a `~/.idealyst/apps/<name>-<pid>.json` registration so existing
    /// discovery finds the relayed app with no MCP-side changes.
    pub register: bool,
    pub identity: Option<Identity>,
    /// Where `screenshot` PNGs are saved (the host can write; the app can't).
    /// `None` → `~/.idealyst/screenshots`. The CLI passes a project-local dir.
    pub screenshot_dir: Option<PathBuf>,
    /// Also write the registration here (`<project>/.idealyst/robot.json`),
    /// removed with the relay. `~/.idealyst/apps` is invisible from outside
    /// a container, but the project directory is shared with the host, so
    /// an MCP server running on the host finds a container's relay through
    /// this file — once its port is pinned and forwarded.
    pub project_registration: Option<PathBuf>,
    /// Told whenever the connected apps change.
    pub on_apps: Option<OnApps>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            ws_port: 0,
            tcp_port: 0,
            tcp_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            register: true,
            identity: None,
            screenshot_dir: None,
            project_registration: None,
            on_apps: None,
        }
    }
}

struct Inner {
    /// Outbound channel to the currently-connected app (None when no app is
    /// connected). Cloned by TCP sessions to send forwarded requests.
    app_outbound: Mutex<Option<Sender<String>>>,
    /// Which app session owns `app_outbound` (written under its lock). An
    /// app that redials — the web client reconnects after a dropped socket,
    /// e.g. a same-origin proxy that went away with a restarting app server
    /// — gets a new session while the old one may still be noticing its
    /// socket died; the old one must not clear the new one's channel.
    app_session: AtomicU64,
    /// Forwarded-request id → response sink.
    pending: Mutex<HashMap<u64, Sender<Value>>>,
    /// TCP connections that issued `subscribe`, to fan pushes out to.
    subscribers: Mutex<Vec<Arc<Mutex<TcpStream>>>>,
    next_id: AtomicU64,
    /// Whether we've already told the app to start pushing.
    app_subscribed: AtomicBool,
    /// Filename prefix for saved screenshots (the app's identity name).
    app_label: String,
    /// Where screenshot PNGs are written. `None` disables saving (no HOME and
    /// no configured dir).
    screenshot_dir: Option<PathBuf>,
    /// The `~/.idealyst/apps/<name>-<pid>.json` we wrote at start (if any). The
    /// relay writes it *before* the app dials, so it can't yet know the app's
    /// platform; on the `hello` frame we patch this file with the reported
    /// platform so the MCP server can tell e.g. web from macОS for parity work.
    reg_path: Mutex<Option<PathBuf>>,
    /// Every open app connection, oldest first, with the channel that
    /// reaches it. Requests go to one of them (`app_session`): the newest,
    /// until it disconnects — then the newest that is left. An older one
    /// stays listed until its socket closes, because it IS still connected:
    /// a second browser tab, or a tab whose replacement dialed before it
    /// noticed its own socket die.
    apps: Mutex<Vec<(AppInfo, Sender<String>)>>,
    on_apps: Option<OnApps>,
}

impl Inner {
    /// Change the app list under its lock and tell the observer while
    /// still holding it, so two connections changing at once reach the
    /// observer in the order they happened.
    fn update_apps(&self, change: impl FnOnce(&mut Vec<(AppInfo, Sender<String>)>)) {
        let mut apps = self.apps.lock().unwrap();
        change(&mut apps);
        let active = self.app_session.load(Ordering::SeqCst);
        for (app, _) in apps.iter_mut() {
            app.active = app.id == active;
        }
        if let Some(OnApps(f)) = &self.on_apps {
            let list: Vec<AppInfo> = apps.iter().map(|(a, _)| a.clone()).collect();
            f(&list);
        }
    }

    /// Install `tx` as the connected app's channel, for a new session.
    fn set_app(&self, session: u64, tx: Sender<String>) {
        // Lock order: `app_outbound`, then `apps` (inside `update_apps`).
        let mut out = self.app_outbound.lock().unwrap();
        self.app_session.store(session, Ordering::SeqCst);
        *out = Some(tx.clone());
        self.update_apps(|apps| {
            apps.push((AppInfo { id: session, platform: None, label: None, active: true }, tx))
        });
        drop(out);
        // Requests forwarded to the previous app will not be answered by
        // this one: fail them now rather than at their timeout.
        self.pending.lock().unwrap().clear();
        // The new app has not been told to push yet.
        self.app_subscribed.store(false, Ordering::SeqCst);
    }

    /// Forget a closed session. When it was the one requests went to, they
    /// go to the newest app still connected, if any — returns whether that
    /// happened, so the caller re-subscribes it for existing subscribers.
    fn clear_app(&self, session: u64) -> bool {
        let mut out = self.app_outbound.lock().unwrap();
        let was_routed = self.app_session.load(Ordering::SeqCst) == session;
        let mut next = None;
        self.update_apps(|apps| {
            apps.retain(|(a, _)| a.id != session);
            if was_routed {
                next = apps.last().map(|(a, tx)| (a.id, tx.clone()));
                // Before `update_apps` recomputes `active` from it.
                self.app_session.store(next.as_ref().map_or(0, |(id, _)| *id), Ordering::SeqCst);
            }
        });
        if !was_routed {
            return false;
        }
        let rerouted = next.is_some();
        *out = next.map(|(_, tx)| tx);
        drop(out);
        self.app_subscribed.store(false, Ordering::SeqCst);
        // Fail any in-flight requests so their TCP sessions don't hang.
        self.pending.lock().unwrap().clear();
        rerouted
    }
}

pub struct RelayHandle {
    pub ws_addr: SocketAddr,
    pub tcp_addr: SocketAddr,
    reg_path: Option<PathBuf>,
    project_reg_path: Option<PathBuf>,
    inner: Arc<Inner>,
}

impl RelayHandle {
    /// The registration files this relay wrote (`~/.idealyst/apps/…`,
    /// the project's `robot.json`). They are removed when the handle
    /// drops; a process that exits without dropping it (a signal handler
    /// calling `process::exit`) removes them itself.
    pub fn registration_files(&self) -> Vec<PathBuf> {
        self.reg_path.iter().chain(&self.project_reg_path).cloned().collect()
    }

    /// The apps connected right now, oldest first.
    pub fn apps(&self) -> Vec<AppInfo> {
        self.inner.apps.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
    }

    /// The app's id in discovery: its registration's file stem
    /// (`<name>-<pid>`), which the Inspector attaches by. `None` when the
    /// relay didn't register.
    pub fn app_id(&self) -> Option<String> {
        self.reg_path.as_ref()?.file_stem()?.to_str().map(str::to_string)
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        for p in self.reg_path.iter().chain(&self.project_reg_path) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Start the relay. Binds both listeners synchronously (so the addresses are
/// known on return) and spawns the accept loops in the background.
pub fn start(config: RelayConfig) -> anyhow::Result<RelayHandle> {
    let ws_listener = TcpListener::bind(("127.0.0.1", config.ws_port))?;
    let tcp_listener = TcpListener::bind((config.tcp_host, config.tcp_port))?;
    let ws_addr = ws_listener.local_addr()?;
    let tcp_addr = tcp_listener.local_addr()?;

    let app_label = config
        .identity
        .as_ref()
        .map(|id| id.name.replace(['.', ' ', '/'], "-"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "app".to_string());

    let inner = Arc::new(Inner {
        app_outbound: Mutex::new(None),
        app_session: AtomicU64::new(0),
        pending: Mutex::new(HashMap::new()),
        subscribers: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
        app_subscribed: AtomicBool::new(false),
        app_label,
        // CLI-provided dir wins; otherwise the global default.
        screenshot_dir: config.screenshot_dir.clone().or_else(default_screenshots_dir),
        reg_path: Mutex::new(None),
        apps: Mutex::new(Vec::new()),
        on_apps: config.on_apps.clone(),
    });

    // App side: accept WS dial-ins.
    {
        let inner = inner.clone();
        std::thread::spawn(move || {
            for stream in ws_listener.incoming().flatten() {
                let inner = inner.clone();
                std::thread::spawn(move || {
                    if let Ok(ws) = tungstenite::accept(stream) {
                        app_session(ws, &inner);
                    }
                });
            }
        });
    }

    // MCP side: accept TCP bridge clients.
    {
        let inner = inner.clone();
        std::thread::spawn(move || {
            for stream in tcp_listener.incoming().flatten() {
                let inner = inner.clone();
                std::thread::spawn(move || tcp_session(stream, &inner));
            }
        });
    }

    let reg_path = if config.register {
        config
            .identity
            .as_ref()
            .and_then(|id| write_registration(tcp_addr.port(), id).ok())
    } else {
        None
    };
    // Remember it so the `hello` handler can patch in the app's platform once
    // it dials (we wrote the file before knowing which platform connects).
    *inner.reg_path.lock().unwrap() = reg_path.clone();

    let project_reg_path = match (&config.project_registration, &config.identity) {
        (Some(path), Some(id)) => write_project_registration(path, tcp_addr.port(), id).ok(),
        _ => None,
    };

    Ok(RelayHandle {
        ws_addr,
        tcp_addr,
        reg_path,
        project_reg_path,
        inner,
    })
}

/// Drive one app's WebSocket connection: pump forwarded requests out, route the
/// app's responses + pushes back.
fn app_session(mut ws: tungstenite::WebSocket<TcpStream>, inner: &Arc<Inner>) {
    let _ = ws.get_ref().set_read_timeout(Some(WS_POLL));
    let (out_tx, out_rx) = channel::<String>();
    static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
    let session = NEXT_SESSION.fetch_add(1, Ordering::SeqCst);
    inner.set_app(session, out_tx);
    // A redialing app inherits the subscribers its predecessor had: tell it
    // to push, or they would go quiet.
    if !inner.subscribers.lock().unwrap().is_empty() {
        ensure_app_subscribed(inner);
    }

    loop {
        // Send any queued forwarded requests.
        let mut wrote = false;
        while let Ok(frame) = out_rx.try_recv() {
            if ws.send(Message::Text(frame.into())).is_err() {
                close_session(inner, session);
                return;
            }
            wrote = true;
        }
        if wrote {
            let _ = ws.flush();
        }

        match ws.read() {
            Ok(Message::Text(t)) => route_from_app(t.as_str(), inner, session),
            Ok(Message::Binary(b)) => {
                if let Ok(s) = std::str::from_utf8(&b) {
                    route_from_app(s, inner, session);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // ping/pong/frame
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    close_session(inner, session);
}

/// An app's socket closed. If requests now go to an older connection,
/// that one has never been told to push: tell it, when anyone listens.
fn close_session(inner: &Arc<Inner>, session: u64) {
    if inner.clear_app(session) && !inner.subscribers.lock().unwrap().is_empty() {
        ensure_app_subscribed(inner);
    }
}

/// Route a frame the app sent us: a response to a forwarded request, a push, or
/// the one-time `hello`.
fn route_from_app(text: &str, inner: &Arc<Inner>, session: u64) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Some(hello) = v.get("hello") {
        let field = |k: &str| {
            hello.get(k).and_then(|p| p.as_str()).filter(|s| !s.is_empty()).map(str::to_string)
        };
        let (platform, label) = (field("platform"), field("label"));
        inner.update_apps(|apps| {
            if let Some((app, _)) = apps.iter_mut().find(|(a, _)| a.id == session) {
                app.platform = platform;
                app.label = label;
            }
        });
        // Patch the platform the app just reported into our registration file,
        // so the MCP server can target e.g. web vs macOS distinctly for parity
        // work. We wrote the file before the app dialed, so it had no platform.
        if let Some(platform) = hello.get("platform").and_then(|p| p.as_str()) {
            patch_registration_platform(inner, platform);
        }
        return;
    }
    if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
        if let Some(sink) = inner.pending.lock().unwrap().remove(&id) {
            let _ = sink.send(v);
        }
        return;
    }
    if v.get("event").and_then(|e| e.as_str()) == Some("changed") {
        let mut line = text.to_string();
        line.push('\n');
        let mut subs = inner.subscribers.lock().unwrap();
        subs.retain(|w| w.lock().map(|mut s| s.write_all(line.as_bytes()).is_ok()).unwrap_or(false));
    }
}

/// Serve one MCP/TCP bridge client.
fn tcp_session(stream: TcpStream, inner: &Arc<Inner>) {
    let writer = match stream.try_clone() {
        Ok(w) => Arc::new(Mutex::new(w)),
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(json!(0));
        let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("").to_string();
        let args = req.get("args").cloned().unwrap_or(json!({}));

        if cmd == "subscribe" {
            inner.subscribers.lock().unwrap().push(writer.clone());
            ensure_app_subscribed(inner);
            write_json(&writer, &json!({ "id": id, "ok": "subscribed" }));
            continue;
        }

        match forward(inner, &cmd, &args) {
            Ok(mut resp) => {
                if let Value::Object(map) = &mut resp {
                    map.insert("id".into(), id.clone());
                }
                // A browser tab / device can't write to the dev host, so the
                // relay (which IS on the host) saves screenshot PNGs to a
                // canonical location and adds a `path` to the response. The
                // base64 stays on the wire here; the MCP strips it before the
                // tool result so the bytes never reach the model — readers go
                // through `path`.
                if cmd == "screenshot" {
                    if let Some(dir) = &inner.screenshot_dir {
                        save_screenshot(dir, &inner.app_label, &mut resp);
                    }
                }
                write_json(&writer, &resp);
            }
            Err(e) => write_json(&writer, &json!({ "id": id, "err": e })),
        }
    }
}

/// The global default: `~/.idealyst/screenshots` (peer of `~/.idealyst/apps`),
/// used when the CLI doesn't pass a project-local dir.
fn default_screenshots_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".idealyst").join("screenshots"))
}

/// Decode a `screenshot` response's `png_base64`, write it to
/// `<dir>/<app>-<unix_millis>.png`, and inject the absolute `path` into the
/// response's `ok` object. Best-effort: any failure leaves the response
/// untouched (the base64 is still there).
fn save_screenshot(dir: &Path, label: &str, resp: &mut Value) {
    use base64::Engine as _;
    let Some(b64) = resp
        .get("ok")
        .and_then(|o| o.get("png_base64"))
        .and_then(|v| v.as_str())
    else {
        return;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{label}-{millis}.png"));
    if std::fs::write(&path, &bytes).is_ok() {
        if let Some(ok) = resp.get_mut("ok").and_then(|o| o.as_object_mut()) {
            ok.insert("path".into(), json!(path.to_string_lossy()));
        }
    }
}

/// Forward a request to the app and await its response. The relay rewrites the
/// id to a private monotonic value so concurrent TCP clients never collide.
fn forward(inner: &Arc<Inner>, cmd: &str, args: &Value) -> Result<Value, String> {
    let out_tx = wait_for_app(inner).ok_or_else(|| "no app connected to the relay".to_string())?;
    let rid = inner.next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = channel::<Value>();
    inner.pending.lock().unwrap().insert(rid, tx);

    let frame = json!({ "id": rid, "cmd": cmd, "args": args }).to_string();
    if out_tx.send(frame).is_err() {
        inner.pending.lock().unwrap().remove(&rid);
        return Err("app disconnected".into());
    }
    match rx.recv_timeout(RESPONSE_TIMEOUT) {
        Ok(resp) => Ok(resp),
        Err(_) => {
            inner.pending.lock().unwrap().remove(&rid);
            Err("app response timed out".into())
        }
    }
}

/// Tell the app to begin pushing change events (once).
fn ensure_app_subscribed(inner: &Arc<Inner>) {
    if inner.app_subscribed.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(tx) = inner.app_outbound.lock().unwrap().clone() {
        // Reserved id 0: the app's ack is dropped (no pending entry).
        let _ = tx.send(json!({ "id": 0, "cmd": "subscribe", "args": {} }).to_string());
    }
}

/// Block until an app is connected (or the wait elapses), returning a clone of
/// its outbound channel.
fn wait_for_app(inner: &Arc<Inner>) -> Option<Sender<String>> {
    let deadline = Instant::now() + APP_WAIT;
    loop {
        if let Some(tx) = inner.app_outbound.lock().unwrap().clone() {
            return Some(tx);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn write_json(writer: &Arc<Mutex<TcpStream>>, v: &Value) {
    let mut s = v.to_string();
    s.push('\n');
    if let Ok(mut w) = writer.lock() {
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }
}

/// Write the `~/.idealyst/apps/<name>-<pid>.json` registration the MCP server
/// discovers. Same JSON shape (and `proto:1`) the native bridge writes.
fn write_registration(tcp_port: u16, id: &Identity) -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("no HOME"))?;
    let dir = PathBuf::from(home).join(".idealyst").join("apps");
    std::fs::create_dir_all(&dir)?;
    let pid = std::process::id();
    let label = id.name.replace(['.', ' '], "-");
    let path = dir.join(format!("{label}-{pid}.json"));
    let body = json!({
        "port": tcp_port,
        "pid": pid,
        "name": id.name,
        "bundle_id": id.bundle_id,
        "project_root": id.project_root,
        "proto": 1,
    });
    std::fs::write(&path, body.to_string())?;
    Ok(path)
}

/// Write the project-local registration: the same body as
/// [`write_registration`]'s (`pid` included — it is the dev process's, and
/// meaningless outside its container, so a reader checks liveness by
/// connecting to `port` instead).
fn write_project_registration(path: &Path, tcp_port: u16, id: &Identity) -> anyhow::Result<PathBuf> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = json!({
        "port": tcp_port,
        "pid": std::process::id(),
        "name": id.name,
        "bundle_id": id.bundle_id,
        "project_root": id.project_root,
        "proto": 1,
    });
    std::fs::write(path, body.to_string())?;
    Ok(path.to_path_buf())
}

/// Patch `"platform"` into the registration file we wrote at start, using the
/// value the app reported in its `hello`. Best-effort — a failed read/parse/
/// write just leaves the file platform-less (the MCP server treats absent
/// platform as "unknown"), never disrupts relaying.
fn patch_registration_platform(inner: &Arc<Inner>, platform: &str) {
    let Some(path) = inner.reg_path.lock().unwrap().clone() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("platform".to_string(), Value::String(platform.to_string()));
        let _ = std::fs::write(&path, v.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Inspector opens `idealyst dev --inspect`'s app by this id, and
    /// discovery names apps by their registration's file stem: the two
    /// must agree.
    #[test]
    fn app_id_is_the_registration_file_stem() {
        let handle = |reg_path| RelayHandle {
            ws_addr: "127.0.0.1:1".parse().unwrap(),
            tcp_addr: "127.0.0.1:2".parse().unwrap(),
            reg_path,
            project_reg_path: None,
            inner: Arc::new(Inner {
                app_outbound: Mutex::new(None),
                app_session: AtomicU64::new(0),
                pending: Mutex::new(HashMap::new()),
                subscribers: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                app_subscribed: AtomicBool::new(false),
                app_label: "app".into(),
                screenshot_dir: None,
                reg_path: Mutex::new(None),
                apps: Mutex::new(Vec::new()),
                on_apps: None,
            }),
        };
        // Paths that don't exist: Drop's remove_file is a no-op.
        let registered = handle(Some(PathBuf::from("/nonexistent/.idealyst/apps/My-App-4242.json")));
        assert_eq!(registered.app_id().as_deref(), Some("My-App-4242"));
        assert_eq!(handle(None).app_id(), None);
    }
}
