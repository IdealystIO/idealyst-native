//! Exercise the `App` / `Locator` / `SignalAssert` surface against a mock TCP
//! server speaking the bridge protocol — the locate → act → assert flow, the
//! auto-waiting (an element that mounts late, a control that is disabled for a
//! while, a label that updates late), element state, `nth`, navigation, and
//! that failures panic with what was last seen. (The `#[robot_test]` macro +
//! harness need a live app + relay, so they're verified end-to-end via
//! `idealyst test`; this covers the client logic in plain `cargo test`.)

use robot_test::{App, Cookie, Selector};
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
    /// `tap_native` attempts on "cta": covered by a scrim for two.
    cta_attempts: u32,
    /// Native taps "cta" received.
    cta_taps: u32,
    /// Replies still to answer as a busy relay does (`app response timed
    /// out`) instead of answering.
    busy_replies: u32,
    /// Replies still to answer as a relay with no app does.
    no_app_replies: u32,
    /// `(verb, n)`: answer `verb` busy `n` more times.
    busy_verb: Option<(String, u32)>,
    /// `(verb, n)`: answer `verb` as a relay with no app `n` more times.
    no_app_verb: Option<(String, u32)>,
    /// Delay the next reply this long (a client-side I/O timeout).
    delay_next: Option<Duration>,
    /// Answer `find_all_elements` as an app from before combined queries:
    /// the first field present wins (test_id, label, label_contains, kind).
    old_app: bool,
    /// Every request: `(cmd, args)`.
    requests: Vec<(String, Value)>,
    /// The window's viewport; `set_viewport` lands after one look.
    viewport: (f64, f64),
    pending_viewport: Option<(f64, f64)>,
    /// `set_viewport` answers unsupported (a browser tab, a phone).
    viewport_fixed: bool,
    /// "tab" (50, off the window to the right) and "clipped" (51, in the
    /// window but clipped by its scroll view) after `scroll_into_view`.
    tab_revealed: bool,
    clipped_revealed: bool,
    /// Native taps "tab" / "clipped" received.
    revealed_taps: u32,
    /// `get_platform`'s answer; `None` answers as an app from before it.
    platform: Option<&'static str>,
    /// `open_link` levels the deep link has reached (one more per resume,
    /// as nested navigators that mount late are moved).
    link_levels: usize,
    /// The browser's cookies (`set_cookie` / `get_cookies` / `clear_cookies`).
    cookies: Vec<Value>,
    /// `load_url` reboots: the path becomes the URL's, the count resets.
    /// `false` answers as a native app the dev session can't relaunch.
    loads_web: bool,
    /// `get_style` calls on "todo" (Text 60): struck through from the third.
    style_looks: u32,
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
    spawn_mock_app_with(Mock::default()).0
}

/// [`spawn_mock_app`] starting from `mock`, with a handle on its state.
fn spawn_mock_app_with(mock: Mock) -> (SocketAddr, Arc<Mutex<Mock>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(Mock { path: "/".into(), viewport: (1280.0, 800.0), ..mock }));
    let shared = state.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || handle(conn, state));
        }
    });
    (addr, shared)
}

fn field(m: &Mock, id: u64) -> Value {
    let disabled = id == 11 && m.field_attempts < 2;
    json!({ "id": id, "kind": "TextInput", "label": "Email", "test_id": if id == 11 { "field" } else { "other" },
            "state": { "value": if id == 11 { m.field_text.clone() } else { String::new() }, "disabled": disabled, "focused": false } })
}

/// The login form the selector tests query: a caption Text "Email" and two
/// "Email" fields — one in the dialog (View 40), one after it.
fn login_form() -> Vec<Value> {
    vec![
        json!({ "id": 40, "kind": "View", "label": null, "test_id": "dialog" }),
        json!({ "id": 41, "kind": "Text", "label": "Email", "test_id": null }),
        json!({ "id": 42, "kind": "TextInput", "label": "Email", "test_id": "dialog-email" }),
        json!({ "id": 43, "kind": "TextInput", "label": "Email", "test_id": "footer-email" }),
    ]
}

