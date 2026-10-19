//! `screenshot` over CDP, with no real browser: a fake DevTools endpoint
//! (the `/json/list` HTTP route plus one WebSocket per page target) stands
//! in for a Chromium the dev session launched, and a fake web app dials the
//! relay with a pairing nonce. Pins that the relay captures the tab whose
//! nonce matches the driven app's — not merely the first tab it finds —
//! and that `auto` falls back to the page's own capture when no browser
//! has that tab.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tungstenite::Message;

/// A 1×1 PNG: what the fake page answers when the relay forwards.
const PAGE_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGNgAAIAAAUAAen63NgAAAAASUVORK5CYII=";

/// A PNG head claiming `w`×`h` — enough for the relay's size read.
fn png_head(w: u32, h: u32) -> String {
    use base64::Engine as _;
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&w.to_be_bytes());
    png.extend_from_slice(&h.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    base64::engine::general_purpose::STANDARD.encode(png)
}

/// One fake page target: answers `Runtime.evaluate` with its nonce and
/// `Page.captureScreenshot` with a PNG of `size`, emitting an unrelated
/// event first each time (a real target interleaves events with replies).
fn spawn_target(nonce: Option<&'static str>, size: (u32, u32)) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let Ok(mut ws) = tungstenite::accept(stream) else { return };
                while let Ok(msg) = ws.read() {
                    let Message::Text(text) = msg else { continue };
                    let req: Value = serde_json::from_str(text.as_str()).unwrap();
                    let result = match req["method"].as_str().unwrap() {
                        "Runtime.evaluate" => {
                            assert_eq!(
                                req["params"]["expression"],
                                format!("globalThis.{}", robot_relay::cdp::NONCE_GLOBAL)
                            );
                            match nonce {
                                Some(n) => json!({ "result": { "type": "string", "value": n } }),
                                None => json!({ "result": { "type": "undefined" } }),
                            }
                        }
                        "Page.captureScreenshot" => json!({ "data": png_head(size.0, size.1) }),
                        other => panic!("unexpected CDP method {other}"),
                    };
                    let event = json!({ "method": "Page.frameNavigated", "params": {} });
                    ws.send(Message::Text(event.to_string().into())).unwrap();
                    let reply = json!({ "id": req["id"], "result": result });
                    ws.send(Message::Text(reply.to_string().into())).unwrap();
                }
            });
        }
    });
    addr
}

/// The browser's DevTools HTTP endpoint: `/json/list` naming `targets`
/// (plus a non-page target, which the relay must skip).
fn spawn_devtools(targets: Vec<SocketAddr>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut list: Vec<Value> = targets
        .iter()
        .enumerate()
        .map(|(i, a)| json!({ "type": "page", "id": format!("T{i}"), "webSocketDebuggerUrl": format!("ws://{a}/devtools/page/T{i}") }))
        .collect();
    list.insert(0, json!({ "type": "service_worker", "webSocketDebuggerUrl": "ws://127.0.0.1:1/x" }));
    let body = Value::Array(list).to_string();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            assert!(String::from_utf8_lossy(&head).starts_with("GET /json/list "));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

/// A `.idealyst`-like dir holding one profile per DevTools port.
fn profiles_dir(name: &str, ports: &[u16]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay_cdp_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (i, port) in ports.iter().enumerate() {
        let profile = dir.join(format!("headless-client-profile-{i}"));
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("DevToolsActivePort"), format!("{port}\n/devtools/browser/b{i}\n")).unwrap();
    }
    dir
}

/// A web app that pairs with `nonce` and answers a forwarded `screenshot`
/// with its own (DOM) capture, echoing the `source` it was asked for.
fn spawn_page(ws_addr: SocketAddr, nonce: &'static str) {
    std::thread::spawn(move || {
        let (mut ws, _) = tungstenite::connect(format!("ws://{ws_addr}/")).unwrap();
        let hello = json!({ "hello": { "platform": "web", "label": "Chrome 131", "nonce": nonce } });
        ws.send(Message::Text(hello.to_string().into())).unwrap();
        while let Ok(msg) = ws.read() {
            let Message::Text(text) = msg else { continue };
            let v: Value = serde_json::from_str(text.as_str()).unwrap();
            if v["cmd"] == "screenshot" {
                let reply = json!({ "id": v["id"], "ok": {
                    "png_base64": PAGE_PNG, "width": 1, "height": 1, "source": "dom",
                    "asked": v["args"]["source"],
                }});
                ws.send(Message::Text(reply.to_string().into())).unwrap();
            }
        }
    });
}

