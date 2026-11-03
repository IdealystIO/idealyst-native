//! `screenshot` source `cdp` against a real headless Chromium: the browser
//! writes `DevToolsActivePort` into its profile (as the dev session's
//! headless client does), a plain page dials the relay with a pairing
//! nonce, and the relay captures that page over CDP. Pins the parts the
//! fake-DevTools test (`cdp_screenshot.rs`) assumes: the port file's shape,
//! `/json/list`, a WebSocket opened with no `Origin`, `Runtime.evaluate`
//! of the nonce, and `Page.captureScreenshot`.
//!
//! Skips (and says so) when no Chromium-family browser is installed;
//! `IDEALYST_BROWSER` picks one explicitly.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const BROWSERS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
];

fn browser() -> Option<String> {
    if let Ok(p) = std::env::var("IDEALYST_BROWSER") {
        return Some(p);
    }
    BROWSERS.iter().find(|p| std::path::Path::new(p).exists()).map(|p| p.to_string())
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn cdp_captures_a_real_headless_tab_by_its_nonce() {
    let Some(browser) = browser() else {
        eprintln!("cdp_real_browser: no Chromium-family browser found; skipped");
        return;
    };
    let root = std::env::temp_dir().join(format!("relay_cdp_real_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profiles = root.join(".idealyst");
    let profile = profiles.join("headless-client-profile-1");
    std::fs::create_dir_all(&profile).unwrap();

    let relay = robot_relay::start(robot_relay::RelayConfig {
        register: false,
        screenshot_dir: Some(root.join("shots")),
        cdp_profiles_dir: Some(profiles.clone()),
        ..Default::default()
    })
    .unwrap();

    // What the wasm robot transport does on open, in plain JS.
    let page = format!(
        r#"<!doctype html><body style="margin:0;background:rgb(255,0,0)"><script>
        window.{global} = "real-nonce";
        var ws = new WebSocket("ws://127.0.0.1:{port}/");
        ws.onopen = function () {{
          ws.send(JSON.stringify({{ hello: {{ platform: "web", label: "test", nonce: "real-nonce" }} }}));
        }};
        </script></body>"#,
        global = robot_relay::cdp::NONCE_GLOBAL,
        port = relay.ws_addr.port(),
    );
    let page_path: PathBuf = root.join("page.html");
    std::fs::write(&page_path, page).unwrap();

    let _browser = Kill(
        Command::new(&browser)
            .args(["--headless=new", "--remote-debugging-port=0", "--window-size=640,480", "--no-first-run"])
            .args(["--no-default-browser-check", "--disable-gpu", "--force-device-scale-factor=1"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("file://{}", page_path.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the browser"),
    );

    let mut paired = false;
    for _ in 0..150 {
        if relay.apps().iter().any(|a| a.label.as_deref() == Some("test")) {
            paired = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(paired, "the page never dialed the relay");

    let mut stream = TcpStream::connect(relay.tcp_addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    writeln!(stream, "{}", json!({ "id": 1, "cmd": "screenshot", "args": { "source": "cdp" } })).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let resp: Value = serde_json::from_str(&line).unwrap();

    assert_eq!(resp["ok"]["source"], "cdp", "{}", &line[..line.len().min(400)]);
    assert_eq!(resp["ok"]["width"], 640, "the viewport's width at scale 1");
    let path = resp["ok"]["path"].as_str().expect("saved host-side");
    let png = std::fs::read(path).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    std::fs::remove_dir_all(&root).ok();
}

/// One request/response on `stream`: `page` for any path, the request's
/// head pushed onto `seen`.
fn serve_one(stream: TcpStream, page: &str, seen: &std::sync::Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        head.push_str(&line);
    }
    seen.lock().unwrap().push(head);
    let mut out = stream;
    let _ = write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
}

/// One request to the relay's TCP bridge.
fn call(relay: &robot_relay::RelayHandle, cmd: &str, args: Value) -> Value {
    let mut stream = TcpStream::connect(relay.tcp_addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    writeln!(stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args, "timeout_ms": 20000 })).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// CrewForge E2E report B4: an app that picks its screen from the URL it
/// boots at (`/fill/<token>`), and a kiosk that signs in with an HttpOnly
/// `device` cookie, could not be reached — no verb booted the app at a URL
/// or set a cookie. Against a real headless Chromium over plain HTTP: the
/// relay sets an HttpOnly and a script-visible cookie on the driven tab,
/// lists both, then `load_url` boots a NEW page at a path — the server
/// receives both cookies, the new page reads its path and the visible
/// cookie and dials the relay, and the call returns once it has.
#[test]
fn regression_load_url_and_cookies_drive_a_real_headless_tab() {
    let Some(browser) = browser() else {
        eprintln!("cdp_real_browser: no Chromium-family browser found; skipped");
        return;
    };
    let root = std::env::temp_dir().join(format!("relay_cdp_load_url_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profiles = root.join(".idealyst");
    let profile = profiles.join("test-browser-profile");
    std::fs::create_dir_all(&profile).unwrap();

    let relay = robot_relay::start(robot_relay::RelayConfig {
        register: false,
        cdp_profiles_dir: Some(profiles.clone()),
        ..Default::default()
    })
    .unwrap();
    // Each page load is a new app instance: a fresh nonce, and a label
    // carrying what it booted with.
    let page = format!(
        r#"<!doctype html><body><script>
        var nonce = "n" + Math.random();
        window.{global} = nonce;
        var ws = new WebSocket("ws://127.0.0.1:{port}/");
        ws.onopen = function () {{
          ws.send(JSON.stringify({{ hello: {{ platform: "web", nonce: nonce,
            label: location.pathname + " | " + document.cookie }} }}));
        }};
        </script></body>"#,
        global = robot_relay::cdp::NONCE_GLOBAL,
        port = relay.ws_addr.port(),
    );
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://127.0.0.1:{}", server.local_addr().unwrap().port());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let seen = seen.clone();
        std::thread::spawn(move || {
            for stream in server.incoming().flatten() {
                serve_one(stream, &page, &seen);
            }
        });
    }

    let _browser = Kill(
        Command::new(&browser)
            .args(["--headless=new", "--remote-debugging-port=0", "--no-first-run"])
            .args(["--no-default-browser-check", "--disable-gpu"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("{origin}/start"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the browser"),
    );
    let active_label = || relay.apps().into_iter().find(|a| a.active).and_then(|a| a.label);
    let mut booted = false;
    for _ in 0..150 {
        if active_label().is_some_and(|l| l.starts_with("/start")) {
            booted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(booted, "the page never dialed the relay");

    let r = call(&relay, "set_cookie", json!({ "name": "device", "value": "kiosk-1", "http_only": true }));
    assert_eq!(r["ok"]["name"], "device", "{r}");
    let r = call(&relay, "set_cookie", json!({ "name": "theme", "value": "dark" }));
    assert!(r.get("err").is_none(), "{r}");
    let r = call(&relay, "get_cookies", json!({}));
    let cookies = r["ok"].as_array().unwrap_or_else(|| panic!("{r}"));
    let device = cookies.iter().find(|c| c["name"] == "device").expect("device cookie listed");
    assert_eq!((device["value"].as_str(), device["http_only"].as_bool()), (Some("kiosk-1"), Some(true)));

    let r = call(&relay, "load_url", json!({ "url": "/fill/abc?x=1" }));
    assert_eq!(r["ok"]["url"], format!("{origin}/fill/abc?x=1"), "{r}");
    let label = active_label().expect("the new page dialed in");
    assert_eq!(label, "/fill/abc | theme=dark", "the new page booted at the path; HttpOnly stays out of document.cookie");
    let request = seen.lock().unwrap().iter().find(|h| h.starts_with("GET /fill/abc")).cloned().expect("served");
    assert!(request.contains("device=kiosk-1") && request.contains("theme=dark"), "{request}");

    let r = call(&relay, "clear_cookies", json!({ "name": "theme" }));
    assert!(r.get("err").is_none(), "{r}");
    let names: Vec<Value> = call(&relay, "get_cookies", json!({}))["ok"].as_array().unwrap().iter().map(|c| c["name"].clone()).collect();
    assert_eq!(names, [json!("device")]);
    call(&relay, "clear_cookies", json!({}));
    assert_eq!(call(&relay, "get_cookies", json!({}))["ok"], json!([]));
    drop(_browser);
    std::fs::remove_dir_all(&root).ok();
}
