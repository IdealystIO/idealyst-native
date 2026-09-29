//! The server end to end: a scripted robot bridge on one side, real
//! WebSocket front ends on the other.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inspector_protocol::{AppInfo, ClientMsg, Focus, Perf, ServerMsg, Snapshot, Status};
use inspector_server::{Assets, Config, Server};
use serde_json::{json, Value};

// =============================================================================
// A scripted bridge
// =============================================================================

#[derive(Default)]
struct Bridge {
    /// Calls recorded since the last `get_perf_counters` (which DRAINS,
    /// as the real bridge does).
    perf_pending: AtomicU64,
    /// Open request/response connections (subscriptions excluded).
    open_conns: AtomicUsize,
    /// Every request/response connection ever accepted.
    total_conns: AtomicUsize,
    /// `get_snapshot` replies this error instead of a tree.
    refuse_snapshot: Option<&'static str>,
}

fn start_bridge(bridge: Bridge) -> (Arc<Bridge>, u16) {
    let bridge = Arc::new(bridge);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let b = bridge.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let b = b.clone();
            std::thread::spawn(move || serve_bridge(stream, &b));
        }
    });
    (bridge, port)
}

fn serve_bridge(stream: TcpStream, b: &Bridge) {
    let mut writer = stream.try_clone().unwrap();
    let mut counted = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let req: Value = serde_json::from_str(&line).unwrap();
        let cmd = req["cmd"].as_str().unwrap_or("");
        if cmd == "subscribe" {
            let _ = writeln!(writer, "{}", json!({ "id": req["id"], "ok": "subscribed" }));
            // Hold the subscription open; pushes aren't scripted here.
            continue;
        }
        if !counted {
            counted = true;
            b.open_conns.fetch_add(1, Ordering::SeqCst);
            b.total_conns.fetch_add(1, Ordering::SeqCst);
        }
        let reply = match cmd {
            "get_snapshot" => match b.refuse_snapshot {
                Some(err) => Err(err.to_string()),
                None => Ok(json!([{ "id": 1, "kind": "View", "test_id": "root", "label": null,
                    "components": [{ "instance_id": 7, "name": "App" }], "children": [] }])),
            },
            "list_components" => Ok(json!([{}])),
            "list_watched_signals" | "list_navigators" | "get_logs" => Ok(json!([])),
            "get_perf_counters" => {
                let n = b.perf_pending.swap(0, Ordering::SeqCst);
                Ok(if n == 0 {
                    json!([])
                } else {
                    json!([{ "phase": "flush", "call_count": n, "total_us": n * 10, "max_us": 10 }])
                })
            }
            "get_component" => {
                let id = req["args"]["instance_id"].as_u64().unwrap();
                Ok(json!({ "instance_id": id, "name": format!("C{id}"), "file": "src/lib.rs", "line": 1,
                    "element_id": null, "methods": [], "props": [] }))
            }
            "get_signal_history" => Ok(Value::Null),
            "clear_logs" => Ok(json!("cleared")),
            other => Err(format!("unknown command `{other}`")),
        };
        let resp = match reply {
            Ok(ok) => json!({ "id": req["id"], "ok": ok }),
            Err(err) => json!({ "id": req["id"], "err": err }),
        };
        if writeln!(writer, "{resp}").is_err() {
            break;
        }
    }
    if counted {
        b.open_conns.fetch_sub(1, Ordering::SeqCst);
    }
}

// =============================================================================
// Server + front ends
// =============================================================================

static INDEX: &[u8] = b"<!doctype html><title>Inspector</title>";
static WASM: &[u8] = b"\0asm\x01\0\0\0";
static FILES: &[(&str, &[u8])] = &[("index.html", INDEX), ("pkg/inspector_bg.wasm", WASM)];

