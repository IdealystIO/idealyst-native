//! The page's half of replaying patches to a page that connects late.
//!
//! The reload stream hands a connecting page the patches its base bundle
//! lacks (`ReloadSignal::connect_snapshot`). That reaches the page BEFORE
//! its wasm module has booted and published the appliers, so the script
//! has to queue them — the first cut applied at once, found no applier,
//! reloaded, was replayed the patch again and reloaded for ever. It also
//! has to skip a patch it already took (a reconnect replays it again) and
//! reload when the stream it reconnected to belongs to a restarted dev
//! session.
//!
//! Run in node against a fake `EventSource`, like `stream_fallback.rs`.
//! Needs `node` on `PATH`; without it the test says so and passes. The
//! CLI E2E (`wasm_hot_patch_e2e`) covers the same path in a real browser.

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
// The script's own logging must not reach the JSON on stdout.
console.info = console.log = console.warn = console.error;
global.document = new El("#document");
document.body = new El("body");
document.createElement = t => new El(t);
const acks = [];
Object.defineProperty(globalThis, "navigator", {
  value: { sendBeacon: (url, body) => { acks.push(JSON.parse(body).kind); return true; } },
  configurable: true,
});
let reloads = 0;
global.location = { reload() { reloads++; } };
const opened = [];
global.EventSource = function (url) {
  this.url = url; this.readyState = 1; this.listeners = {};
  opened.push(this);
};
EventSource.prototype.addEventListener = function (k, f) { (this.listeners[k] = this.listeners[k] || []).push(f); };
EventSource.prototype.fire = function (k, e) {
  if (k === "message" && this.onmessage) this.onmessage(e);
  (this.listeners[k] || []).forEach(f => f(e || {}));
};
const script = require("fs").readFileSync(0, "utf8");
eval(script.replace(/^<script>/, "").replace(/<\/script>$/, ""));
const es = opened[0];
const applied = [];
const hotApplier = (url) => applied.push("hot " + url);
const overlayApplier = (data) => { applied.push("overlay " + data); return { applied: 1, refused: 0 }; };
const hot = (id, n) => es.fire("hot-patch", { lastEventId: id, data: JSON.stringify({ url: "/p" + n, table: {} }) });
const overlay = (id, d) => es.fire("patch", { lastEventId: id, data: d });
const gen = (id, g) => es.fire("message", { lastEventId: id, data: String(g) });
const scenario = process.argv[1];
const later = (ms, f) => setTimeout(f, ms);
if (scenario === "replay-before-boot") {
  // Connected, replayed an overlay then a hot patch, module not booted.
  gen("s:0", 1); overlay("s:2", "o2"); hot("s:3", 3);
  later(200, () => { window.__idealyst_overlay_patch = overlayApplier; window.__idealyst_hot_patch = hotApplier; });
} else if (scenario === "reconnect-replays-what-it-holds") {
  window.__idealyst_overlay_patch = overlayApplier; window.__idealyst_hot_patch = hotApplier;
  gen("s:0", 1); hot("s:3", 3);
  // The server restarted; the stream reconnects and replays the base's
  // missing patch again, then a new one arrives.
  gen("s:0", 1); hot("s:3", 3); hot("s:5", 5);
} else if (scenario === "restarted-session") {
  window.__idealyst_hot_patch = hotApplier;
  gen("a:0", 1);
  // The dev session was restarted: same generation number, new session.
  gen("b:0", 1); hot("b:2", 2);
} else if (scenario === "booted-without-hot-applier") {
  window.__idealyst_overlay_patch = overlayApplier;
  gen("s:0", 1); hot("s:3", 3);
}
later(500, () => process.stdout.write(JSON.stringify({ applied, reloads, acks })));
"##;

fn run(scenario: &str) -> Option<serde_json::Value> {
    Command::new("node").arg("--version").output().ok()?.status.success().then_some(())?;
    let mut child = Command::new("node")
        .args(["-e", HARNESS, scenario])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let script = dev_http::reload_script_tag("/__idealyst/reload");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "node failed");
    Some(serde_json::from_slice(&out.stdout).expect("harness output"))
}

/// Regression: the replayed hot patch reached a page whose module had
/// not booted, found no applier and reloaded — and the reloaded page was
/// replayed it again, for ever (seen on CrewForge: a refresh after a hot
/// patch never settled). It now waits, then applies in order, once.
#[test]
fn regression_a_patch_replayed_before_the_module_boots_waits_for_it() {
    let Some(out) = run("replay-before-boot") else {
        return eprintln!("node not on PATH; skipped");
    };
    assert_eq!(out["reloads"], 0, "{out}");
    assert_eq!(out["applied"], serde_json::json!(["overlay o2", "hot /p3"]), "{out}");
}

#[test]
fn a_reconnect_does_not_apply_a_patch_twice() {
    let Some(out) = run("reconnect-replays-what-it-holds") else { return };
    assert_eq!(out["reloads"], 0, "{out}");
    assert_eq!(out["applied"], serde_json::json!(["hot /p3", "hot /p5"]), "{out}");
}

/// The generation restarts at 1 every session, so it could not tell a
/// page that its stream now belongs to a restarted session — whose
/// patches pair with a bundle the page may not run.
#[test]
fn a_stream_from_a_restarted_session_reloads_the_page() {
    let Some(out) = run("restarted-session") else { return };
    assert_eq!(out["reloads"], 1, "{out}");
    assert_eq!(out["applied"], serde_json::json!([]), "{out}");
}

/// A booted module with no hot-patch applier keeps the old fallback:
/// the dev loop decided not to rebuild, so the page reloads.
#[test]
fn a_booted_page_without_the_applier_still_reloads() {
    let Some(out) = run("booted-without-hot-applier") else { return };
    assert_eq!(out["reloads"], 1, "{out}");
    assert!(out["acks"].as_array().unwrap().iter().any(|a| a == "failed"), "{out}");
}
