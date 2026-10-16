//! Exercise the `App` / `Locator` / `SignalAssert` surface against a mock TCP
//! server speaking the bridge protocol — the locate → act → assert flow, the
//! auto-waiting (an element that mounts late, a control that is disabled for a
//! while, a label that updates late), element state, `nth`, navigation, and
//! that failures panic with what was last seen. (The `#[robot_test]` macro +
//! harness need a live app + relay, so they're verified end-to-end via
//! `idealyst test`; this covers the client logic in plain `cargo test`.)

use robot_test::App;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The mock app's state.
#[derive(Default)]
struct Mock {
    count: i64,
    /// `find_element` calls for "late": it mounts on the third look.
    late_looks: u32,
    /// `type_text` attempts on "field": refused (disabled) for the first two.
    field_attempts: u32,
    field_text: String,
    /// `find_element` calls for "counter" after a click; its label lags
    /// one look behind the count (a reactive update landing late).
    counter_lag: bool,
    path: String,
    /// `call_function` calls on "late.fn": registered from the third.
    late_fn_calls: u32,
    /// `poll_function` calls on the pending "demo.slow" call.
    slow_polls: u32,
}

/// A mock app:
/// - `inc` (Button 7) bumps `count`; `counter` (Text 3) shows it, one look late;
/// - `late` (View 5) mounts on the third look;
/// - `field` (TextInput 11) is disabled for two attempts, then takes text;
/// - two TextInputs by role (11, 12);
/// - one navigator, at `path`;
/// - robot functions: `demo.count` (sync), `demo.slow` (async, done on the
///   third poll), `late.fn` (registers on the third call), `demo.fail`.
fn spawn_mock_app() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(Mock { path: "/".into(), ..Default::default() }));
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || handle(conn, state));
        }
    });
    addr
}

fn field(m: &Mock, id: u64) -> Value {
    let disabled = id == 11 && m.field_attempts < 2;
    json!({ "id": id, "kind": "TextInput", "label": "Email", "test_id": if id == 11 { "field" } else { "other" },
            "state": { "value": if id == 11 { m.field_text.clone() } else { String::new() }, "disabled": disabled, "focused": false } })
}

