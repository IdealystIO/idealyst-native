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
//! app → relay   {"hello":{"name":"todo","platform":"web","project_root":"…"}}   (once, on connect)
//! relay → app   {"id":7,"cmd":"find_element","args":{…}}                          (a forwarded request)
//! app → relay   {"id":7,"ok":{…}}  |  {"id":7,"err":"…"}                          (its response)
//! app → relay   {"event":"changed","rev":42}                                      (a push, when subscribed)
//! ```
//!
//! MCP side (TCP, newline-delimited JSON): the existing protocol, unchanged.
//! The relay multiplexes: it rewrites each forwarded request's `id` to a private
//! monotonic id, routes the matching response back to the originating TCP
//! connection, and fans `changed` pushes out to every subscribed TCP client.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
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

#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// WebSocket port the app dials. 0 = ephemeral.
    pub ws_port: u16,
    /// TCP port the MCP server connects to. 0 = ephemeral.
    pub tcp_port: u16,
    /// Write a `~/.idealyst/apps/<name>-<pid>.json` registration so existing
    /// discovery finds the relayed app with no MCP-side changes.
    pub register: bool,
    pub identity: Option<Identity>,
    /// Where `screenshot` PNGs are saved (the host can write; the app can't).
    /// `None` → `~/.idealyst/screenshots`. The CLI passes a project-local dir.
    pub screenshot_dir: Option<PathBuf>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            ws_port: 0,
            tcp_port: 0,
            register: true,
            identity: None,
            screenshot_dir: None,
        }
    }
}

struct Inner {
    /// Outbound channel to the currently-connected app (None when no app is
    /// connected). Cloned by TCP sessions to send forwarded requests.
    app_outbound: Mutex<Option<Sender<String>>>,
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
}

impl Inner {
    fn clear_app(&self) {
        *self.app_outbound.lock().unwrap() = None;
        self.app_subscribed.store(false, Ordering::SeqCst);
        // Fail any in-flight requests so their TCP sessions don't hang.
        self.pending.lock().unwrap().clear();
    }
}

pub struct RelayHandle {
    pub ws_addr: SocketAddr,
    pub tcp_addr: SocketAddr,
    reg_path: Option<PathBuf>,
    _inner: Arc<Inner>,
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        if let Some(p) = &self.reg_path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Start the relay. Binds both listeners synchronously (so the addresses are
/// known on return) and spawns the accept loops in the background.
pub fn start(config: RelayConfig) -> anyhow::Result<RelayHandle> {
    let ws_listener = TcpListener::bind(("127.0.0.1", config.ws_port))?;
    let tcp_listener = TcpListener::bind(("127.0.0.1", config.tcp_port))?;
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
        pending: Mutex::new(HashMap::new()),
        subscribers: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
        app_subscribed: AtomicBool::new(false),
        app_label,
        // CLI-provided dir wins; otherwise the global default.
        screenshot_dir: config.screenshot_dir.clone().or_else(default_screenshots_dir),
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

    Ok(RelayHandle {
        ws_addr,
        tcp_addr,
        reg_path,
        _inner: inner,
    })
}

/// Drive one app's WebSocket connection: pump forwarded requests out, route the
/// app's responses + pushes back.
fn app_session(mut ws: tungstenite::WebSocket<TcpStream>, inner: &Arc<Inner>) {
    let _ = ws.get_ref().set_read_timeout(Some(WS_POLL));
    let (out_tx, out_rx) = channel::<String>();
    *inner.app_outbound.lock().unwrap() = Some(out_tx);

    loop {
        // Send any queued forwarded requests.
        let mut wrote = false;
        while let Ok(frame) = out_rx.try_recv() {
            if ws.send(Message::Text(frame.into())).is_err() {
                inner.clear_app();
                return;
            }
            wrote = true;
        }
        if wrote {
            let _ = ws.flush();
        }

        match ws.read() {
            Ok(Message::Text(t)) => route_from_app(t.as_str(), inner),
            Ok(Message::Binary(b)) => {
                if let Ok(s) = std::str::from_utf8(&b) {
                    route_from_app(s, inner);
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
    inner.clear_app();
}

/// Route a frame the app sent us: a response to a forwarded request, a push, or
/// the one-time `hello`.
fn route_from_app(text: &str, inner: &Arc<Inner>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if v.get("hello").is_some() {
        // Registry hook: today we just note it; the table grows here when
        // native dials in too.
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
                // base64 is kept so the MCP can still return the image inline.
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
