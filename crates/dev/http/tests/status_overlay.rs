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
// `!storage <json>` seeds a fake localStorage before the overlay is built.
const stored = {};
document.defaultView = { localStorage: {
  getItem: k => (k in stored ? stored[k] : null),
  setItem: (k, v) => { stored[k] = String(v); },
} };
const input = require("fs").readFileSync(0, "utf8").split("\n").filter(Boolean);
for (const line of input) {
  if (line.startsWith("!storage ")) Object.assign(stored, JSON.parse(line.slice("!storage ".length)));
}

__OVERLAY__

// A fake tab-capture controller (`tab_capture.js`'s surface): its state
// is set by `!capture {…}` lines; the button's clicks are counted.
const capture = {
  st: { supported: true, sharing: false, requested: false, lastShot: null, error: null },
  starts: 0, stops: 0, subs: [],
  state() { return this.st; },
  start() { this.starts++; return Promise.resolve(); },
  stop() { this.stops++; },
  subscribe(f) { this.subs.push(f); },
};
const o = idealystStatusOverlay(document, capture);
const lines = input.filter(l => !l.startsWith("!storage "));
const out = [];
for (const line of lines) {
  if (line.startsWith("!robot ")) {
    // What the app's robot client publishes: a JSON string.
    o.robot(line.slice("!robot ".length));
    continue;
  }
  if (line.startsWith("!capture ")) {
    Object.assign(capture.st, JSON.parse(line.slice("!capture ".length)));
    capture.subs.forEach(f => f());
    continue;
  }
  if (line.startsWith("#")) {
    const cmd = line.slice(1).trim();
    if (cmd === "esc") document.fire("keydown", { key: "Escape" });
    if (cmd === "click-share") document.body.children[0].find("share").fire("click", {});
    if (cmd === "click-collapse") document.body.children[0].find("collapse").fire("click", {});
    if (cmd === "click-pill") document.body.children[0].find("pill").fire("click", {});
    const host = document.body.children[0];
    const share = host.find("share");
    const panel = host.find("panel");
    out.push({
      badge: host.find("badge").textContent,
      panel: panel.style.display !== "none",
      panelText: panel.textContent,
      diagnostics: host.findAll("diagnostic").length,
      robot: host.find("robot-row").style.display === "none" ? null : host.find("robot").textContent,
      robotColor: host.find("robot").style.color || null,
      share: share.style.display === "none" ? null : share.textContent,
      shareLit: share.style.background === "#e5b443",
      shareTitle: share.attrs["title"] || null,
      expanded: host.find("stack").style.display !== "none",
      pill: host.find("pill").style.display === "none" ? null : host.find("pill").textContent,
      pillLit: host.find("pill").style.background === "#e5b443",
      stored: stored["idealyst-dev-overlay-collapsed"] || null,
      starts: capture.starts,
      stops: capture.stops,
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
    assert!(script.contains("function idealystStatusOverlay(doc, capture)"));
    assert!(script.contains("function idealystTabCapture(win, doc)"));
    assert!(!script.contains("__TAB_CAPTURE__"));
    assert!(script.contains(r#"es.addEventListener("dev-state""#));
    assert!(!script.contains("__STATUS_OVERLAY__"));
    // The names `backend_web::robot_transport` publishes under
    // (`ROBOT_STATUS_HOOK`, `ROBOT_STATUS_GLOBAL`): dev-http cannot depend
    // on backend-web, so the pair is held together here.
    assert!(script.contains("window.__idealyst_dev_robot_status = status.robot;"));
    assert!(script.contains("if (window.__idealyst_dev_robot) status.robot(window.__idealyst_dev_robot);"));
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

// A full-stack session: the project's own server has a target of its own.
const FS_SESSION: &str = r#"{"v":1,"seq":20,"at_ms":0,"type":"session_started","app":"CrewForge","targets":["web"],"mode":"local","hot_tier":{"state":"armed"},"server":{"target":"server","name":"crewforge-server"}}"#;
const SRV_CHANGE: &str = r#"{"v":1,"seq":21,"at_ms":1,"type":"change_detected","target":"server","paths":["/cf/crates/core/src/lib.rs"],"crates":[],"folded":0}"#;
const SRV_BUILD: &str = r#"{"v":1,"seq":22,"at_ms":2,"type":"build_started","target":"server","cause":"save","folded":0}"#;
const SRV_PROGRESS: &str = r#"{"v":1,"seq":23,"at_ms":3,"type":"cargo_progress","target":"server","compiled":509,"total":512,"current":"crewforge-api-server"}"#;
const WEB_RELOADED: &str = r#"{"v":1,"seq":24,"at_ms":4,"type":"build_finished","target":"web","outcome":"reloaded","gen":2,"ms":3100}"#;
const SRV_RELOADED: &str = r#"{"v":1,"seq":25,"at_ms":5,"type":"build_finished","target":"server","outcome":"reloaded","gen":2,"ms":9000}"#;
const SRV_READY: &str = r#"{"v":1,"seq":26,"at_ms":6,"type":"server_ready","target":"web","kind":"full_stack","url":"http://127.0.0.1:3100"}"#;
const SRV_DIAG: &str = r#"{"v":1,"seq":27,"at_ms":7,"type":"diagnostic","target":"server","diagnostic":{"level":"error","message":"cannot find value `x`","file":"crates/api-server/src/routes.rs","line":4,"column":5,"rendered":"error[E0425]: cannot find value `x`\n"}}"#;
const SRV_FAILED: &str = r#"{"v":1,"seq":28,"at_ms":8,"type":"build_finished","target":"server","outcome":"failed","error":"server build failed","ms":900}"#;

/// The badge says "building server…" through a server rebuild — the
/// page is served by that server, and a save that rebuilt the bundle too
/// reloads only once the server is back — then "server restarted".
#[test]
fn the_badge_follows_the_full_stack_server_through_a_restart() {
    let Some(frames) = run(&[
        FS_SESSION, SRV_CHANGE, SRV_BUILD, SRV_PROGRESS, "#",
        BUILD, "#",
        WEB_RELOADED, "#",
        SRV_RELOADED, "#",
        SRV_READY, "#",
    ]) else {
        return skip();
    };
    assert_eq!(frames[0]["badge"], "⚙ rebuilding server… 509/512 crewforge-api-server", "{}", frames[0]);
    // Both building: the server leads, the bundle follows.
    assert_eq!(frames[1]["badge"], "⚙ rebuilding server… 509/512 crewforge-api-server · ⚙ rebuilding", "{}", frames[1]);
    // The bundle is done; the page still waits on the server.
    assert_eq!(frames[2]["badge"], "⚙ rebuilding server… 509/512 crewforge-api-server", "{}", frames[2]);
    assert_eq!(frames[3]["badge"], "⚙ restarting server", "{}", frames[3]);
    assert_eq!(frames[4]["badge"], "✓ server restarted", "{}", frames[4]);
}

/// A server that fails to build shows its error like any build's, named
/// as the server's.
#[test]
fn a_server_build_failure_shows_the_panel() {
    let Some(frames) = run(&[FS_SESSION, SRV_BUILD, SRV_DIAG, SRV_FAILED, "#"]) else {
        return skip();
    };
    assert_eq!(frames[0]["panel"], true, "{}", frames[0]);
    let text = frames[0]["panelText"].as_str().unwrap();
    assert!(text.contains("Build failed (server)"), "{text}");
    assert!(text.contains("crates/api-server/src/routes.rs:4:5"), "{text}");
    assert_eq!(frames[0]["badge"], "✗ server build failed");
}

/// Regression: a page that could not reach the Robot relay showed nothing —
/// the badge never mentioned the robot, so "is this tab connected?" had no
/// answer short of an MCP call failing. The robot client's reports now get
/// their own line: absent until the client reports, then connecting,
/// connected, or the failure with when it retries.
#[test]
fn regression_the_robot_connection_is_shown_on_the_page() {
    let Some(out) = run(&[
        BUILD,
        "#",
        r#"!robot {"state":"connecting","url":"ws://localhost:3100/__idealyst/relay"}"#,
        "#",
        r#"!robot {"state":"retrying","error":"can't reach the robot relay: ws://localhost:3100/__idealyst/relay (close 1006), ws://127.0.0.1:35109 (close 1006)","retry_ms":2000}"#,
        "#",
        r#"!robot {"state":"connected","url":"ws://localhost:3100/__idealyst/relay"}"#,
        "#",
        r#"!robot {"state":"dropped","url":"ws://localhost:3100/__idealyst/relay","error":"close 1006"}"#,
        "#",
    ]) else {
        return skip();
    };
    assert_eq!(out[0]["robot"], serde_json::Value::Null, "nothing until the client reports");
    assert_eq!(out[1]["robot"], "◌ robot connecting…");
    assert_eq!(
        out[2]["robot"],
        "✗ can't reach the robot relay: ws://localhost:3100/__idealyst/relay (close 1006), ws://127.0.0.1:35109 (close 1006) · retrying in 2.0 s"
    );
    assert_eq!(out[2]["robotColor"], "#ff8b8b");
    assert_eq!(out[3]["robot"], "● robot connected");
    assert_eq!(out[3]["robotColor"], "#8be28b");
    assert_eq!(out[4]["robot"], "◌ robot disconnected (close 1006), reconnecting…");
    // The build badge is untouched by any of it.
    assert!(out.iter().all(|s| s["badge"] == "⚙ rebuilding" && s["panel"] == false), "{out:?}");
}

/// Regression: a full-stack server whose port was taken said only that
/// it "exited" on the page, if anything. The server going down is an
/// error panel with the cause, on the badge and over the page.
#[test]
fn regression_a_stopped_server_says_why_on_the_page() {
    let session = r#"{"v":1,"seq":1,"at_ms":0,"type":"session_started","app":"cf","targets":["web"],"mode":"local","hot_tier":{"state":"armed"},"server":{"target":"server","name":"crewforge-server"}}"#;
    let in_use = r#"{"v":1,"seq":2,"at_ms":1,"type":"server_down","target":"server","reason":"port_in_use","port":8080,"holder":"pid 4821 (crewforge-server)"}"#;
    let exited = r#"{"v":1,"seq":3,"at_ms":2,"type":"server_down","target":"server","reason":"exited","status":"exit status: 1","cause":"Error: Address already in use (os error 48)","log_file":"/cf/server.log"}"#;
    let Some(out) = run(&[session, in_use, "#", exited, "#"]) else {
        return skip();
    };
    assert_eq!(
        out[0]["badge"],
        "✗ server not started: port 8080 is already in use by pid 4821 (crewforge-server)"
    );
    assert_eq!(out[0]["panel"], true);
    assert!(out[0]["panelText"].as_str().unwrap().starts_with("Server not started (server)"), "{}", out[0]["panelText"]);
    assert_eq!(out[1]["badge"], "✗ server stopped: exited with exit status: 1: Error: Address already in use (os error 48)");
    let text = out[1]["panelText"].as_str().unwrap();
    assert!(text.contains("Its output is in /cf/server.log"), "{text}");
}

/// The tab-share button: hidden until the robot is connected, then the
/// click that starts the share (the browser allows `getDisplayMedia` only
/// from one). It lights up when an agent asked for the tab, says when it
/// is sharing, and a click then stops it.
#[test]
fn the_share_button_follows_the_robot_and_the_capture() {
    const CONNECTED: &str = r#"!robot {"state":"connected","url":"ws://h/__idealyst/relay"}"#;
    let Some(out) = run(&[
        BUILD,
        "#",
        CONNECTED,
        "#",
        r#"!capture {"requested":true}"#,
        "#",
        "#click-share",
        r#"!capture {"requested":false,"sharing":true,"lastShot":1760000000000}"#,
        "#",
        "#click-share",
        r#"!capture {"sharing":false,"lastShot":null,"error":"a window was shared, not this tab"}"#,
        "#",
        r#"!robot {"state":"dropped","url":"ws://h/__idealyst/relay","error":"close 1006"}"#,
        "#",
    ]) else {
        return skip();
    };
    assert_eq!(out[0]["share"], serde_json::Value::Null, "no robot, no agent to share with");
    assert_eq!(out[1]["share"], "allow capture");
    assert!(
        out[1]["shareTitle"].as_str().unwrap().contains("The browser asks for permission"),
        "the chip says the click is the browser's permission prompt"
    );
    assert_eq!(out[1]["shareLit"], false);
    assert_eq!(out[2]["share"], "agent wants capture · allow");
    assert_eq!(out[2]["shareLit"], true, "an agent's request lights the button");
    // `#click-share` records a frame of its own: the click, before the
    // controller reports back.
    assert_eq!(out[3]["starts"], 1, "the click starts the share");
    assert_eq!(out[4]["share"], "◉ capturing · stop");
    let title = out[4]["shareTitle"].as_str().unwrap();
    assert!(title.contains(" · last screenshot "), "{title}");
    assert_eq!(out[4]["shareLit"], false);
    assert_eq!(out[5]["stops"], 1, "a click while sharing stops it");
    assert_eq!(out[5]["starts"], 1);
    assert_eq!(out[6]["share"], "allow capture (a window was shared, not this tab)");
    assert_eq!(out[7]["share"], serde_json::Value::Null, "hidden again while the robot is away");
}

/// A browser that cannot share a tab shows no button — until an agent
/// asks, when it says why the agent will not get one.
#[test]
fn a_browser_without_tab_sharing_says_so_only_when_asked() {
    let Some(out) = run(&[
        r#"!capture {"supported":false}"#,
        r#"!robot {"state":"connected","url":"ws://h/__idealyst/relay"}"#,
        "#",
        r#"!capture {"requested":true}"#,
        "#",
    ]) else {
        return skip();
    };
    assert_eq!(out[0]["share"], serde_json::Value::Null);
    assert_eq!(out[1]["share"], "✗ capture unsupported");
    assert!(out[1]["shareTitle"].as_str().unwrap().contains("cannot capture a tab"));
}

/// The overlay collapses to one status glyph and back, remembers that
/// for the page, still opens a failed build's panel while collapsed, and
/// lights the glyph when an agent asks for capture.
#[test]
fn the_overlay_collapses_to_a_glyph_and_remembers_it() {
    let Some(out) = run(&[
        BUILD,
        "#",
        "#click-collapse",
        r#"!robot {"state":"connected","url":"ws://h/__idealyst/relay"}"#,
        r#"!capture {"requested":true}"#,
        "#",
        DIAG,
        FAILED,
        "#",
        "#click-pill",
    ]) else {
        return skip();
    };
    assert_eq!(out[0]["expanded"], true);
    assert_eq!(out[0]["pill"], serde_json::Value::Null);
    assert_eq!(out[1]["expanded"], false, "the – collapses the overlay");
    assert_eq!(out[1]["pill"], "⚙", "the glyph is the status icon");
    assert_eq!(out[1]["stored"], "1", "remembered for the page");
    assert_eq!(out[2]["pillLit"], true, "an agent's capture request lights the glyph");
    assert_eq!(out[3]["panel"], true, "a failed build still opens its panel while collapsed");
    assert_eq!(out[3]["pill"], "✗");
    assert_eq!(out[4]["expanded"], true, "the glyph expands it again");
    assert_eq!(out[4]["stored"], "0");

    let Some(out) = run(&[r#"!storage {"idealyst-dev-overlay-collapsed":"1"}"#, BUILD, "#"]) else {
        return skip();
    };
    assert_eq!(out[0]["expanded"], false, "a reload starts collapsed when it was left collapsed");
}
