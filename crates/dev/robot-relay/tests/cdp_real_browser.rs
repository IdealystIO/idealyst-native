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
