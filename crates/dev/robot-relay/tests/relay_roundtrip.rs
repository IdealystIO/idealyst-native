//! End-to-end relay test with NO browser: a Rust "fake web app" dials the
//! relay's WebSocket and services verbs (standing in for the wasm robot
//! transport), while a plain TCP client drives the relay's bridge exactly as
//! the MCP server / arena evaluator would. Proves request/response forwarding,
//! id remapping, and subscribe→push fan-out.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use tungstenite::Message;

/// A minimal valid 1×1 PNG, base64 — stands in for a backend's capture.
const PNG_1X1: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGNgAAIAAAUAAen63NgAAAAASUVORK5CYII=";

/// A fake web app: connect to the relay over WS, announce identity, then answer
/// forwarded verbs the way the wasm robot transport will.
fn spawn_fake_app(ws_addr: SocketAddr) {
    std::thread::spawn(move || {
        let url = format!("ws://{ws_addr}/");
        let (mut ws, _) = tungstenite::connect(url).expect("app dials relay");
        ws.send(Message::Text(
            json!({ "hello": { "name": "todo", "platform": "web", "project_root": "/tmp/p" } })
                .to_string()
                .into(),
        ))
        .unwrap();
        ws.flush().unwrap();

        loop {
            let msg = match ws.read() {
                Ok(m) => m,
                Err(_) => break,
            };
            let text = match msg {
                Message::Text(t) => t.as_str().to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let id = v.get("id").cloned().unwrap_or(json!(0));
            let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
            let args = v.get("args").cloned().unwrap_or(json!({}));
            let reply = match cmd {
                "ping" => Some(json!({ "id": id, "ok": "pong" })),
                "find_element" => {
                    let want = args.get("label_contains").and_then(|x| x.as_str());
                    if want == Some("Buy milk") {
                        Some(json!({ "id": id, "ok": { "id": "e1", "label": "Buy milk" } }))
                    } else {
                        Some(json!({ "id": id, "ok": Value::Null }))
                    }
                }
                // Ack; the push is emitted just below (exercises fan-out).
                "subscribe" => Some(json!({ "id": id, "ok": "subscribed" })),
                // A 1×1 PNG, like a real backend's capture — the relay should
                // decode + save it host-side and inject a `path`.
                "screenshot" => Some(json!({ "id": id, "ok": {
                    "png_base64": PNG_1X1, "width": 1, "height": 1
                }})),
                _ => Some(json!({ "id": id, "err": "unknown" })),
            };
            if let Some(r) = reply {
                ws.send(Message::Text(r.to_string().into())).unwrap();
                ws.flush().unwrap();
                if cmd == "subscribe" {
                    std::thread::sleep(Duration::from_millis(100));
                    ws.send(Message::Text(
                        json!({ "event": "changed", "rev": 42 }).to_string().into(),
                    ))
                    .unwrap();
                    ws.flush().unwrap();
                }
            }
        }
    });
}

/// Minimal NDJSON TCP client, like the evaluator's RobotClient.
struct TcpBridge {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    id: u64,
}
impl TcpBridge {
    fn connect(addr: SocketAddr) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        Self {
            reader: BufReader::new(s.try_clone().unwrap()),
            writer: s,
            id: 1,
        }
    }
    fn call(&mut self, cmd: &str, args: Value) -> Value {
        let id = self.id;
        self.id += 1;
        let mut line = json!({ "id": id, "cmd": cmd, "args": args }).to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.flush().unwrap();
        self.read_frame()
    }
    fn read_frame(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }
}

fn start_relay() -> robot_relay::RelayHandle {
    robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: false, // don't touch ~/.idealyst/apps in tests
        identity: None,
        screenshot_dir: None,
        ..Default::default()
    })
    .expect("relay starts")
}

#[test]
fn forwards_requests_and_preserves_caller_ids() {
    let relay = start_relay();
    spawn_fake_app(relay.ws_addr);
    // Give the app a moment to dial in.
    std::thread::sleep(Duration::from_millis(200));

    let mut bridge = TcpBridge::connect(relay.tcp_addr);

    let pong = bridge.call("ping", json!({}));
    assert_eq!(pong["id"], 1, "caller's id is restored, not the relay's");
    assert_eq!(pong["ok"], "pong");

    let found = bridge.call("find_element", json!({ "label_contains": "Buy milk" }));
    assert_eq!(found["id"], 2);
    assert_eq!(found["ok"]["label"], "Buy milk");

    let missing = bridge.call("find_element", json!({ "label_contains": "nope" }));
    assert!(missing["ok"].is_null());
}