fn respond(cmd: &str, args: &Value, m: &mut Mock) -> Result<Value, String> {
    Ok(match cmd {
        "ping" => json!("pong"),
        "find_element" => match args.get("test_id").and_then(|v| v.as_str()) {
            Some("inc") => json!({ "id": 7, "kind": "Button", "label": "+ increment", "test_id": "inc", "state": { "disabled": false } }),
            Some("counter") => {
                let shown = if m.counter_lag { m.count - 1 } else { m.count };
                m.counter_lag = false;
                json!({ "id": 3, "kind": "Text", "label": format!("Counter: {shown}"), "test_id": "counter" })
            }
            Some("late") => {
                m.late_looks += 1;
                if m.late_looks >= 3 { json!({ "id": 5, "kind": "View", "label": null, "test_id": "late" }) } else { Value::Null }
            }
            Some("field") => field(m, 11),
            // Mounted, but its overlay is closed: not visible.
            Some("closed") => json!({ "id": 20, "kind": "View", "label": null, "test_id": "closed", "visible": false }),
            Some("below") => json!({ "id": 21, "kind": "View", "label": null, "test_id": "below", "visible": true, "in_viewport": false }),
            _ => Value::Null,
        },
        "find_all_elements" => match args.get("kind").and_then(|v| v.as_str()) {
            Some("TextInput") => json!([field(m, 11), field(m, 12)]),
            _ => json!([]),
        },
        "click" => match args["element_id"].as_u64() {
            Some(7) => {
                m.count += 1;
                m.counter_lag = true;
                json!("ok")
            }
            _ => return Err("action 'click' not available on this element".into()),
        },
        "type_text" => {
            if args["element_id"] == json!(11) {
                m.field_attempts += 1;
                if m.field_attempts <= 2 {
                    return Err("action 'type_text' refused: the element is disabled".into());
                }
                m.field_text = args["text"].as_str().unwrap_or("").into();
            }
            json!("ok")
        }
        // Watched values come back as a JSON string of the Debug form.
        "read_signal" if args["name"] == "count" => json!(m.count.to_string()),
        "read_signal" => return Err("no such signal".into()),
        "list_navigators" => json!([{ "nav_id": 1, "type_name": "stack_navigator", "active_route": "home",
                                      "active_path": m.path, "depth": 1, "can_go_back": false,
                                      "is_current": true, "controllable": true }]),
        "navigate" => {
            m.path = args["path"].as_str().unwrap_or("/").into();
            json!("ok")
        }
        "open_link" => {
            let url = args["url"].as_str().unwrap_or("");
            m.path = url.split_once("://").map(|(_, rest)| format!("/{rest}")).unwrap_or_default();
            json!(true)
        }
        "list_functions" => json!([
            { "name": "demo.count", "doc": "Counts.", "args": [{ "name": "by", "type": "u32" }], "returns": "u32", "async": false },
            { "name": "demo.slow", "doc": "", "args": [], "returns": "u32", "async": true },
        ]),
        "call_function" => match args["name"].as_str() {
            Some("demo.count") => json!({ "done": true, "value": args["args"]["by"].as_u64().unwrap_or(0) * 10 }),
            Some("demo.slow") => json!({ "done": false, "call": 9 }),
            Some("demo.fail") => return Err("the inbox is full".into()),
            Some("late.fn") => {
                m.late_fn_calls += 1;
                if m.late_fn_calls < 3 {
                    return Err("no function named 'late.fn'; registered: [demo.count]".into());
                }
                json!({ "done": true, "value": "late" })
            }
            other => return Err(format!("no function named '{}'; registered: [demo.count]", other.unwrap_or(""))),
        },
        "poll_function" if args["call"] == 9 => {
            m.slow_polls += 1;
            if m.slow_polls < 3 { json!({ "done": false }) } else { json!({ "done": true, "value": 7 }) }
        }
        "poll_function" => return Err("no pending call".into()),
        _ => return Err(format!("unknown command: {cmd}")),
    })
}

fn handle(stream: TcpStream, state: Arc<Mutex<Mock>>) {
    let mut writer = stream.try_clone().unwrap();
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(req): Result<Value, _> = serde_json::from_str(line.trim()) else { break };
        let id = req.get("id").cloned().unwrap_or(json!(0));
        let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        let args = req.get("args").cloned().unwrap_or(json!({}));
        let resp = match respond(cmd, &args, &mut state.lock().unwrap()) {
            Ok(ok) => json!({ "id": id, "ok": ok }),
            Err(err) => json!({ "id": id, "err": err }),
        };
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}

fn app() -> App {
    App::connect(spawn_mock_app()).unwrap()
}

