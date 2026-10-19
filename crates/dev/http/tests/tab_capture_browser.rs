//! The tab-capture controller in a real headless Chromium: the share is
//! started from a real (CDP-dispatched, so user-activating) click, Chrome's
//! `--auto-accept-this-tab-capture` answers the dialog `preferCurrentTab`
//! asks, and a grabbed frame is read back pixel by pixel. Pins what the
//! node test (`tab_capture.rs`) fakes: the options Chromium accepts, the
//! capture-handle check passing for this tab, and that a frame shows the
//! page with the dev overlay left out.
//!
//! Skips (and says so) when no Chromium-family browser is installed;
//! `IDEALYST_BROWSER` picks one explicitly.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tungstenite::Message;

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

/// A red block at the top-left, the dev overlay's host element as a blue
/// block at the top-right (it must not appear in a frame), and a button
/// that starts the share.
const PAGE: &str = r#"<!doctype html><body style="margin:0;background:#fff">
<div id="app"><div style="position:absolute;left:0;top:0;width:100px;height:100px;background:rgb(255,0,0)"></div></div>
<idealyst-dev-overlay style="position:fixed;left:200px;top:0;width:100px;height:100px;background:rgb(0,0,255);display:block"></idealyst-dev-overlay>
<button id="share" style="position:fixed;left:0;top:200px;width:100px;height:40px">share</button>
<script>
__CAPTURE__
var capture = idealystTabCapture(window, document);
window.started = null;
document.getElementById("share").addEventListener("click", function () {
  capture.start().then(function () { window.started = "ok"; }, function (e) { window.started = "err: " + e.message; });
});
// A frame's pixel at (x, y), as [r, g, b], in frame pixels.
window.pixelOf = function (frame, x, y) {
  return new Promise(function (resolve) {
    var img = new Image();
    img.onload = function () {
      var c = document.createElement("canvas");
      c.width = frame.width; c.height = frame.height;
      var g = c.getContext("2d");
      g.drawImage(img, 0, 0);
      var d = g.getImageData(x, y, 1, 1).data;
      resolve([d[0], d[1], d[2]]);
    };
    img.src = "data:image/png;base64," + frame.png_base64;
  });
};
</script></body>"#;

struct Cdp {
    ws: tungstenite::WebSocket<TcpStream>,
    id: u64,
}

impl Cdp {
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let frame = json!({ "id": self.id, "method": method, "params": params });
        self.ws.send(Message::Text(frame.to_string().into())).unwrap();
        loop {
            let Message::Text(t) = self.ws.read().unwrap() else { continue };
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["id"].as_u64() == Some(self.id) {
                return v["result"].clone();
            }
        }
    }

    fn eval(&mut self, expr: &str) -> Value {
        let r = self.call("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }));
        r["result"]["value"].clone()
    }
}

fn page_ws_url(port: u16) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).unwrap();
    // DevTools leaves the connection open: read until the JSON closes.
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf).unwrap();
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw);
        if let Some(at) = text.find("\r\n\r\n") {
            if let Ok(list) = serde_json::from_str::<Value>(&text[at + 4..]) {
                return list
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["type"] == "page")
                    .and_then(|t| t["webSocketDebuggerUrl"].as_str())
                    .unwrap()
                    .to_string();
            }
        }
    }
}

#[test]
fn a_shared_tab_frame_is_the_page_without_the_overlay() {
    let Some(browser) = browser() else {
        eprintln!("tab_capture_browser: no Chromium-family browser found; skipped");
        return;
    };
    let root = std::env::temp_dir().join(format!("tab_capture_browser_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    let page = root.join("page.html");
    std::fs::write(&page, PAGE.replace("__CAPTURE__", dev_http::TAB_CAPTURE_JS)).unwrap();

    let _browser = Kill(
        Command::new(&browser)
            .args(["--headless=new", "--remote-debugging-port=0", "--window-size=640,480", "--no-first-run"])
            .args(["--no-default-browser-check", "--force-device-scale-factor=1", "--auto-accept-this-tab-capture"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("file://{}", page.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the browser"),
    );
    let mut port = None;
    for _ in 0..100 {
        port = std::fs::read_to_string(profile.join("DevToolsActivePort"))
            .ok()
            .and_then(|t| t.lines().next()?.trim().parse::<u16>().ok());
        if port.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let port = port.expect("the browser wrote DevToolsActivePort");
    let url = page_ws_url(port);
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let (ws, _) = tungstenite::client(url.as_str(), stream).unwrap();
    let mut cdp = Cdp { ws, id: 0 };

    for _ in 0..50 {
        if cdp.eval("typeof capture") == "object" {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(cdp.eval("capture.state().supported"), true);
    // A grab before the share refuses.
    assert_eq!(cdp.eval("capture.grab().then(() => 'grabbed', e => e.message)"), "the tab is not shared");

    for kind in ["mousePressed", "mouseReleased"] {
        cdp.call("Input.dispatchMouseEvent", json!({ "type": kind, "x": 50, "y": 220, "button": "left", "clickCount": 1 }));
    }
    let mut started = Value::Null;
    for _ in 0..50 {
        started = cdp.eval("window.started");
        if !started.is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(started, "ok", "this tab is accepted, capture handle and all");
    assert_eq!(cdp.eval("capture.active()"), true);

    let frame = cdp.eval(
        "capture.grab().then(f => Promise.all([pixelOf(f, 50, 50), pixelOf(f, 250, 50), pixelOf(f, 450, 50)])
           .then(p => ({ width: f.width, height: f.height, red: p[0], overlay: p[1], blank: p[2] })))",
    );
    assert_eq!(frame["width"], 640, "{frame}");
    let close = |px: &Value, want: [i64; 3]| {
        px.as_array().unwrap().iter().zip(want).all(|(got, want)| (got.as_i64().unwrap() - want).abs() <= 24)
    };
    assert!(close(&frame["red"], [255, 0, 0]), "the page's pixels: {frame}");
    assert!(close(&frame["overlay"], [255, 255, 255]), "the overlay is hidden for the frame: {frame}");
    assert!(close(&frame["blank"], [255, 255, 255]), "{frame}");
    assert_eq!(cdp.eval("getComputedStyle(document.querySelector('idealyst-dev-overlay')).visibility"), "visible");
    assert!(cdp.eval("capture.state().lastShot").is_number());

    cdp.eval("capture.stop()");
    assert_eq!(cdp.eval("capture.active()"), false);
    std::fs::remove_dir_all(&root).ok();
}