#[test]
fn subscribe_acks_and_pushes_fan_out() {
    let relay = start_relay();
    spawn_fake_app(relay.ws_addr);
    std::thread::sleep(Duration::from_millis(200));

    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let ack = bridge.call("subscribe", json!({}));
    assert_eq!(ack["ok"], "subscribed");

    // The fake app emits a changed push ~100ms after subscribe.
    let push = bridge.read_frame();
    assert_eq!(push["event"], "changed");
    assert_eq!(push["rev"], 42);
}

#[test]
fn errors_when_no_app_is_connected() {
    let relay = start_relay();
    // No app dials in.
    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let resp = bridge.call("ping", json!({}));
    assert!(
        resp.get("err").is_some(),
        "should report no-app, got: {resp}"
    );
}

#[test]
fn screenshot_response_is_saved_to_the_configured_dir() {
    // Exercise the CLI-supplied directory (e.g. a project-local path).
    let dir = std::env::temp_dir().join("relay_shot_test_dir");
    let _ = std::fs::remove_dir_all(&dir);
    let relay = robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: false,
        identity: Some(robot_relay::Identity {
            name: "relayshottest".into(),
            bundle_id: None,
            project_root: None,
        }),
        screenshot_dir: Some(dir.clone()),
        ..Default::default()
    })
    .expect("relay starts");
    spawn_fake_app(relay.ws_addr);
    std::thread::sleep(Duration::from_millis(200));

    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let resp = bridge.call("screenshot", json!({}));
    let path = resp["ok"]["path"]
        .as_str()
        .expect("relay injects a `path` into the screenshot response");
    assert!(
        path.starts_with(dir.to_str().unwrap()),
        "saved into the configured dir: {path}"
    );
    assert!(path.contains("relayshottest"), "filename uses the app label: {path}");
    assert!(resp["ok"]["png_base64"].is_string(), "base64 kept for inline use");

    let bytes = std::fs::read(path).expect("the screenshot file was written");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "saved file is the decoded PNG");
    std::fs::remove_dir_all(&dir).ok();
}

