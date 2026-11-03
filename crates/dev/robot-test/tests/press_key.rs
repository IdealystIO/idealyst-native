//! `App::press_key` / `Locator::press_key` against a mock bridge: the key
//! string goes out as the `key` verb's `{key, modifiers}`, a locator adds
//! its element (waiting for it to mount and be enabled), the mechanism
//! comes back, and a key name the app refuses fails at once.

use robot_test::App;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Mock {
    /// `find_element` calls for "password": it mounts on the second look.
    looks: u32,
    /// `key` calls on the field: refused (disabled) for the first one.
    field_keys: u32,
    /// Every `key` request's arguments.
    keys: Vec<Value>,
}

fn respond(cmd: &str, args: &Value, m: &mut Mock) -> Result<Value, String> {
    match cmd {
        "find_element" => {
            m.looks += 1;
            Ok(if m.looks < 2 { Value::Null } else { json!({ "id": 7, "kind": "TextInput", "test_id": "password", "label": null }) })
        }
        "key" => {
            m.keys.push(args.clone());
            if args["key"] == "Return" {
                return Err("unknown key 'Return'".into());
            }
            if args["element_id"] == 7 {
                m.field_keys += 1;
                if m.field_keys < 2 {
                    return Err("action 'key' refused: the element is disabled".into());
                }
                return Ok(json!({ "mechanism": "native" }));
            }
            Ok(json!({ "mechanism": "dispatch" }))
        }
        other => Err(format!("unknown command: {other}")),
    }
}

fn handle(stream: TcpStream, state: Arc<Mutex<Mock>>) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(req): Result<Value, _> = serde_json::from_str(line.trim()) else { break };
        let cmd = req["cmd"].as_str().unwrap_or("").to_string();
        let args = req.get("args").cloned().unwrap_or(json!({}));
        let resp = match respond(&cmd, &args, &mut state.lock().unwrap()) {
            Ok(ok) => json!({ "id": req["id"], "ok": ok }),
            Err(err) => json!({ "id": req["id"], "err": err }),
        };
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
    }
}

fn mock() -> (App, Arc<Mutex<Mock>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(Mock::default()));
    let shared = state.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || handle(conn, state));
        }
    });
    (App::connect(addr).unwrap(), shared)
}

#[test]
fn app_press_key_sends_the_key_and_its_modifiers() {
    let (mut app, state) = mock();
    assert_eq!(app.press_key("ControlOrMeta+K"), "dispatch");
    assert_eq!(app.press_key("Escape"), "dispatch");
    let keys = state.lock().unwrap().keys.clone();
    assert_eq!(keys[0], json!({ "key": "K", "modifiers": ["ControlOrMeta"] }));
    assert_eq!(keys[1], json!({ "key": "Escape", "modifiers": [] }));
}

#[test]
fn locator_press_key_waits_for_the_field_then_presses_on_it() {
    let (mut app, state) = mock();
    assert_eq!(app.test_id("password").press_key("Enter"), "native");
    let m = state.lock().unwrap();
    assert!(m.looks >= 2, "it waited for the field to mount");
    assert_eq!(m.field_keys, 2, "a disabled field refused once, then took it");
    assert_eq!(m.keys.last().unwrap(), &json!({ "key": "Enter", "modifiers": [], "element_id": 7 }));
}

#[test]
fn an_unknown_key_fails_at_once() {
    let (mut app, _state) = mock();
    let started = Instant::now();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.test_id("password").press_key("Return");
    }))
    .unwrap_err();
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("unknown key 'Return'"), "{msg}");
    assert!(started.elapsed() < Duration::from_secs(3), "not retried until the timeout");
}