/// `find_all_elements` over [`login_form`]: every field ANDed, `within` the
/// dialog meaning ids 41–42 — or, for an old app, the first field only.
fn find_in_form(args: &Value, old_app: bool) -> Vec<Value> {
    let fields = ["test_id", "label", "label_contains", "kind"];
    let used: Vec<&str> = if old_app {
        fields.iter().copied().filter(|f| !args[*f].is_null()).take(1).collect()
    } else {
        fields.to_vec()
    };
    login_form()
        .into_iter()
        .filter(|el| {
            used.iter().all(|f| match (&args[*f], *f) {
                (Value::Null, _) => true,
                (want, "label_contains") => {
                    let label = el["label"].as_str().unwrap_or("");
                    match want {
                        Value::Array(subs) => subs.iter().all(|s| label.contains(s.as_str().unwrap())),
                        sub => label.contains(sub.as_str().unwrap()),
                    }
                }
                (want, f) => &el[f] == want,
            })
        })
        .filter(|el| old_app || args["within"].is_null() || (args["within"]["test_id"] == "dialog" && [41, 42].contains(&el["id"].as_u64().unwrap())))
        .collect()
}

fn respond(cmd: &str, args: &Value, m: &mut Mock) -> Result<Value, String> {
    m.requests.push((cmd.to_string(), args.clone()));
    if m.busy_replies > 0 && cmd != "ping" {
        m.busy_replies -= 1;
        return Err(format!("app response timed out: the relay waited 5000 ms for the app to answer `{cmd}` (the app is connected but busy or hung)"));
    }
    if let Some((verb, n)) = &mut m.busy_verb {
        if verb == cmd && *n > 0 {
            *n -= 1;
            return Err(format!("app response timed out: the relay waited 5000 ms for the app to answer `{cmd}` (the app is connected but busy or hung)"));
        }
    }
    if let Some((verb, n)) = &mut m.no_app_verb {
        if verb == cmd && *n > 0 {
            *n -= 1;
            return Err("no app connected to the relay".into());
        }
    }
    if m.no_app_replies > 0 && cmd != "ping" {
        m.no_app_replies -= 1;
        return Err("no app connected to the relay".into());
    }
    Ok(match cmd {
        "ping" => json!("pong"),
        "load_url" if !m.loads_web => {
            return Err("load_url is unsupported on ios: booting a native app at a URL relaunches it".into())
        }
        "load_url" => {
            let url = args["url"].as_str().unwrap_or_default();
            m.path = url.to_string();
            m.count = 0;
            json!({ "url": format!("http://127.0.0.1:3000{url}") })
        }
        "set_cookie" => {
            let name = args["name"].clone();
            m.cookies.retain(|c| c["name"] != name);
            let mut c = args.clone();
            c.as_object_mut().unwrap().remove("url");
            c["domain"] = json!("127.0.0.1");
            c["path"] = json!("/");
            m.cookies.push(c);
            json!({ "name": name })
        }
        "get_cookies" => json!(m.cookies),
        "get_style" if args["element_id"] == json!(60) => {
            m.style_looks += 1;
            let deco = if m.style_looks >= 3 { "line-through" } else { "none" };
            json!({ "source": "framework", "style": { "text_decoration": deco, "font_weight": 700, "color": [1.0, 0.0, 0.0, 1.0] } })
        }
        "clear_cookies" => {
            match args.get("name") {
                Some(name) => m.cookies.retain(|c| &c["name"] != name),
                None => m.cookies.clear(),
            }
            json!("ok")
        }
        "find_element" => match args.get("test_id").and_then(|v| v.as_str()) {
            Some("inc") => json!({ "id": 7, "kind": "Button", "label": "+ increment", "test_id": "inc", "state": { "disabled": false } }),
            Some("todo") => json!({ "id": 60, "kind": "Text", "label": "Buy milk", "test_id": "todo" }),
            Some("email") => json!({ "id": 61, "kind": "TextInput", "label": null, "test_id": "email",
                                     "state": { "value": "", "keyboard": "email", "native_keyboard": "email" } }),
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
            Some("cta") => json!({ "id": 30, "kind": "Button", "label": format!("Taps: {}", m.cta_taps), "test_id": "cta", "visible": true, "state": { "disabled": false } }),
            Some("below") => json!({ "id": 21, "kind": "View", "label": null, "test_id": "below", "visible": true, "in_viewport": false }),
            Some("tab") => json!({ "id": 50, "kind": "Pressable", "label": null, "test_id": "tab", "visible": true, "in_viewport": m.tab_revealed }),
            Some("clipped") => json!({ "id": 51, "kind": "Pressable", "label": null, "test_id": "clipped", "visible": true, "in_viewport": true }),
            _ => Value::Null,
        },
        "find_all_elements" if args.get("label").is_some() || args.get("within").is_some() || args.get("label_contains").is_some() => {
            json!(find_in_form(args, m.old_app))
        }
        "find_all_elements" => match args.get("kind").and_then(|v| v.as_str()) {
            Some("TextInput") => json!([field(m, 11), field(m, 12)]),
            _ => json!([]),
        },
        "count_elements" => json!(if args["kind"] == "Button" { 3 } else { 0 }),
        "get_viewport" => {
            let (width, height) = m.viewport;
            if let Some(next) = m.pending_viewport.take() {
                m.viewport = next;
            }
            let bp = if width >= 1280.0 { "xl" } else if width >= 768.0 { "md" } else { "xs" };
            json!({ "width": width, "height": height, "breakpoint": bp })
        }
        "set_viewport" if m.viewport_fixed => {
            return Err("set_viewport is unsupported on this platform: the app cannot resize its own window here".into())
        }
        "set_viewport" => {
            m.pending_viewport = Some((args["width"].as_f64().unwrap(), args["height"].as_f64().unwrap()));
            json!("ok")
        }
        // "inc" is on a backend without native input; "cta" is covered by
        // a scrim for two attempts, then takes a native tap.
        "get_platform" => match m.platform {
            Some(p) => json!({ "platform": p }),
            None => return Err("unknown command: get_platform".into()),
        },
        "scroll_into_view" => match args["element_id"].as_u64() {
            Some(50) => {
                m.tab_revealed = true;
                json!({ "scrolled": [{ "element_id": 49, "x": 600.0, "y": 0.0 }] })
            }
            Some(51) if !m.clipped_revealed => {
                m.clipped_revealed = true;
                json!({ "scrolled": [{ "element_id": 49, "x": 120.0, "y": 0.0 }] })
            }
            _ => json!({ "scrolled": [] }),
        },
        // Real-input presses and right clicks land wherever they are sent.
        "press_native" | "release_native" | "hover_native" | "drag_native" | "context_click" => {
            json!({ "mechanism": "native" })
        }
        "tap_native" => match args["element_id"].as_u64() {
            Some(50) if !m.tab_revealed => {
                return Err("action 'tap_native' refused: the point (700, 20) is outside the window (scroll the element into view)".into())
            }
            Some(51) if !m.clipped_revealed => {
                return Err("action 'tap_native' refused: another element covers the point (390, 20) — View test_id=\"next\" would receive the tap".into())
            }
            Some(50 | 51) => {
                m.revealed_taps += 1;
                json!({ "mechanism": "native" })
            }
            Some(30) => {
                m.cta_attempts += 1;
                if m.cta_attempts <= 2 {
                    return Err("action 'tap_native' refused: another element covers the point (60, 20) — View test_id=\"scrim\" would receive the tap".into());
                }
                m.cta_taps += 1;
                json!({ "mechanism": "native" })
            }
            Some(3) => return Err("action 'click' not available on this element".into()),
            _ => return Err("action 'tap_native' unsupported: this backend has no native input".into()),
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
        "list_navigators" => {
            let (path, query) = m.path.split_once('?').unwrap_or((&m.path, ""));
            let query: serde_json::Map<String, Value> = query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect();
            json!([{ "nav_id": 1, "type_name": "stack_navigator", "active_route": "home",
                     "active_path": path, "query": query, "href": m.path, "depth": 1,
                     "can_go_back": false, "is_current": true, "controllable": true }])
        }
        "navigate" => {
            m.path = args["path"].as_str().unwrap_or("/").into();
            json!("ok")
        }
        // `myapp://projects/7/schedule` sits under navigators that mount
        // late: each call reaches one more level. `…/bogus` stops at
        // `/projects/7` for good. `myapp://gated` is claimed by an
        // interceptor. Anything else lands at once.
        "open_link" => {
            let url = args["url"].as_str().unwrap_or("");
            let target = url.split_once("://").map(|(_, rest)| format!("/{rest}")).unwrap_or_default();
            if target == "/gated" {
                return Ok(json!({ "reached": m.path, "complete": false, "routed": false }));
            }
            let segs: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();
            if segs.first() == Some(&"projects") {
                if args["resume"] != json!(true) {
                    m.link_levels = 0;
                }
                let reachable = if segs.last() == Some(&"bogus") { segs.len() - 1 } else { segs.len() };
                m.link_levels = (m.link_levels + 1).min(reachable);
                m.path = format!("/{}", segs[..m.link_levels].join("/"));
                return Ok(json!({ "reached": m.path, "complete": m.path == target, "routed": true }));
            }
            m.path = target;
            json!({ "reached": m.path.split('?').next().unwrap(), "complete": true, "routed": true })
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
        let delay = state.lock().unwrap().delay_next.take();
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
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

/// A deep link under navigators that mount late lands in ONE
/// `open_link` call: the client carries it on (`resume`) as each level
/// comes up, instead of stopping at the first.
#[test]
fn regression_open_link_waits_for_the_whole_nested_path() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    assert!(app.open_link("myapp://projects/7/schedule"));
    app.assert_path("/projects/7/schedule");
    let resumes = state.lock().unwrap().requests.iter().filter(|(c, a)| c == "open_link" && a["resume"] == true).count();
    assert_eq!(resumes, 2, "resumed until the last level showed");
}

/// A segment no screen matches: `open_link` fails naming where it got.
#[test]
fn open_link_names_the_path_it_reached_when_it_stops_short() {
    let mut app = app();
    app.set_timeout(Duration::from_millis(300));
    let outcome = app.try_open_link("myapp://projects/7/bogus");
    assert_eq!(
        outcome,
        robot_test::LinkOutcome { routed: true, reached: "/projects/7".into(), complete: false }
    );
    let msg = panic_message(|| {
        app.open_link("myapp://projects/7/bogus");
    });
    assert!(msg.contains("reached \"/projects/7\""), "got: {msg}");
    // A link an interceptor claims is not resumed, and reports false.
    assert!(!app.open_link("myapp://gated"));
}

/// The active screen's query rides beside the path-only `active_path`.
#[test]
fn navigator_query_and_href_assertions() {
    let mut app = app();
    assert!(app.open_link("myapp://assets?asset_tag=~7781"));
    app.assert_path("/assets");
    app.assert_query("asset_tag", "~7781");
    app.assert_href("/assets?asset_tag=~7781");
    let nav = &app.navigators()[0];
    assert_eq!(nav.query.get("asset_tag").map(String::as_str), Some("~7781"));
    app.set_timeout(Duration::from_millis(200));
    let msg = panic_message(|| app.assert_query("asset_tag", "~1"));
    assert!(msg.contains("the query is"), "got: {msg}");
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

/// `click` is a native tap first: it waits while something covers the
/// element, then taps; on a backend without native input it presses
/// through the handler. `tap_native` has no fallback.
#[test]
fn click_taps_natively_waits_for_a_cover_and_falls_back() {
    let mut app = app();
    app.test_id("cta").click();
    app.test_id("cta").assert_text("Taps: 1");
    assert_eq!(app.test_id("cta").tap_native(), "native");

    // "inc" has no native input: click falls back to the handler.
    app.test_id("inc").click();
    app.signal("count").assert_eq(1);
    let msg = panic_message(|| {
        app.test_id("inc").tap_native();
    });
    assert!(msg.contains("unsupported"), "got: {msg}");
}

/// Regression (CrewForge E2E report): a locator with a 20 s timeout panicked
/// at the first `find_all_elements failed: app response timed out` — the
/// lookup inside the wait loop called the panicking `App::call`, so a busy
/// app ended the wait at once. A busy reply is "not yet": the locator asks
/// again and succeeds within its timeout, as do `count()`, `label()` and
/// `assert_path`.
#[test]
fn regression_locator_waits_through_app_response_timed_out() {
    let (addr, mock) = spawn_mock_app_with(Mock { busy_replies: 3, ..Default::default() });
    let mut app = App::connect(addr).unwrap();
    app.test_id("inc").click();
    app.signal("count").assert_eq(1);

    mock.lock().unwrap().busy_replies = 2;
    assert_eq!(app.role("TextInput").count(), 2);
    mock.lock().unwrap().busy_replies = 2;
    assert_eq!(app.test_id("inc").label().as_deref(), Some("+ increment"));
    mock.lock().unwrap().busy_replies = 2;
    app.assert_path("/");
    mock.lock().unwrap().busy_replies = 2;
    assert_eq!(app.count("Button"), 3);
    // A relay that has no app for a moment (a reload) is waited out too.
    mock.lock().unwrap().no_app_replies = 2;
    app.test_id("counter").assert_text("Counter: 1");

    // Still busy at the deadline: fails with the last error, naming the
    // app as the side that timed out.
    app.set_timeout(Duration::from_millis(200));
    mock.lock().unwrap().busy_replies = 1_000;
    let msg = panic_message(|| app.test_id("counter").assert_visible());
    assert!(msg.contains("still failing after") && msg.contains("app response timed out"), "got: {msg}");
}

/// An ACTION the app did not answer may still run once the app is free, so
/// it is not sent again: it fails at once and says why. (A query in the same
/// spot is retried — see the test above.)
#[test]
fn an_action_the_app_did_not_answer_is_not_sent_twice() {
    let (addr, mock) = spawn_mock_app_with(Mock { busy_verb: Some(("click".into(), 1)), ..Default::default() });
    let mut app = App::connect(addr).unwrap();
    let started = Instant::now();
    let msg = panic_message(|| app.test_id("inc").dispatch_click());
    assert!(msg.contains("app response timed out") && msg.contains("not sent again"), "got: {msg}");
    assert!(started.elapsed() < Duration::from_secs(2), "failed at once, not after the timeout");
    let clicks = mock.lock().unwrap().requests.iter().filter(|(c, _)| c == "click").count();
    assert_eq!(clicks, 1, "sent once");

    // A relay with no app never delivered it: that one IS retried.
    mock.lock().unwrap().busy_verb = None;
    mock.lock().unwrap().no_app_replies = 0;
    let before = mock.lock().unwrap().count;
    mock.lock().unwrap().no_app_verb = Some(("click".into(), 2));
    app.test_id("inc").dispatch_click();
    assert_eq!(mock.lock().unwrap().count, before + 1);
}

/// When no reply reaches the client within its I/O timeout, the error says
/// the CLIENT gave up, and the late reply — when it does arrive — is skipped
/// rather than read as the answer to the next call.
#[test]
fn regression_a_reply_after_a_client_timeout_is_not_read_as_the_next_answer() {
    let (addr, mock) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.set_io_timeout(Duration::from_millis(150));
    // The first lookup's reply comes 400 ms late: the client times out and
    // asks again; the waiting locator still succeeds.
    mock.lock().unwrap().delay_next = Some(Duration::from_millis(400));
    app.test_id("counter").assert_text("Counter: 0");
    // The late `find_element` reply is in the stream ahead of this one; a
    // client that read it would hand `counter`'s JSON to `read_signal`.
    app.signal("count").assert_eq(0);

    let mut raw = robot_test::RobotClient::connect(addr).unwrap();
    raw.set_io_timeout(Duration::from_millis(100)).unwrap();
    mock.lock().unwrap().delay_next = Some(Duration::from_millis(300));
    let err = raw.call("ping", json!({})).unwrap_err();
    match robot_test::CallError::of(&err) {
        Some(robot_test::CallError::ClientTimedOut { verb, .. }) => assert_eq!(verb, "ping"),
        other => panic!("expected a client-side timeout, got {other:?}"),
    }
    assert!(err.to_string().contains("the client gave up waiting on the relay"), "{err}");
    assert!(robot_test::is_transient(&err));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(raw.call("ping", json!({})).unwrap(), json!("pong"), "the late reply was skipped");

    // The relay's busy reply is told apart from it.
    mock.lock().unwrap().busy_replies = 1;
    let err = raw.call("find_element", json!({ "test_id": "inc" })).unwrap_err();
    assert!(matches!(robot_test::CallError::of(&err), Some(robot_test::CallError::AppTimedOut(_))), "{err}");
}

/// A request carries the relay's wait (`timeout_ms`) a little under the
/// client's own, so a busy app comes back from the relay first.
#[test]
fn the_io_timeout_is_sent_to_the_relay() {
    // Read the request envelope off a raw socket.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let probe = listener.local_addr().unwrap();
    let seen = std::thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
        let req: Value = serde_json::from_str(line.trim()).unwrap();
        let mut w = conn;
        w.write_all(format!("{}\n", json!({ "id": req["id"], "ok": "pong" })).as_bytes()).unwrap();
        req
    });
    let mut client = robot_test::RobotClient::connect(probe).unwrap();
    client.set_io_timeout(Duration::from_secs(20)).unwrap();
    client.call("ping", json!({})).unwrap();
    let req = seen.join().unwrap();
    assert_eq!(req["timeout_ms"], 19_500);
}

/// The filters go to the app as ONE query per look — kind AND label AND
/// within — and pick the field, not its caption, and only the dialog's.
#[test]
fn chained_filters_resolve_in_one_query() {
    let (addr, mock) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.role("TextInput").with_label("Email").assert_count(2);
    app.role("TextInput").with_label("Email").within(Selector::test_id("dialog")).assert_count(1);
    app.label("Email").with_role("TextInput").within(Selector::test_id("dialog")).assert_visible();
    assert_eq!(
        app.role("TextInput").with_label("Email").within(Selector::test_id("dialog")).label().as_deref(),
        Some("Email")
    );
    // A locator detached into a variable works as the container too.
    let dialog = app.test_id("dialog").selector();
    assert_eq!(app.role("TextInput").within(dialog.clone()).with_text("Em").with_text("ail").count(), 1);
    assert_eq!(app.locate(Selector::role("Text").with_label("Email").within(dialog)).count(), 1);

    mock.lock().unwrap().requests.clear();
    app.role("TextInput").with_label("Email").within(Selector::test_id("dialog").nth(0)).assert_visible();
    let reqs = mock.lock().unwrap().requests.clone();
    assert_eq!(reqs.len(), 1, "one round trip: {reqs:?}");
    assert_eq!(reqs[0].0, "find_all_elements");
    assert_eq!(
        reqs[0].1,
        json!({ "kind": "TextInput", "label": "Email", "within": { "test_id": "dialog", "nth": 0 } })
    );

    // The description names every filter.
    app.set_timeout(Duration::from_millis(150));
    let msg = panic_message(|| app.role("Button").with_label("Email").within(Selector::test_id("dialog")).assert_visible());
    assert!(msg.contains(r#"role("Button").with_label("Email").within(test_id("dialog"))"#), "got: {msg}");
}

/// Against an app from before combined queries (it honoured only the first
/// field, so `{kind, label}` came back with the caption), the client's own
/// check keeps the result to what was asked.
#[test]
fn chained_filters_hold_against_an_app_that_ignores_extra_fields() {
    let (addr, _mock) = spawn_mock_app_with(Mock { old_app: true, ..Default::default() });
    let mut app = App::connect(addr).unwrap();
    app.role("TextInput").with_label("Email").assert_count(2);
    app.role("Text").with_label("Email").assert_count(1);
}

/// `viewport()` reads size + breakpoint; `set_viewport` waits until the app
/// reports the new size; a platform that can't resize says so.
#[test]
fn viewport_is_read_and_set() {
    let (addr, mock) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    let v = app.viewport();
    assert_eq!((v.width, v.height, v.breakpoint.as_str()), (1280.0, 800.0, "xl"));
    app.set_viewport(390.0, 844.0);
    let v = app.viewport();
    assert_eq!((v.width, v.height, v.breakpoint.as_str()), (390.0, 844.0, "xs"));

    mock.lock().unwrap().viewport_fixed = true;
    let err = app.try_set_viewport(800.0, 600.0).unwrap_err();
    assert!(err.contains("unsupported on this platform"), "{err}");
    let msg = panic_message(|| app.set_viewport(800.0, 600.0));
    assert!(msg.contains("set_viewport(800, 600)") && msg.contains("unsupported"), "got: {msg}");
}

fn verbs(state: &Arc<Mutex<Mock>>) -> Vec<String> {
    state.lock().unwrap().requests.iter().map(|(cmd, _)| cmd.clone()).filter(|c| c != "find_element").collect()
}

/// CrewForge's phone tab strip: a tab off the window. `click` scrolls it
/// into view first, as Playwright does, then taps it — one scroll, one tap.
#[test]
fn click_scrolls_an_out_of_view_element_into_view_first() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.test_id("tab").click();
    assert_eq!(state.lock().unwrap().revealed_taps, 1);
    assert_eq!(verbs(&state), ["scroll_into_view", "tap_native"]);
}

/// In the window but clipped by its scroll view, the tap lands on a
/// neighbour (the robot reports it covered): `click` scrolls and taps
/// again at once, rather than waiting out a cover that is only a scroll.
#[test]
fn click_scrolls_an_element_its_scroll_view_clips_and_taps_again() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.set_timeout(Duration::from_millis(200));
    app.test_id("clipped").click();
    assert_eq!(state.lock().unwrap().revealed_taps, 1);
    assert_eq!(verbs(&state), ["tap_native", "scroll_into_view", "tap_native"]);
}

/// Opting out taps where the element is: off the window, it waits, then
/// fails, as it would for a user who doesn't scroll — per locator, or for
/// the whole app.
#[test]
fn no_scroll_and_auto_scroll_off_tap_without_scrolling() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.set_timeout(Duration::from_millis(200));
    let msg = panic_message(|| app.test_id("tab").no_scroll().click());
    assert!(msg.contains("outside the window"), "got: {msg}");
    app.set_auto_scroll(false);
    let msg = panic_message(|| {
        app.test_id("tab").tap_native();
    });
    assert!(msg.contains("outside the window"), "got: {msg}");
    assert!(!verbs(&state).iter().any(|v| v == "scroll_into_view"), "{:?}", verbs(&state));
    assert_eq!(state.lock().unwrap().revealed_taps, 0);

    // `scroll_into_view` itself still scrolls.
    app.test_id("tab").scroll_into_view();
    assert!(state.lock().unwrap().tab_revealed);
}

/// An app from before `scroll_into_view` answers "unknown command": the
/// tap goes ahead as it did before (waits for a cover, then taps).
#[test]
fn click_on_an_app_without_scroll_into_view_still_taps() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.test_id("cta").click();
    assert_eq!(state.lock().unwrap().cta_taps, 1);
}

/// `platform()` asks the app once; an app from before `get_platform`
/// answers `None` from `try_platform`.
#[test]
fn platform_is_asked_once_and_absent_on_an_old_app() {
    use robot_test::Platform;
    let (addr, state) = spawn_mock_app_with(Mock { platform: Some("ios"), ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    assert_eq!(app.platform(), Platform::Ios);
    assert!(app.platform().is_mobile());
    assert_eq!(verbs(&state).iter().filter(|v| *v == "get_platform").count(), 1);

    let mut old = App::connect(spawn_mock_app()).unwrap();
    assert_eq!(old.try_platform(), Ok(None));
    assert_eq!(Platform::from_id("windows"), Platform::Windows);
    assert_eq!(Platform::from_id("tvos"), Platform::Other("tvos".into()));
    assert!(Platform::Linux.is_native() && !Platform::Web.is_native());
}

/// CrewForge E2E report B5: right-click menus. `context_click` is the
/// platform's own context gesture — a secondary click (the `context_click`
/// verb) on desktop and web, scrolled into view first like a tap; on a
/// phone a long press: `press_native`, [`LONG_PRESS`] of real time, then
/// `release_native` on the same element.
#[test]
fn context_click_right_clicks_on_desktop_and_long_presses_on_a_phone() {
    let (addr, state) = spawn_mock_app_with(Mock { platform: Some("macos"), ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    assert_eq!(app.test_id("tab").context_click(), "native");
    let sent: Vec<String> = verbs(&state).into_iter().filter(|v| v != "get_platform").collect();
    assert_eq!(sent, ["scroll_into_view", "context_click"]);

    let (addr, state) = spawn_mock_app_with(Mock { platform: Some("ios"), ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    let started = std::time::Instant::now();
    app.test_id("cta").context_click();
    assert!(started.elapsed() >= robot_test::LONG_PRESS, "held for {:?}", started.elapsed());
    let m = state.lock().unwrap();
    let presses: Vec<(&str, &Value)> = m
        .requests
        .iter()
        .filter(|(c, _)| c.ends_with("_native"))
        .map(|(c, a)| (c.as_str(), a))
        .collect();
    assert_eq!(
        presses,
        [("press_native", &json!({ "element_id": 30 })), ("release_native", &json!({ "element_id": 30 }))]
    );
}

/// Android E2E (robot-e2e-demo): `long_press` on a row below the fold of
/// a phone's screen. Every real-input action scrolls its element into view
/// first, as `click` does — `long_press`, `press_native`, `hover_native`,
/// `drag_native` — and `release_native` scrolls nothing (it lifts where
/// the press went down). `.no_scroll()` and `set_auto_scroll(false)` opt
/// out of all of them.
#[test]
fn every_native_pointer_action_scrolls_its_element_into_view_first() {
    let (addr, state) = spawn_mock_app_with(Mock { platform: Some("android"), ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    let fresh = |state: &Arc<Mutex<Mock>>| {
        let mut m = state.lock().unwrap();
        m.tab_revealed = false;
        m.requests.clear();
    };
    app.test_id("tab").long_press(Duration::from_millis(10));
    assert_eq!(verbs(&state), ["scroll_into_view", "press_native", "release_native"]);
    fresh(&state);
    app.test_id("tab").press_native();
    assert_eq!(verbs(&state), ["scroll_into_view", "press_native"]);
    app.test_id("tab").release_native();
    assert_eq!(verbs(&state), ["scroll_into_view", "press_native", "release_native"]);
    fresh(&state);
    app.test_id("tab").hover_native();
    assert_eq!(verbs(&state), ["scroll_into_view", "hover_native"]);
    fresh(&state);
    app.test_id("tab").drag_native(&[(1.0, 2.0), (30.0, 2.0)]);
    assert_eq!(verbs(&state), ["scroll_into_view", "drag_native"]);
    let drag = state.lock().unwrap().requests.iter().find(|(c, _)| c == "drag_native").unwrap().1.clone();
    assert_eq!(drag, json!({ "points": [[1.0, 2.0], [30.0, 2.0]], "element_id": 50 }));

    fresh(&state);
    app.test_id("tab").no_scroll().press_native();
    app.set_auto_scroll(false);
    app.test_id("tab").hover_native();
    app.test_id("tab").long_press(Duration::from_millis(10));
    assert_eq!(verbs(&state), ["press_native", "hover_native", "press_native", "release_native"]);
}

/// `long_press(hold)` holds a real press for `hold`, and `secondary_click`
/// sends the right click on every platform, a phone included.
#[test]
fn long_press_holds_for_the_given_time_and_secondary_click_is_always_a_right_click() {
    let (addr, state) = spawn_mock_app_with(Mock { platform: Some("android"), ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(app.test_id("cta").long_press(Duration::from_millis(150)), "native");
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert_eq!(app.test_id("cta").secondary_click(), "native");
    let sent: Vec<String> = verbs(&state).into_iter().filter(|v| v != "get_platform").collect();
    assert_eq!(sent, ["press_native", "release_native", "context_click"]);
}

/// `load_url` boots the app again at the URL and outlasts the per-call I/O
/// timeout (the reload takes longer than an action's reply), then puts that
/// timeout back.
#[test]
fn load_url_waits_for_the_reboot_past_the_io_timeout() {
    let (addr, state) = spawn_mock_app_with(Mock { loads_web: true, count: 4, ..Mock::default() });
    let mut app = App::connect(addr).unwrap();
    app.set_io_timeout(Duration::from_millis(200));
    state.lock().unwrap().delay_next = Some(Duration::from_millis(500));
    assert_eq!(app.load_url("/projects/7"), "http://127.0.0.1:3000/projects/7");
    assert_eq!(state.lock().unwrap().count, 0, "a fresh app");
    app.assert_path("/projects/7");
    let sent = verbs(&state);
    let at = sent.iter().position(|v| v == "load_url").unwrap();
    assert_eq!(sent[at + 1], "ping", "waits for the new instance to answer: {sent:?}");

    // The short I/O timeout is back for every other call.
    state.lock().unwrap().delay_next = Some(Duration::from_millis(500));
    assert!(app.try_call("ping", json!({})).is_err(), "the 200 ms I/O timeout is restored");
}

/// A platform that can't be relaunched says so; `try_load_url` returns it.
#[test]
fn load_url_on_a_platform_that_cannot_relaunch_reports_it() {
    let mut app = app();
    let err = app.try_load_url("/x").unwrap_err().to_string();
    assert!(err.contains("unsupported on ios"), "{err}");
}

/// Cookies round-trip: set (HttpOnly included), read back, cleared by name
/// and all at once. `url` is only for setting and is not sent when unset.
#[test]
fn cookies_are_set_read_and_cleared() {
    let (addr, state) = spawn_mock_app_with(Mock::default());
    let mut app = App::connect(addr).unwrap();
    app.set_cookie(Cookie::new("device", "k1").http_only());
    app.set_cookie(Cookie { same_site: Some("Lax".into()), ..Cookie::new("theme", "dark") });
    let sent = state.lock().unwrap().requests.iter().find(|(c, _)| c == "set_cookie").unwrap().1.clone();
    assert_eq!(sent, json!({ "name": "device", "value": "k1", "http_only": true }));

    let cookies = app.cookies();
    let names: Vec<&str> = cookies.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["device", "theme"]);
    assert_eq!(cookies[0].http_only, Some(true));
    assert_eq!(cookies[1].same_site.as_deref(), Some("Lax"));
    assert_eq!(cookies[1].domain.as_deref(), Some("127.0.0.1"));

    app.clear_cookie("device");
    assert_eq!(app.cookies().len(), 1);
    app.clear_cookies();
    assert!(app.cookies().is_empty());
}

/// `style()` reads `get_style` into a typed `Style`; `assert_style` waits
/// for a style that lands late (a to-do struck through after its toggle).
#[test]
fn style_reads_and_waits_for_the_resolved_style() {
    let mut app = app();
    let s = app.test_id("todo").style();
    assert_eq!(s.source, "framework");
    assert_eq!(s.text_decoration, Some(robot_test::TextDecoration::None));
    assert!(s.is_bold());
    assert_eq!(s.color, Some([1.0, 0.0, 0.0, 1.0]));
    app.test_id("todo").assert_style(|s| s.is_line_through());
    let msg = panic_message(|| {
        app.set_timeout(Duration::from_millis(200));
        app.test_id("todo").assert_style(|s| s.is_underline());
    });
    assert!(msg.contains("assert_style") && msg.contains("LineThrough"), "{msg}");
}

/// A text field reports its keyboard kind.
#[test]
fn keyboard_reports_the_fields_keyboard_kind() {
    let mut app = app();
    assert_eq!(app.test_id("email").keyboard().as_deref(), Some("email"));
    app.test_id("email").assert_keyboard("email");
    let msg = panic_message(|| {
        app.set_timeout(Duration::from_millis(200));
        app.test_id("email").assert_keyboard("phone");
    });
    assert!(msg.contains("keyboard is Some(\"email\")"), "{msg}");
}
