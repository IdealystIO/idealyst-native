//! Exercise the `App` / `Locator` / `SignalAssert` surface against a mock TCP
//! server speaking the bridge protocol — proving the locate → act → assert flow
//! and that a failed assertion panics. (The `#[robot_test]` macro + harness need
//! a live app + relay, so they're verified end-to-end via `idealyst test`; this
//! covers the pure client logic in plain `cargo test`.)

use robot_test::App;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// A mock app: an Increment button (`test_id="inc"`) whose click bumps a `count`
/// signal; a `counter` text whose label tracks the count.
fn spawn_mock_app() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicI64::new(0));
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let count = count.clone();
            std::thread::spawn(move || handle(conn, count));
        }
    });
    addr
}

fn handle(stream: TcpStream, count: Arc<AtomicI64>) {
    let mut writer = stream.try_clone().unwrap();
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(req): Result<Value, _> = serde_json::from_str(line.trim()) else {
            break;
        };
        let id = req.get("id").cloned().unwrap_or(json!(0));
        let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        let args = req.get("args").cloned().unwrap_or(json!({}));
        let resp = match cmd {
            "ping" => json!({ "id": id, "ok": "pong" }),
            "find_element" => {
                let test_id = args.get("test_id").and_then(|v| v.as_str());
                match test_id {
                    Some("inc") => {
                        json!({ "id": id, "ok": { "id": 7, "kind": "Button", "label": "+ increment", "test_id": "inc" } })
                    }
                    Some("counter") => json!({ "id": id, "ok": {
                        "id": 3, "kind": "Text",
                        "label": format!("Counter: {}", count.load(Ordering::SeqCst)),
                        "test_id": "counter"
                    } }),
                    _ => json!({ "id": id, "ok": Value::Null }),
                }
            }
            "click" => {
                if args.get("element_id") == Some(&json!(7)) {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                json!({ "id": id, "ok": "ok" })
            }
            // Watched values come back as a JSON string of the Debug form.
            "read_signal" => {
                if args.get("name").and_then(|v| v.as_str()) == Some("count") {
                    json!({ "id": id, "ok": count.load(Ordering::SeqCst).to_string() })
                } else {
                    json!({ "id": id, "err": "no such signal" })
                }
            }
            _ => json!({ "id": id, "err": "unknown verb" }),
        };
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}

#[test]
fn locate_act_assert_flow() {
    let mut app = App::connect(spawn_mock_app()).unwrap();

    app.test_id("counter").assert_text("Counter: 0");
    app.signal("count").assert_eq(0);

    app.test_id("inc").click();
    app.test_id("inc").click();

    app.test_id("counter").assert_text("Counter: 2");
    app.signal("count").assert_eq(2);
    app.test_id("counter").assert_text_contains("Counter:");
}

#[test]
fn missing_element_panics_clearly() {
    let mut app = App::connect(spawn_mock_app()).unwrap();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.test_id("does-not-exist").click();
    }))
    .unwrap_err();
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("matched no element"), "got: {msg}");
}

#[test]
fn wrong_signal_value_panics() {
    let mut app = App::connect(spawn_mock_app()).unwrap();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.signal("count").assert_eq(99);
    }))
    .unwrap_err();
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("actual value was"), "got: {msg}");
}