/// A server whose apps directory registers one app, `Todo-<pid>`, at
/// `bridge_port`.
fn server_with_app(bridge_port: u16) -> (Server, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let id = format!("Todo-{pid}");
    std::fs::write(
        dir.path().join(format!("{id}.json")),
        json!({ "port": bridge_port, "pid": pid, "name": "Todo", "platform": "web", "proto": 1 }).to_string(),
    )
    .unwrap();
    let server = inspector_server::start(Config {
        port: 0,
        assets: Assets::new(FILES),
        apps_dir: Some(dir.path().to_path_buf()),
        scan_interval: Duration::from_millis(100),
    })
    .unwrap();
    (server, id, dir)
}

struct Tab {
    ws: tungstenite::WebSocket<TcpStream>,
    last: Option<Snapshot>,
}

impl Tab {
    fn open(server: &Server) -> Tab {
        Tab::open_with(server, None).expect("handshake")
    }

    fn open_with(server: &Server, origin: Option<&str>) -> Result<Tab, tungstenite::Error> {
        let stream = TcpStream::connect(server.addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{}/ws", server.addr)).unwrap();
        if let Some(o) = origin {
            req.headers_mut().insert("Origin", o.parse().unwrap());
        }
        let (ws, _) = tungstenite::client(req, stream).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => e,
            tungstenite::HandshakeError::Interrupted(_) => panic!("handshake interrupted"),
        })?;
        Ok(Tab { ws, last: None })
    }

    fn send(&mut self, msg: ClientMsg) {
        self.ws.send(tungstenite::Message::Text(msg.to_json().into())).unwrap();
    }

    /// Read frames until one satisfies `pred`.
    fn until(&mut self, what: &str, mut pred: impl FnMut(&ServerMsg) -> bool) -> ServerMsg {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for {what}; last snapshot: {:?}", self.last);
            match self.ws.read() {
                Ok(tungstenite::Message::Text(t)) => {
                    let msg: ServerMsg = serde_json::from_str(t.as_str()).unwrap();
                    if let ServerMsg::Snapshot { snapshot, .. } = &msg {
                        self.last = Some((**snapshot).clone());
                    }
                    if pred(&msg) {
                        return msg;
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => panic!("socket error waiting for {what}: {e}"),
            }
        }
    }

    fn snapshot_where(&mut self, what: &str, mut pred: impl FnMut(&Snapshot) -> bool) -> Snapshot {
        match self.until(what, |m| matches!(m, ServerMsg::Snapshot { snapshot, .. } if pred(snapshot))) {
            ServerMsg::Snapshot { snapshot, .. } => *snapshot,
            _ => unreachable!(),
        }
    }

    fn apps(&mut self) -> Vec<AppInfo> {
        match self.until("apps", |m| matches!(m, ServerMsg::Apps { .. })) {
            ServerMsg::Apps { apps } => apps,
            _ => unreachable!(),
        }
    }
}

