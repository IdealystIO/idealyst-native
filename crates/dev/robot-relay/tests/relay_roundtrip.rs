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