/// Regression: a web app redials the relay when its socket drops (the
/// same-origin `/__idealyst/relay` proxy goes away with a restarting app
/// server). The new session takes over while the old one may still be
/// noticing its dead socket — and the old session's teardown used to
/// clear the app channel unconditionally, unplugging the NEW app: every
/// verb then failed "no app connected to the relay" with the page
/// connected.
#[test]
fn regression_an_old_session_closing_does_not_unplug_the_redialed_app() {
    let relay = start_relay();
    // The stale session: connected, never answers, closed after the new
    // app is in.
    let (mut stale, _) = tungstenite::connect(format!("ws://{}/", relay.ws_addr)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    spawn_fake_app(relay.ws_addr);
    std::thread::sleep(Duration::from_millis(200));
    stale.close(None).unwrap();
    let _ = stale.flush();
    drop(stale);
    // Several relay poll slices: the stale session has seen the close.
    std::thread::sleep(Duration::from_millis(200));

    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let pong = bridge.call("ping", json!({}));
    assert_eq!(pong["ok"], "pong", "the redialed app still serves verbs: {pong}");
}

/// A redialed app inherits its predecessor's subscribers: it is told to
/// push, so a subscribed inspector keeps getting `changed` events.
#[test]
fn a_redialed_app_is_resubscribed_for_existing_subscribers() {
    let relay = start_relay();
    let (mut first, _) = tungstenite::connect(format!("ws://{}/", relay.ws_addr)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let mut line = json!({ "id": 1, "cmd": "subscribe", "args": {} }).to_string();
    line.push('\n');
    bridge.writer.write_all(line.as_bytes()).unwrap();
    assert_eq!(bridge.read_frame()["ok"], "subscribed");
    // The first app got its subscribe; it goes away.
    let _ = first.read().unwrap();
    first.close(None).unwrap();
    let _ = first.flush();
    drop(first);
    std::thread::sleep(Duration::from_millis(100));

    // The redialed app is told to subscribe (the fake app pushes rev 42
    // on its `subscribe`), and the push reaches the old subscriber.
    spawn_fake_app(relay.ws_addr);
    let push = bridge.read_frame();
    assert_eq!(push["event"], "changed", "{push}");
    assert_eq!(push["rev"], 42);
}

/// Poll `cond` until it holds or a few seconds pass; relay threads notice
/// connects and closes on their own poll slices.
fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An app that only says hello (and never answers a verb).
fn dial_with_hello(relay: &robot_relay::RelayHandle, hello: Value) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>> {
    let (mut ws, _) = tungstenite::connect(format!("ws://{}/", relay.ws_addr)).unwrap();
    ws.send(Message::Text(json!({ "hello": hello }).to_string().into())).unwrap();
    ws.flush().unwrap();
    ws
}

/// The dev panel's robot column is built from these: every open
/// connection is listed with what its `hello` said, the newest is the
/// active one, and the observer's latest call always equals the present.
#[test]
fn every_connected_app_is_listed_and_the_observer_sees_each_change() {
    use std::sync::{Arc, Mutex};
    let seen: Arc<Mutex<Vec<Vec<robot_relay::AppInfo>>>> = Arc::default();
    let sink = seen.clone();
    let relay = robot_relay::start(robot_relay::RelayConfig {
        register: false,
        on_apps: Some(robot_relay::OnApps(Arc::new(move |apps| sink.lock().unwrap().push(apps.to_vec())))),
        ..Default::default()
    })
    .unwrap();
    let platforms = |relay: &robot_relay::RelayHandle| {
        relay
            .apps()
            .into_iter()
            .map(|a| (a.platform, a.label, a.active))
            .collect::<Vec<_>>()
    };

    let tab = dial_with_hello(&relay, json!({ "platform": "web", "label": "Chrome 131" }));
    eventually("the tab's hello", || relay.apps().first().is_some_and(|a| a.platform.is_some()));
    let mut desktop = dial_with_hello(&relay, json!({ "platform": "macos" }));
    eventually("both apps", || relay.apps().iter().filter(|a| a.platform.is_some()).count() == 2);
    assert_eq!(
        platforms(&relay),
        vec![
            (Some("web".into()), Some("Chrome 131".into()), false),
            (Some("macos".into()), None, true),
        ],
        "both are listed; the newest is the one driven"
    );

    desktop.close(None).unwrap();
    let _ = desktop.flush();
    eventually("the desktop app's close", || relay.apps().len() == 1);
    assert_eq!(
        platforms(&relay),
        vec![(Some("web".into()), Some("Chrome 131".into()), true)],
        "the tab that is left becomes the active one"
    );
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last, relay.apps(), "the observer's latest call is the present");
    drop(tab);
}

/// Regression: with two apps connected (two browser tabs), the newer one
/// closing left the relay routing to nothing — every verb failed "no app
/// connected to the relay" while the older tab sat connected. Requests
/// now fall back to the newest app still connected.
#[test]
fn regression_requests_fall_back_to_an_older_app_when_the_active_one_leaves() {
    let relay = start_relay();
    spawn_fake_app(relay.ws_addr);
    eventually("the serving app", || relay.apps().len() == 1);
    // A newer app that takes over, then leaves without answering anything.
    let mut newer = dial_with_hello(&relay, json!({ "platform": "web" }));
    eventually("the newer app", || relay.apps().len() == 2);
    newer.close(None).unwrap();
    let _ = newer.flush();
    eventually("the newer app's close", || relay.apps().len() == 1);

    let mut bridge = TcpBridge::connect(relay.tcp_addr);
    let pong = bridge.call("ping", json!({}));
    assert_eq!(pong["ok"], "pong", "the older app serves verbs again: {pong}");
}

/// `--robot-port`: the TCP bridge binds the asked-for port, on the asked-for
/// interface (every interface, in a container whose port is forwarded).
#[test]
fn a_pinned_tcp_port_is_bound_on_the_asked_for_interface() {
    let free = std::net::TcpListener::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port();
    let relay = robot_relay::start(robot_relay::RelayConfig {
        tcp_port: free,
        tcp_host: std::net::Ipv4Addr::UNSPECIFIED.into(),
        register: false,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(relay.tcp_addr.port(), free);
    assert!(relay.tcp_addr.ip().is_unspecified(), "{}", relay.tcp_addr);
    assert!(relay.ws_addr.ip().is_loopback(), "the WebSocket side stays on loopback");
    // And it is taken: a second relay asking for it fails loudly rather
    // than quietly landing somewhere else.
    let again = robot_relay::start(robot_relay::RelayConfig {
        tcp_port: free,
        tcp_host: std::net::Ipv4Addr::UNSPECIFIED.into(),
        register: false,
        ..Default::default()
    });
    assert!(again.is_err());
}

/// The project-local registration an MCP server on the host finds a
/// container's relay by: it names the bridge port, and it goes away
/// with the relay.
#[test]
fn the_project_registration_names_the_bridge_port_and_goes_with_the_relay() {
    let dir = std::env::temp_dir().join(format!("relay_project_reg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join(".idealyst").join("robot.json");
    let relay = robot_relay::start(robot_relay::RelayConfig {
        register: false,
        identity: Some(robot_relay::Identity {
            name: "todo".into(),
            bundle_id: None,
            project_root: Some(dir.display().to_string()),
        }),
        project_registration: Some(path.clone()),
        ..Default::default()
    })
    .unwrap();
    let body: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(body["port"], relay.tcp_addr.port());
    assert_eq!(body["name"], "todo");
    assert_eq!(body["proto"], 1);
    assert_eq!(relay.registration_files(), vec![path.clone()], "for a session that exits without dropping it");
    drop(relay);
    assert!(!path.exists(), "removed with the relay");
    let _ = std::fs::remove_dir_all(&dir);
}