fn http_get(server: &Server, path: &str) -> String {
    let mut s = TcpStream::connect(server.addr).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", server.addr).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn live(s: &Snapshot) -> bool {
    matches!(s.status, Status::Live { .. })
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn serves_health_and_the_embedded_bundle() {
    let (server, _, _dir) = server_with_app(1);
    assert!(inspector_server::probe(server.addr.port()));

    let index = http_get(&server, "/?app=Todo-1");
    assert!(index.starts_with("HTTP/1.1 200"), "{index}");
    assert!(index.contains("text/html") && index.ends_with("<title>Inspector</title>"), "{index}");

    let wasm = http_get(&server, "/pkg/inspector_bg.wasm");
    assert!(wasm.contains("Content-Type: application/wasm"), "{wasm}");

    assert!(http_get(&server, "/nope.js").starts_with("HTTP/1.1 404"));
    assert!(http_get(&server, "/favicon.ico").starts_with("HTTP/1.1 204"), "no icon is not an error");
}

#[test]
fn a_front_end_is_greeted_with_the_protocol_and_the_app_list() {
    let (_bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut tab = Tab::open(&server);
    tab.until("hello", |m| matches!(m, ServerMsg::Hello { protocol } if *protocol == inspector_protocol::PROTOCOL_VERSION));
    let apps = tab.apps();
    assert_eq!(apps.len(), 1, "{apps:?}");
    assert_eq!(apps[0].id, id);
    assert_eq!(apps[0].port, port);
}

#[test]
fn a_new_registration_is_pushed_without_a_rescan() {
    let (server, _, dir) = server_with_app(1);
    let mut tab = Tab::open(&server);
    assert_eq!(tab.apps().len(), 1);
    let pid = std::process::id();
    std::fs::write(
        dir.path().join(format!("Notes-{pid}.json")),
        json!({ "port": 2, "pid": pid, "name": "Notes" }).to_string(),
    )
    .unwrap();
    tab.until("the second app", |m| matches!(m, ServerMsg::Apps { apps } if apps.len() == 2));
}

#[test]
fn attaching_streams_the_apps_state() {
    let (_bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut tab = Tab::open(&server);
    tab.send(ClientMsg::Attach { app: id.clone() });
    let msg = tab.until("a live snapshot", |m| matches!(m, ServerMsg::Snapshot { snapshot, .. } if live(snapshot)));
    let ServerMsg::Snapshot { app, snapshot } = msg else { unreachable!() };
    assert_eq!(app, id, "the frame names what the front end attached with");
    assert_eq!(snapshot.tree[0].components[0].name, "App");
    assert_eq!(snapshot.component_count, 1);
}

/// `get_perf_counters` drains the app's counters. Two front ends that
/// each polled the app would each see part of the calls; the server's one
/// connection drains them and every front end reads the same total.
#[test]
fn regression_two_front_ends_share_one_perf_total() {
    let (bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut a = Tab::open(&server);
    let mut b = Tab::open(&server);
    a.send(ClientMsg::Attach { app: id.clone() });
    b.send(ClientMsg::Attach { app: id.clone() });
    a.snapshot_where("a live", live);
    b.snapshot_where("b live", live);

    bridge.perf_pending.store(5, Ordering::SeqCst);
    let flush_calls = |s: &Snapshot| match &s.perf {
        Perf::Rows(rows) => rows.iter().find(|r| r.phase == "flush").map(|r| r.call_count),
        Perf::Unavailable(_) => None,
    };
    a.snapshot_where("a sees 5 calls", |s| flush_calls(s) == Some(5));
    b.snapshot_where("b sees 5 calls", |s| flush_calls(s) == Some(5));
    assert_eq!(bridge.total_conns.load(Ordering::SeqCst), 1, "one app connection serves both front ends");
}

#[test]
fn each_front_end_keeps_its_own_focus() {
    let (_bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut a = Tab::open(&server);
    let mut b = Tab::open(&server);
    a.send(ClientMsg::Attach { app: id.clone() });
    b.send(ClientMsg::Attach { app: id.clone() });
    a.send(ClientMsg::Focus { component: Some(7), signal: None });
    b.send(ClientMsg::Focus { component: Some(9), signal: None });
    let name = |s: &Snapshot| s.component.as_ref().map(|c| c.name.clone());
    a.snapshot_where("a's C7", |s| name(s).as_deref() == Some("C7"));
    b.snapshot_where("b's C9", |s| name(s).as_deref() == Some("C9"));
    // Focus set before attaching carries into the attachment.
    let mut c = Tab::open(&server);
    c.send(ClientMsg::Focus { component: Some(3), signal: None });
    c.send(ClientMsg::Attach { app: id });
    c.snapshot_where("c's C3", |s| name(s).as_deref() == Some("C3"));
    assert_eq!(Focus::default().component, None);
}

#[test]
fn an_actions_outcome_goes_to_the_front_end_that_sent_it() {
    let (_bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut a = Tab::open(&server);
    let mut b = Tab::open(&server);
    a.send(ClientMsg::Attach { app: id.clone() });
    b.send(ClientMsg::Attach { app: id.clone() });
    a.snapshot_where("a live", live);
    b.snapshot_where("b live", live);

    a.send(ClientMsg::Action { label: "clear logs".into(), cmd: "clear_logs".into(), args: json!({}) });
    let s = a.snapshot_where("a's result", |s| s.last_action.is_some());
    let result = s.last_action.unwrap();
    assert_eq!((result.label.as_str(), result.result), ("clear logs", Ok(())));

    a.send(ClientMsg::Action { label: "bogus".into(), cmd: "no_such_verb".into(), args: json!({}) });
    let s = a.snapshot_where("a's refusal", |s| s.last_action.as_ref().is_some_and(|r| r.label == "bogus"));
    assert!(s.last_action.unwrap().result.unwrap_err().contains("unknown command"));

    // b never sent anything: a fresh frame for b carries no result.
    b.send(ClientMsg::Focus { component: Some(7), signal: None });
    let s = b.snapshot_where("b refreshed", |s| s.component.is_some());
    assert_eq!(s.last_action, None);
}

/// Carried over from the desktop client: the relay answers every verb
/// "no app connected to the relay" until the app dials in. That must read
/// as Not connected, not as a Live empty tree.
#[test]
fn regression_relay_without_an_app_reads_as_not_connected() {
    let (_bridge, port) = start_bridge(Bridge { refuse_snapshot: Some("no app connected to the relay"), ..Bridge::default() });
    let (server, id, _dir) = server_with_app(port);
    let mut tab = Tab::open(&server);
    tab.send(ClientMsg::Attach { app: id });
    let s = tab.snapshot_where("down", |s| matches!(s.status, Status::Down(_)));
    let Status::Down(err) = s.status else { unreachable!() };
    assert!(err.contains("no app connected to the relay"), "{err}");
}

#[test]
fn attaching_an_unknown_app_says_so() {
    let (server, _, _dir) = server_with_app(1);
    let mut tab = Tab::open(&server);
    tab.send(ClientMsg::Attach { app: "Ghost-1".into() });
    let s = tab.snapshot_where("down", |s| matches!(s.status, Status::Down(_)));
    assert!(matches!(s.status, Status::Down(ref e) if e.contains("Ghost-1")), "{:?}", s.status);
}

#[test]
fn attach_by_address_reaches_an_unregistered_bridge() {
    let (_bridge, port) = start_bridge(Bridge::default());
    let (server, _, _dir) = server_with_app(1);
    let mut tab = Tab::open(&server);
    let addr = format!("127.0.0.1:{port}");
    tab.send(ClientMsg::AttachAddr { addr: addr.clone() });
    let msg = tab.until("live", |m| matches!(m, ServerMsg::Snapshot { snapshot, .. } if live(snapshot)));
    assert!(matches!(msg, ServerMsg::Snapshot { ref app, .. } if *app == addr));
}

#[test]
fn the_app_connection_closes_when_the_last_front_end_leaves() {
    let (bridge, port) = start_bridge(Bridge::default());
    let (server, id, _dir) = server_with_app(port);
    let mut a = Tab::open(&server);
    let mut b = Tab::open(&server);
    a.send(ClientMsg::Attach { app: id.clone() });
    b.send(ClientMsg::Attach { app: id.clone() });
    a.snapshot_where("a live", live);
    b.snapshot_where("b live", live);

    a.send(ClientMsg::Detach);
    drop(a);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(bridge.open_conns.load(Ordering::SeqCst), 1, "b still holds the session");

    drop(b); // socket closes without a Detach
    wait_for("the app connection to close", || bridge.open_conns.load(Ordering::SeqCst) == 0);

    // Attaching again starts a fresh session.
    let mut c = Tab::open(&server);
    c.send(ClientMsg::Attach { app: id });
    c.snapshot_where("c live", live);
    assert_eq!(bridge.total_conns.load(Ordering::SeqCst), 2);
}

/// Browsers don't apply CORS to WebSockets: without the Origin guard any
/// page the developer visits could drive their dev app.
#[test]
fn a_cross_origin_page_cannot_open_the_socket() {
    let (server, _, _dir) = server_with_app(1);
    let err = Tab::open_with(&server, Some("https://evil.example")).err().expect("refused");
    assert!(matches!(err, tungstenite::Error::Http(ref r) if r.status() == 403), "{err:?}");
    let own = format!("http://127.0.0.1:{}", server.addr.port());
    assert!(Tab::open_with(&server, Some(&own)).is_ok());
}
