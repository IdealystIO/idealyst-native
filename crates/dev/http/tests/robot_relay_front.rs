//! The page reaches the dev session's Robot relay on its own origin.
//!
//! Regression (CrewForge, `idealyst dev --web --local` in a devcontainer):
//! the served page named the relay as `ws://127.0.0.1:<random port>`, a
//! port of the container's loopback that a host browser cannot reach —
//! only the app's port is forwarded — so the relay never got a client and
//! every robot verb answered "no app connected to the relay". With a relay
//! in its `ReloadContext`, `serve_static` now also answers
//! `/__idealyst/relay` on the app's port by splicing the WebSocket to the
//! relay, and every other request still reaches the static server.
//!
//! Runs the real `robot-relay`: a verb sent on its TCP bridge (the MCP
//! side) reaches the "page" through the app port, and the page's answer
//! comes back the same way.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use dev_http::{serve_static, ReloadContext, ROBOT_RELAY_URL};
use dev_reload::ReloadSignal;
use tungstenite::Message;

fn pick_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn wait_for(port: u16) {
    for _ in 0..250 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("the server never bound {port}");
}

#[test]
fn regression_robot_relay_reachable_through_the_app_port() {
    let relay = robot_relay::start(robot_relay::RelayConfig {
        register: false,
        ..Default::default()
    })
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html><head></head><body>app</body></html>").unwrap();
    let root = dir.path().to_path_buf();
    let port = pick_port();
    let ws_addr = relay.ws_addr;
    thread::spawn(move || {
        let ctx = ReloadContext { signal: ReloadSignal::new(), relay: Some(ws_addr) };
        let _ = serve_static("127.0.0.1", port, &root, Some(ctx), None, None, None, None, None, false);
    });
    wait_for(port);

    // The page, dialing its own origin.
    let (mut page, response) =
        tungstenite::connect(format!("ws://127.0.0.1:{port}{ROBOT_RELAY_URL}")).expect("handshake through the app port");
    assert_eq!(response.status(), 101);
    page.send(Message::Text(r#"{"hello":{"name":"t","platform":"web"}}"#.into())).unwrap();

    // The MCP side sends a verb on the relay's TCP bridge.
    let mcp = TcpStream::connect(relay.tcp_addr).unwrap();
    mcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut mcp_w = mcp.try_clone().unwrap();
    mcp_w.write_all(b"{\"id\":7,\"cmd\":\"ping\",\"args\":{}}\n").unwrap();

    // relay -> app port -> page.
    let forwarded = match page.read().unwrap() {
        Message::Text(t) => t.to_string(),
        other => panic!("expected the forwarded verb, got {other:?}"),
    };
    let v: serde_json::Value = serde_json::from_str(&forwarded).unwrap();
    assert_eq!(v["cmd"], "ping", "{forwarded}");
    let rid = v["id"].as_u64().unwrap();

    // page -> app port -> relay -> MCP.
    page.send(Message::Text(format!(r#"{{"id":{rid},"ok":"pong"}}"#).into())).unwrap();
    let mut line = String::new();
    BufReader::new(mcp).read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["ok"], "pong");

    // The app itself is still served on the same port, through the front.
    let mut http = TcpStream::connect(("127.0.0.1", port)).unwrap();
    http.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    http.write_all(b"GET /index.html HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    http.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains("<body>app"), "{out}");
}

/// The same regression, end to end in a real browser: a robot-enabled web
/// bundle is served by `serve_static`, and the injected relay URL names a
/// port nothing listens on — what a host browser sees of a container's
/// loopback. The app must still reach the relay, through the page's own
/// origin, and answer a verb from the MCP side.
///
/// `#[ignore]`: needs Chrome and a pre-built bundle:
/// ```text
/// idealyst build --web --robot --out-dir /tmp/rw examples/welcome
/// ROBOT_WEB_DIST=/tmp/rw cargo test -p dev-http --test robot_relay_front -- --ignored
/// ```
#[test]
#[ignore = "needs headless Chrome + a robot web bundle in ROBOT_WEB_DIST (see doc comment)"]
fn browser_app_reaches_the_relay_same_origin_when_its_loopback_url_is_unreachable() {
    let dist = std::path::PathBuf::from(std::env::var("ROBOT_WEB_DIST").expect("ROBOT_WEB_DIST"));
    let chrome = std::env::var("ARENA_CHROME")
        .unwrap_or_else(|_| "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
    let relay = robot_relay::start(robot_relay::RelayConfig { register: false, ..Default::default() }).unwrap();
    let dead = pick_port();
    let port = pick_port();
    let ws_addr = relay.ws_addr;
    thread::spawn(move || {
        let ctx = ReloadContext { signal: ReloadSignal::new(), relay: Some(ws_addr) };
        let head = dev_http::HeadInjectionContext {
            html: format!("<script>window.IDEALYST_ROBOT_RELAY_URL=\"ws://127.0.0.1:{dead}\";</script>"),
        };
        let _ = serve_static("127.0.0.1", port, &dist, Some(ctx), None, None, None, Some(head), None, false);
    });
    wait_for(port);

    let profile = tempfile::tempdir().unwrap();
    let mut chrome = std::process::Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--remote-debugging-port=0"])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(format!("http://127.0.0.1:{port}/"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("launch chrome");

    let mut connected = None;
    for _ in 0..45 {
        let s = TcpStream::connect(relay.tcp_addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        let mut w = s.try_clone().unwrap();
        w.write_all(b"{\"id\":1,\"cmd\":\"get_snapshot\",\"args\":{}}\n").unwrap();
        let mut line = String::new();
        let _ = BufReader::new(s).read_line(&mut line);
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
        if v.get("ok").is_some_and(|ok| !ok.is_null()) {
            connected = Some(v);
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    let _ = chrome.kill();
    let _ = chrome.wait();
    let snapshot = connected.expect("the browser app never reached the relay through its own origin");
    assert!(snapshot.to_string().len() > 20, "{snapshot}");
}