fn panic_message(f: impl FnOnce()) -> String {
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn locate_act_assert_flow() {
    let mut app = app();
    app.test_id("counter").assert_text("Counter: 0");
    app.signal("count").assert_eq(0);
    app.test_id("inc").click();
    app.test_id("inc").click();
    // The label lags one look behind each click: the assertion waits.
    app.test_id("counter").assert_text("Counter: 2");
    app.signal("count").assert_eq(2);
    app.test_id("counter").assert_text_contains("Counter:");
}

/// An element that mounts after a couple of looks is found — no sleep in
/// the test.
#[test]
fn locating_waits_for_an_element_to_mount() {
    let mut app = app();
    app.test_id("late").assert_visible();
}

/// A disabled control refuses; the action waits for it to be enabled (as
/// Playwright waits for an element to be actionable), then takes effect.
#[test]
fn an_action_waits_for_a_disabled_control() {
    let mut app = app();
    app.test_id("field").assert_disabled();
    app.test_id("field").fill("me@example.com");
    app.test_id("field").assert_enabled();
    app.test_id("field").assert_value("me@example.com");
    let state = app.test_id("field").state();
    assert_eq!(state.disabled, Some(false));
    assert_eq!(state.focused, Some(false));
}

#[test]
fn nth_and_count_pick_among_matches() {
    let mut app = app();
    assert_eq!(app.role("TextInput").count(), 2);
    app.role("TextInput").assert_count(2);
    app.role("TextInput").nth(1).assert_value("");
    let msg = panic_message(|| {
        app.set_timeout(Duration::from_millis(200));
        app.role("TextInput").nth(5).assert_visible();
    });
    assert!(msg.contains("fewer than 6 elements match"), "got: {msg}");
}

#[test]
fn navigate_and_wait_for_the_path() {
    let mut app = app();
    app.assert_path("/");
    app.navigate("/settings");
    app.assert_path("/settings");
    assert_eq!(app.navigators()[0].active_path.as_deref(), Some("/settings"));
    assert!(app.open_link("myapp://inbox"));
    app.assert_path("/inbox");
}

/// A missing element fails after the timeout, saying what it last saw.
#[test]
fn missing_element_panics_after_the_timeout() {
    let mut app = app();
    app.set_timeout(Duration::from_millis(300));
    let start = Instant::now();
    let msg = panic_message(|| app.test_id("does-not-exist").click());
    assert!(start.elapsed() >= Duration::from_millis(300), "it waited");
    assert!(msg.contains("no element matches") && msg.contains("still failing"), "got: {msg}");
}

/// An action the element doesn't have fails at once — it won't appear by
/// waiting.
#[test]
fn an_unavailable_action_fails_without_waiting() {
    let mut app = app();
    app.set_timeout(Duration::from_secs(10));
    let start = Instant::now();
    let msg = panic_message(|| app.test_id("counter").click());
    assert!(start.elapsed() < Duration::from_secs(2), "no wait for an action that can't come");
    assert!(msg.contains("not available"), "got: {msg}");
}

#[test]
fn wrong_signal_value_panics() {
    let mut app = app();
    app.set_timeout(Duration::from_millis(200));
    let msg = panic_message(|| app.signal("count").assert_eq(99));
    assert!(msg.contains("the value is \"0\""), "got: {msg}");
}

#[test]
fn raw_call_reaches_any_verb() {
    let mut app = app();
    assert_eq!(app.call("ping", json!({})), json!("pong"));
    assert!(app.try_call("no_such_verb", json!({})).is_err());
}

/// `assert_visible` / `assert_hidden` read the element's `visible`: a
/// mounted element hidden by its overlay is hidden, not visible.
#[test]
fn visibility_assertions_follow_visible_not_mounted() {
    let mut app = app();
    app.test_id("closed").assert_hidden();
    assert!(!app.test_id("closed").is_visible());
    app.set_timeout(Duration::from_millis(200));
    let msg = panic_message(|| app.test_id("closed").assert_visible());
    assert!(msg.contains("mounted but not visible"), "got: {msg}");
    app.test_id("below").assert_visible();
    let msg = panic_message(|| app.test_id("below").assert_in_viewport());
    assert!(msg.contains("outside the window"), "got: {msg}");
}

/// A robot function is called by name; an async one is polled until it is
/// done; one that registers late is waited for; a failure is the
/// function's own message.
#[test]
fn robot_functions_are_called_awaited_and_waited_for() {
    let mut app = app();
    assert_eq!(app.call_function("demo.count", json!({ "by": 3 })), json!(30));
    assert_eq!(app.call_function("demo.slow", json!({})), json!(7), "polled until done");
    assert_eq!(app.call_function("late.fn", json!({})), json!("late"), "waited for registration");
    assert_eq!(app.try_call_function("demo.fail", json!({})), Err("the inbox is full".into()));

    let fns = app.functions();
    assert_eq!(fns.len(), 2);
    assert_eq!(fns[0].name, "demo.count");
    assert_eq!(fns[0].args[0].ty, "u32");
    assert!(fns[1].is_async);

    app.set_timeout(Duration::from_millis(200));
    let msg = panic_message(|| {
        app.call_function("never.registered", json!({}));
    });
    assert!(msg.contains("still not registered"), "got: {msg}");
}
