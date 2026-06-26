//! Robot tier against a mock bridge. Stands up a TCP server that speaks the
//! real newline-delimited JSON protocol (`{id,cmd,args}` → `{id,ok}`/`{id,err}`)
//! and exercises the client + discovery without needing a running idealyst app.

use arena_spine::verify::robot::{discover, RobotClient};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;

/// Spawn a mock robot bridge. Handles requests until the listener is dropped.
/// `find_element{label_contains:"Buy milk"}` → an element; anything else → null.
fn spawn_bridge() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { break };
            handle(stream);
        }
    });
    addr
}

fn handle(stream: TcpStream) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line) {
        if n == 0 {
            break;
        }
        let req: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => break,
        };
        let id = req.get("id").cloned().unwrap_or(json!(0));
        let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        let args = req.get("args").cloned().unwrap_or(json!({}));
        let resp = match cmd {
            "ping" => json!({ "id": id, "ok": "pong" }),
            "find_element" => {
                let wanted = args.get("label_contains").and_then(|v| v.as_str());
                if wanted == Some("Buy milk") {
                    json!({ "id": id, "ok": { "id": "e1", "label": "Buy milk", "kind": "text" } })
                } else {
                    json!({ "id": id, "ok": Value::Null })
                }
            }
            "boom" => json!({ "id": id, "err": "no such thing" }),
            _ => json!({ "id": id, "err": "unknown cmd" }),
        };
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
        line.clear();
    }
}

#[test]
fn client_round_trips_ok_and_err() {
    let addr = spawn_bridge();
    let mut client = RobotClient::connect(addr).expect("connect");

    assert_eq!(client.call("ping", json!({})).unwrap(), json!("pong"));

    let found = client
        .call("find_element", json!({ "label_contains": "Buy milk" }))
        .unwrap();
    assert_eq!(found["label"], "Buy milk");

    let missing = client
        .call("find_element", json!({ "label_contains": "nope" }))
        .unwrap();
    assert!(missing.is_null());

    // An `err` response surfaces as Err, not a silent null.
    assert!(client.call("boom", json!({})).is_err());
}

#[test]
fn discover_matches_registration_by_project_root() {
    let addr = spawn_bridge();
    let tmp = std::env::temp_dir().join(format!("arena_robot_disc_{}", addr.port()));
    let apps = tmp.join("apps");
    let project = tmp.join("project");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::create_dir_all(&project).unwrap();

    // A registration pointing at our project + the mock bridge's live port.
    let reg = json!({
        "port": addr.port(),
        "project_root": project.to_string_lossy(),
    });
    std::fs::write(apps.join("app-123.json"), reg.to_string()).unwrap();

    let discovered = discover(&project, &apps).expect("a live, matching app");
    assert_eq!(discovered.port(), addr.port());

    std::fs::remove_dir_all(&tmp).ok();
}