fn start_relay(cdp_profiles_dir: Option<&Path>) -> robot_relay::RelayHandle {
    robot_relay::start(robot_relay::RelayConfig {
        register: false,
        cdp_profiles_dir: cdp_profiles_dir.map(Path::to_path_buf),
        ..Default::default()
    })
    .unwrap()
}

fn call(relay: &robot_relay::RelayHandle, cmd: &str, args: Value) -> Value {
    let mut stream = TcpStream::connect(relay.tcp_addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    writeln!(stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// Wait until the page's `hello` has reached the relay.
fn await_hello(relay: &robot_relay::RelayHandle) {
    for _ in 0..100 {
        if relay.apps().iter().any(|a| a.platform.is_some()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the page never said hello");
}

#[test]
fn cdp_captures_the_tab_whose_nonce_is_the_driven_apps() {
    // Two browsers: one with an unrelated page and a tab of another app
    // session; the driven tab is in the second.
    let other_app = spawn_target(Some("someone-else"), (10, 10));
    let not_idealyst = spawn_target(None, (11, 11));
    let driven = spawn_target(Some("n-42"), (1280, 720));
    let dir = profiles_dir("match", &[spawn_devtools(vec![not_idealyst, other_app]), spawn_devtools(vec![driven])]);

    let relay = start_relay(Some(&dir));
    spawn_page(relay.ws_addr, "n-42");
    await_hello(&relay);

    let resp = call(&relay, "screenshot", json!({ "source": "cdp" }));
    assert_eq!(resp["ok"]["source"], "cdp", "{resp}");
    assert_eq!((resp["ok"]["width"].as_u64(), resp["ok"]["height"].as_u64()), (Some(1280), Some(720)));

    // `auto` prefers CDP when the tab is reachable.
    let auto = call(&relay, "screenshot", json!({}));
    assert_eq!(auto["ok"]["source"], "cdp", "{auto}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn auto_falls_back_to_the_page_when_no_browser_has_the_tab() {
    // The user's own browser: the page is connected, but no launched
    // browser holds it.
    let dir = profiles_dir("nomatch", &[spawn_devtools(vec![spawn_target(Some("someone-else"), (5, 5))])]);
    let relay = start_relay(Some(&dir));
    spawn_page(relay.ws_addr, "mine");
    await_hello(&relay);

    let auto = call(&relay, "screenshot", json!({ "source": "auto" }));
    assert_eq!(auto["ok"]["source"], "dom", "forwarded to the page: {auto}");
    assert_eq!(auto["ok"]["asked"], "auto", "the page is told the source it was asked for");

    // Explicit `cdp` does not fall back: it says why and what to use.
    let cdp = call(&relay, "screenshot", json!({ "source": "cdp" }));
    let err = cdp["err"].as_str().unwrap_or_else(|| panic!("an error: {cdp}"));
    assert!(err.contains("not in a browser this dev session launched"), "{err}");
    assert!(err.contains("\"tab\"") && err.contains("\"dom\""), "names the alternatives: {err}");

    // `dom` and `tab` always go to the page.
    let dom = call(&relay, "screenshot", json!({ "source": "dom" }));
    assert_eq!(dom["ok"]["asked"], "dom", "{dom}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stale_devtools_port_file_is_skipped() {
    // A profile whose browser is gone: its port refuses connections.
    let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let driven = spawn_target(Some("n-1"), (2, 3));
    let dir = profiles_dir("stale", &[gone, spawn_devtools(vec![driven])]);
    let relay = start_relay(Some(&dir));
    spawn_page(relay.ws_addr, "n-1");
    await_hello(&relay);

    let resp = call(&relay, "screenshot", json!({ "source": "cdp" }));
    assert_eq!(resp["ok"]["source"], "cdp", "{resp}");
    std::fs::remove_dir_all(&dir).ok();
}
