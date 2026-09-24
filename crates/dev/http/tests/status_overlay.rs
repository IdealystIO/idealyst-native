//! The in-page build-state overlay renders what the dev loop sends it.
//!
//! The overlay is plain JS (it must work when the page's wasm is what
//! failed to build), so it is exercised in node against a small fake DOM
//! — enough of `document` for the narrow API the overlay uses. The
//! harness feeds it `dev-state` objects exactly as the reload stream
//! sends them and reads back what is on screen.
//!
//! Needs `node` on `PATH`. Without it the test says so and passes: the
//! end-to-end test (`crates/tools/cli/tests/wasm_hot_patch_e2e.rs`)
//! checks the same overlay in a real browser.

use std::io::Write;
use std::process::{Command, Stdio};

const HARNESS: &str = r##"
// A fake DOM: elements with children, text, style, attributes, listeners.
function El(tag) {
  this.tagName = tag; this.children = []; this.parent = null;
  this.style = {}; this.attrs = {}; this.listeners = {}; this._text = null;
}
El.prototype.appendChild = function (c) { c.parent = this; this.children.push(c); return c; };
El.prototype.removeChild = function (c) { this.children = this.children.filter(x => x !== c); return c; };
Object.defineProperty(El.prototype, "firstChild", { get() { return this.children[0] || null; } });
Object.defineProperty(El.prototype, "textContent", {
  get() { return this._text !== null ? this._text : this.children.map(c => c.textContent).join("\n"); },
  set(v) { this._text = String(v); this.children = []; }
});
El.prototype.setAttribute = function (k, v) { this.attrs[k] = v; };
El.prototype.addEventListener = function (k, f) { (this.listeners[k] = this.listeners[k] || []).push(f); };
El.prototype.attachShadow = function () { return this; };
El.prototype.fire = function (k, e) { (this.listeners[k] || []).forEach(f => f(e)); };
El.prototype.find = function (part) {
  if (this.attrs["data-part"] === part) return this;
  for (const c of this.children) { const f = c.find(part); if (f) return f; }
  return null;
};
El.prototype.findAll = function (part, out) {
  out = out || [];
  if (this.attrs["data-part"] === part) out.push(this);
  for (const c of this.children) c.findAll(part, out);
  return out;
};
const document = new El("#document");
document.body = new El("body");
document.createElement = t => new El(t);

__OVERLAY__

const o = idealystStatusOverlay(document);
const lines = require("fs").readFileSync(0, "utf8").split("\n").filter(Boolean);
const out = [];
for (const line of lines) {
  if (line.startsWith("#")) {
    const cmd = line.slice(1).trim();
    if (cmd === "esc") document.fire("keydown", { key: "Escape" });
    const host = document.body.children[0];
    const panel = host.find("panel");
    out.push({
      badge: host.find("badge").textContent,
      panel: panel.style.display !== "none",
      panelText: panel.textContent,
      diagnostics: host.findAll("diagnostic").length,
    });
    continue;
  }
  o.apply(JSON.parse(line));
}
process.stdout.write(JSON.stringify(out));
"##;

fn node() -> Option<String> {
    Command::new("node").arg("--version").output().ok()?.status.success().then(|| "node".into())
}

