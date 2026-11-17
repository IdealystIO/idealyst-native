//! `App::apps` / `App::select_app` against a real relay with two fake apps:
//! each `App` drives the app it selected (want_b853a728, two apps in one
//! test). `open_page` is the relay's real-browser test
//! (`robot-relay/tests/cdp_real_browser.rs`).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use robot_test::{json, App};
use tungstenite::Message;

fn spawn_app(ws_addr: SocketAddr, label: &'static str) {
    std::thread::spawn(move || {
        let (mut ws, _) = tungstenite::connect(format!("ws://{ws_addr}/")).expect("dial");
        ws.send(Message::Text(json!({ "hello": { "platform": "web", "label": label } }).to_string().into())).unwrap();
        while let Ok(msg) = ws.read() {
            let Message::Text(t) = msg else { continue };
            let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap();
            if v["cmd"] == "whoami" {
                ws.send(Message::Text(json!({ "id": v["id"], "ok": label }).to_string().into())).unwrap();
            }
        }
    });
}

#[test]
fn two_apps_are_driven_by_two_connections() {
    let relay = robot_relay::start(robot_relay::RelayConfig { register: false, ..Default::default() }).unwrap();
    spawn_app(relay.ws_addr, "kiosk");
    let mut main = App::connect(relay.tcp_addr).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while main.apps().len() < 1 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    spawn_app(relay.ws_addr, "office");
    let apps = loop {
        let apps = main.apps();
        if apps.len() == 2 && apps.iter().all(|a| a.label.is_some()) {
            break apps;
        }
        assert!(Instant::now() < deadline, "{apps:?}");
        std::thread::sleep(Duration::from_millis(25));
    };
    let kiosk = apps.iter().find(|a| a.label.as_deref() == Some("kiosk")).unwrap().id;

    let mut second = App::connect(relay.tcp_addr).unwrap();
    second.select_app(kiosk);
    assert_eq!(second.call("whoami", json!({})), json!("kiosk"));
    assert_eq!(main.call("whoami", json!({})), json!("office"), "the default: the newest");
    assert!(second.apps().iter().any(|a| a.id == kiosk && a.selected));
    second.select_app(None);
    assert_eq!(second.call("whoami", json!({})), json!("office"));
}
