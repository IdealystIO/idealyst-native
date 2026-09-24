//! The injected script finds the stream: same-origin first, then the dev
//! session's own port — and only moves on when the first place is not
//! the stream at all.
//!
//! A full-stack page is served by the app's own server. When that server
//! is built on the framework's router it proxies `/__idealyst/reload` to
//! the dev session (the only origin a devcontainer is sure to forward);
//! when it is not, the relative URL answers with a 404 or the app's
//! `index.html`, and the page must fall back to the session's port rather
//! than sit on a stream that will never come.
//!
//! Run in node against a fake `EventSource` (the script's only browser
//! dependency here besides the overlay's small DOM use). Needs `node` on
//! `PATH`; without it the test says so and passes.

use std::io::Write;
use std::process::{Command, Stdio};

const HARNESS: &str = r##"
function El(tag) { this.children = []; this.style = {}; this.attrs = {}; this.listeners = {}; }
El.prototype.appendChild = function (c) { this.children.push(c); return c; };
El.prototype.removeChild = function () {};
El.prototype.setAttribute = function (k, v) { this.attrs[k] = v; };
El.prototype.addEventListener = function (k, f) { (this.listeners[k] = this.listeners[k] || []).push(f); };
El.prototype.attachShadow = function () { return this; };
global.window = global;
global.document = new El("#document");
document.body = new El("body");
document.createElement = t => new El(t);
const beacons = [];
// node 22 has its own read-only `navigator`.
Object.defineProperty(globalThis, "navigator", {
  value: { sendBeacon: (url, body) => { beacons.push(url); return true; } },
  configurable: true,
});
global.location = { reload() {} };
const opened = [];
global.EventSource = function (url) {
  this.url = url; this.readyState = 0; this.listeners = {};
  opened.push(this);
};
EventSource.prototype.addEventListener = function (k, f) { (this.listeners[k] = this.listeners[k] || []).push(f); };
EventSource.prototype.fire = function (k, e) {
  if (k === "message" && this.onmessage) this.onmessage(e);
  (this.listeners[k] || []).forEach(f => f(e || {}));
};
const script = require("fs").readFileSync(0, "utf8");
eval(script.replace(/^<script>/, "").replace(/<\/script>$/, ""));
const out = { urls: [] };
const scenario = process.argv[1];
if (scenario === "not-the-stream") {
  // The app server's catch-all answered with its index.html: the
  // EventSource fails for good before it ever opens.
  opened[0].readyState = 2; opened[0].fire("error");
  opened[1].readyState = 1; opened[1].fire("open"); opened[1].fire("message", { data: "1" });
} else if (scenario === "restarting") {
  // Refused while the server restarts: EventSource stays CONNECTING.
  opened[0].fire("error");
  opened[0].readyState = 1; opened[0].fire("open"); opened[0].fire("message", { data: "1" });
} else if (scenario === "dropped-after-open") {
  opened[0].readyState = 1; opened[0].fire("open"); opened[0].fire("message", { data: "1" });
  opened[0].readyState = 2; opened[0].fire("error");
}
out.urls = opened.map(e => e.url);
out.beacons = beacons;
process.stdout.write(JSON.stringify(out));
"##;

fn run(scenario: &str, script: &str) -> Option<serde_json::Value> {
    Command::new("node").arg("--version").output().ok()?.status.success().then_some(())?;
    let mut child = Command::new("node")
        .args(["-e", HARNESS, scenario])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "node failed");
    Some(serde_json::from_slice(&out.stdout).expect("harness output"))
}

fn script() -> String {
    dev_http::reload_script_tag_with_fallback(
        "/__idealyst/reload",
        Some("http://127.0.0.1:4777/__idealyst/reload"),
    )
}

#[test]
fn a_page_whose_server_does_not_proxy_falls_back_to_the_session_port() {
    let Some(out) = run("not-the-stream", &script()) else {
        return eprintln!("node not on PATH; skipped");
    };
    assert_eq!(
        out["urls"],
        serde_json::json!(["/__idealyst/reload", "http://127.0.0.1:4777/__idealyst/reload"])
    );
    // It acks where it now listens.
    assert_eq!(out["beacons"], serde_json::json!(["http://127.0.0.1:4777/__idealyst/ack"]));
}

#[test]
fn a_same_origin_stream_that_is_only_down_is_kept() {
    let Some(out) = run("restarting", &script()) else { return };
    assert_eq!(out["urls"], serde_json::json!(["/__idealyst/reload"]));
    assert_eq!(out["beacons"], serde_json::json!(["/__idealyst/ack"]));
}

#[test]
fn a_stream_that_opened_is_never_abandoned() {
    let Some(out) = run("dropped-after-open", &script()) else { return };
    assert_eq!(out["urls"], serde_json::json!(["/__idealyst/reload"]));
}

#[test]
fn a_single_source_script_has_nowhere_to_fall_back_to() {
    let Some(out) = run("not-the-stream-single", &dev_http::reload_script_tag("/__idealyst/reload")) else {
        return;
    };
    assert_eq!(out["urls"], serde_json::json!(["/__idealyst/reload"]));
}