fn run(script: &[&str]) -> Option<Vec<serde_json::Value>> {
    let node = node()?;
    let harness = HARNESS.replace("__OVERLAY__", dev_http::STATUS_OVERLAY_JS);
    let mut child = Command::new(node)
        .args(["-e", &harness])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node");
    child.stdin.take().unwrap().write_all(script.join("\n").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some(serde_json::from_slice(&out.stdout).expect("the harness prints JSON"))
}

fn skip() {
    eprintln!("status_overlay: `node` is not on PATH; skipped (the CLI E2E covers the overlay)");
}

const BUILD: &str = r#"{"v":1,"seq":1,"at_ms":0,"type":"build_started","target":"web","cause":"save","folded":0}"#;
const STAGE: &str = r#"{"v":1,"seq":2,"at_ms":1,"type":"stage_started","target":"web","stage":"cargo"}"#;
const PROGRESS: &str = r#"{"v":1,"seq":3,"at_ms":2,"type":"cargo_progress","target":"web","compiled":212,"total":480,"current":"idea-ui"}"#;
const DIAG: &str = r#"{"v":1,"seq":4,"at_ms":3,"type":"diagnostic","target":"web","diagnostic":{"level":"error","message":"mismatched types","code":"E0308","file":"src/app.rs","line":12,"column":9,"rendered":"error[E0308]: mismatched types\n  --> src/app.rs:12:9\n"}}"#;
const FAILED: &str = r#"{"v":1,"seq":5,"at_ms":4,"type":"build_finished","target":"web","outcome":"failed","error":"cargo exited with exit status: 101","ms":3100}"#;
const PATCHED: &str = r#"{"v":1,"seq":6,"at_ms":5,"type":"patch_built","target":"web","files":["src/app.rs"],"crates":[],"redirected":3,"steps":[],"skipped":[],"bytes":10,"ms":446}"#;

#[test]
fn the_overlay_shows_progress_then_the_error_and_clears_on_success() {
    let Some(frames) = run(&[BUILD, STAGE, PROGRESS, "#", DIAG, FAILED, "#", PATCHED, "#"]) else {
        return skip();
    };
    // Building: the badge carries the progress and the stage.
    let building = &frames[0];
    assert_eq!(building["badge"], "⚙ rebuilding 212/480 idea-ui · cargo", "{building}");
    assert_eq!(building["panel"], false);

    // Failed: the panel is up with the diagnostic's file:line and its
    // rendered message.
    let failed = &frames[1];
    assert_eq!(failed["panel"], true, "{failed}");
    assert_eq!(failed["diagnostics"], 1);
    let text = failed["panelText"].as_str().unwrap();
    assert!(text.contains("Build failed (web)"), "{text}");
    assert!(text.contains("src/app.rs:12:9 — mismatched types"), "{text}");
    assert!(text.contains("error[E0308]: mismatched types"), "{text}");
    assert_eq!(failed["badge"], "✗ build failed");

    // The next success clears it.
    let fixed = &frames[2];
    assert_eq!(fixed["panel"], false, "{fixed}");
    assert_eq!(fixed["badge"], "✓ hot patch · 3 fn · 446 ms");
}

#[test]
fn escape_dismisses_the_error_until_the_next_failure() {
    let Some(frames) = run(&[BUILD, DIAG, FAILED, "#", "#esc", BUILD, DIAG, FAILED, "#"]) else {
        return skip();
    };
    assert_eq!(frames[0]["panel"], true);
    assert_eq!(frames[1]["panel"], false, "Esc dismisses");
    assert_eq!(frames[2]["panel"], true, "a new failure shows again");
}

#[test]
fn a_failure_with_no_diagnostic_shows_the_error_itself() {
    let Some(frames) = run(&[BUILD, FAILED, "#"]) else { return skip() };
    assert_eq!(frames[0]["diagnostics"], 0);
    assert!(
        frames[0]["panelText"].as_str().unwrap().contains("cargo exited with exit status: 101"),
        "{}",
        frames[0]
    );
}

#[test]
fn the_overlay_is_part_of_the_served_reload_script() {
    let script = dev_http::reload_script_tag("/__idealyst/reload");
    assert!(script.contains("function idealystStatusOverlay(doc)"));
    assert!(script.contains(r#"es.addEventListener("dev-state""#));
    assert!(!script.contains("__STATUS_OVERLAY__"));
}

/// Undoing the edit that broke the build returns the source to what is
/// running; the error no longer describes it, so it goes.
#[test]
fn saving_back_to_the_running_source_clears_the_error() {
    const UNCHANGED: &str = r#"{"v":1,"seq":9,"at_ms":9,"type":"decided","target":"web","decision":{"tier":"unchanged"}}"#;
    let Some(frames) = run(&[BUILD, DIAG, FAILED, "#", UNCHANGED, "#"]) else { return skip() };
    assert_eq!(frames[0]["panel"], true);
    assert_eq!(frames[1]["panel"], false, "{}", frames[1]);
    assert_eq!(frames[1]["badge"], "○ no change");
}
