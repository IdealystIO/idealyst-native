//! Several apps on one relay, each driven by its own connection
//! (`apps` / `select_app`). No browser: fake apps dial the WebSocket, and
//! plain TCP clients drive the bridge.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tungstenite::Message;

/// A fake app labelled `label`: answers `whoami` with its label. Closing
/// `stop` (dropping its sender) disconnects it.
fn spawn_app(ws_addr: SocketAddr, label: &'static str) -> mpsc::Sender<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (mut ws, _) = tungstenite::connect(format!("ws://{ws_addr}/")).expect("app dials relay");
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        }
        ws.send(Message::Text(json!({ "hello": { "platform": "web", "label": label } }).to_string().into()))
            .unwrap();
        ws.flush().unwrap();
        loop {
            if matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                let _ = ws.close(None);
                let _ = ws.flush();
                return;
            }
            let Ok(Message::Text(t)) = ws.read() else { continue };
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["cmd"] == "whoami" {
                ws.send(Message::Text(json!({ "id": v["id"], "ok": label }).to_string().into())).unwrap();
                ws.flush().unwrap();
            }
        }
    });
    stop_tx
}

struct Bridge {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Bridge {
    fn connect(addr: SocketAddr) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        Self { reader: BufReader::new(s.try_clone().unwrap()), writer: s }
    }
    fn call(&mut self, cmd: &str, args: Value) -> Value {
        let mut line = json!({ "id": 1, "cmd": cmd, "args": args, "timeout_ms": 2000 }).to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
        let mut out = String::new();
        self.reader.read_line(&mut out).unwrap();
        serde_json::from_str(out.trim()).unwrap()
    }
}

fn wait_for_apps(bridge: &mut Bridge, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let apps = bridge.call("apps", json!({}))["ok"].as_array().cloned().unwrap_or_default();
        if apps.len() == n && apps.iter().all(|a| a["label"].is_string()) {
            return apps;
        }
        assert!(Instant::now() < deadline, "{n} apps never connected: {apps:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// want_b853a728 (CrewForge): the two-supervisor claim drives one web app
/// as two users at once. Every request went to the newest connection, so a
/// second tab took the first's place. Each connection now drives the app
/// it selects; one that selects none keeps the default.
#[test]
fn regression_each_connection_drives_the_app_it_selected() {
    let relay = robot_relay::start(robot_relay::RelayConfig { register: false, ..Default::default() }).unwrap();
    let _a = spawn_app(relay.ws_addr, "first");
    let mut main = Bridge::connect(relay.tcp_addr);
    wait_for_apps(&mut main, 1);
    let b = spawn_app(relay.ws_addr, "second");
    let apps = wait_for_apps(&mut main, 2);
    let (first, second) = (apps[0]["id"].as_u64().unwrap(), apps[1]["id"].as_u64().unwrap());
    assert_eq!(apps[1]["active"], true, "the newest is the default");

    let mut other = Bridge::connect(relay.tcp_addr);
    assert_eq!(other.call("select_app", json!({ "app": first }))["ok"]["app"], first);
    assert_eq!(other.call("whoami", json!({}))["ok"], "first", "the selected app");
    assert_eq!(main.call("whoami", json!({}))["ok"], "second", "the default, unchanged");
    let listed = other.call("apps", json!({}))["ok"].clone();
    assert_eq!(listed[0]["selected"], true);
    assert_eq!(listed[1]["selected"], false);

    // The selected app going away fails that connection's requests, says
    // why, and leaves the other connection alone.
    let mut pinned_second = Bridge::connect(relay.tcp_addr);
    pinned_second.call("select_app", json!({ "app": second }));
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(5);
    let err = loop {
        let r = pinned_second.call("whoami", json!({}));
        if let Some(e) = r["err"].as_str() {
            break e.to_string();
        }
        assert!(Instant::now() < deadline, "the closed app still answered");
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(err.contains("no longer connected"), "{err}");
    assert_eq!(other.call("whoami", json!({}))["ok"], "first");

    // `select_app {app: null}` goes back to the default; an unknown id is refused.
    assert!(other.call("select_app", json!({ "app": 999 }))["err"].as_str().unwrap().contains("no longer connected"));
    other.call("select_app", json!({ "app": null }));
    assert_eq!(other.call("whoami", json!({}))["ok"], "first", "the default again (the only app left)");
    let _ = second;
}

/// A page another driver opens (the test runner, on Firefox or WebKit)
/// joins as a secondary app after `expect_secondary`, and the default stays.
#[test]
fn expect_secondary_marks_the_next_app_as_secondary() {
    let relay = robot_relay::start(robot_relay::RelayConfig { register: false, ..Default::default() }).unwrap();
    let _a = spawn_app(relay.ws_addr, "first");
    let mut main = Bridge::connect(relay.tcp_addr);
    wait_for_apps(&mut main, 1);
    assert_eq!(main.call("expect_secondary", json!({}))["ok"], true);
    let _b = spawn_app(relay.ws_addr, "second");
    let apps = wait_for_apps(&mut main, 2);
    assert_eq!(apps[0]["active"], true, "the first page is still the default: {apps:?}");
    assert_eq!(apps[1]["active"], false);
    assert_eq!(main.call("whoami", json!({}))["ok"], "first");
    // Taken back: the next page is the default again.
    main.call("expect_secondary", json!({}));
    main.call("expect_secondary", json!({ "cancel": true }));
    let _c = spawn_app(relay.ws_addr, "third");
    let apps = wait_for_apps(&mut main, 3);
    assert_eq!(apps[2]["active"], true, "{apps:?}");
}
