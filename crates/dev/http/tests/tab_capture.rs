//! The tab-capture controller (`tab_capture.js`) behind the Robot's
//! `screenshot` source `tab`.
//!
//! Exercised in node against fakes of the browser surface it uses
//! (`getDisplayMedia`, a `<video>` fed by the stream, a `<canvas>`), so
//! what it asks the browser for, what it refuses, and the frame it hands
//! out are all checked without a real share dialog — which no headless
//! browser can click through without flags that would skip the very
//! checks under test.
//!
//! Needs `node` on `PATH`; without it the tests say so and pass.

use std::process::Command;

const HARNESS: &str = r##"
const log = [];
function fakeTrack(settings, captureHandle) {
  const t = {
    readyState: "live", listeners: {},
    getSettings() { return settings; },
    getCaptureHandle: captureHandle === undefined ? undefined : () => captureHandle,
    stop() { t.readyState = "ended"; },
    addEventListener(k, f) { t.listeners[k] = f; },
  };
  return t;
}
let handleConfig = null;
let nextShare = null; // (opts) => stream | throws
const mediaDevices = {
  setCaptureHandleConfig(c) { handleConfig = c; },
  getDisplayMedia(opts) {
    log.push({ getDisplayMedia: opts });
    try { return Promise.resolve(nextShare(opts)); } catch (e) { return Promise.reject(e); }
  },
};
function streamOf(track) {
  return { getVideoTracks: () => [track], getTracks: () => [track] };
}
const host = { style: { visibility: "" } };
const doc = {
  querySelector(sel) { return sel === "idealyst-dev-overlay" ? host : null; },
  createElement(tag) {
    if (tag === "video") {
      return {
        videoWidth: 1600, videoHeight: 1200,
        play() { return Promise.resolve(); },
        requestVideoFrameCallback(cb) { setTimeout(cb, 1); },
      };
    }
    if (tag === "canvas") {
      const c = {
        getContext() {
          return { drawImage(v, x, y, w, h) { log.push({ draw: [w, h], overlay: host.style.visibility }); } };
        },
        toDataURL(type) { return "data:" + type + ";base64,UE5HREFUQQ=="; },
      };
      return c;
    }
    throw new Error("unexpected element " + tag);
  },
};
const win = {
  navigator: process.env.NO_MEDIA ? {} : { mediaDevices },
  innerWidth: 800, innerHeight: 600, devicePixelRatio: 2,
  setTimeout, clearTimeout,
  requestAnimationFrame(f) { setTimeout(f, 0); },
};

__CAPTURE__

const c = idealystTabCapture(win, doc);
let notified = 0;
c.subscribe(() => notified++);
const out = {};
async function attempt(name, share) {
  nextShare = share;
  try { await c.start(); out[name] = { ok: true }; }
  catch (e) { out[name] = { err: String(e.message || e) }; }
  out[name].state = c.state();
  out[name].active = c.active();
}
(async () => {
  out.initial = c.state();
  c.request();
  out.requested = c.state();

  // Another surface: a window, as Firefox's full picker allows.
  let windowTrack;
  await attempt("window", () => streamOf(windowTrack = fakeTrack({ displaySurface: "window" })));
  out.window.trackStopped = !!windowTrack && windowTrack.readyState === "ended";

  // Another tab: its capture handle is not this page's.
  await attempt("otherTab", () => streamOf(fakeTrack({ displaySurface: "browser" }, { handle: "someone-else" })));

  // The user dismissed the dialog.
  await attempt("dismissed", () => { const e = new Error("Permission denied"); e.name = "NotAllowedError"; throw e; });

  // This tab.
  let track;
  await attempt("thisTab", () => streamOf(track = fakeTrack({ displaySurface: "browser" }, { handle: handleConfig.handle })));
  out.handleConfig = handleConfig;
  if (!c.active()) { process.stdout.write(JSON.stringify(out)); return; }
  out.thisTab.options = log.filter(l => l.getDisplayMedia).pop().getDisplayMedia;

  const frame = await c.grab();
  out.frame = frame;
  out.draws = log.filter(l => l.draw);
  out.overlayAfter = host.style.visibility;
  out.afterGrab = c.state();

  // The user pressed the browser's "Stop sharing".
  track.stop();
  track.listeners.ended();
  out.ended = c.state();
  try { await c.grab(); out.grabAfterEnd = "resolved"; } catch (e) { out.grabAfterEnd = String(e.message); }
  out.notified = notified;
  process.stdout.write(JSON.stringify(out));
})().catch(e => { console.error(e); process.exit(1); });
"##;

fn node() -> Option<&'static str> {
    Command::new("node").arg("--version").output().ok()?.status.success().then_some("node")
}

fn run(env: &[(&str, &str)]) -> Option<serde_json::Value> {
    let node = node()?;
    let harness = HARNESS.replace("__CAPTURE__", dev_http::TAB_CAPTURE_JS);
    let out = Command::new(node).args(["-e", &harness]).envs(env.iter().copied()).output().expect("spawn node");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some(serde_json::from_slice(&out.stdout).expect("the harness prints JSON"))
}

fn skip() {
    eprintln!("tab_capture: `node` is not on PATH; skipped");
}

#[test]
fn only_this_tab_is_accepted_and_its_frames_leave_the_overlay_out() {
    let Some(out) = run(&[]) else { return skip() };

    assert_eq!(out["initial"]["supported"], true);
    assert_eq!(out["initial"]["sharing"], false);
    assert_eq!(out["requested"]["requested"], true, "an agent's ask is held for the overlay");

    // Anything but this tab is refused, and the stream the user handed
    // over is stopped rather than kept.
    assert_eq!(out["window"]["err"], "a window was shared, not this tab");
    assert_eq!(out["window"]["trackStopped"], true);
    assert_eq!(out["window"]["active"], false);
    assert_eq!(out["window"]["state"]["error"], "a window was shared, not this tab");
    assert_eq!(out["otherTab"]["err"], "another tab was shared, not this one");
    assert_eq!(out["otherTab"]["active"], false);
    // Dismissing the dialog is the user's choice: nothing to show.
    assert_eq!(out["dismissed"]["state"]["error"], serde_json::Value::Null);

    // This tab: the browser is asked for this tab alone, at device pixels.
    assert_eq!(out["thisTab"]["ok"], true, "{out}");
    assert_eq!(out["thisTab"]["active"], true);
    assert_eq!(out["thisTab"]["state"]["requested"], false, "sharing answers the ask");
    let options = &out["thisTab"]["options"];
    assert_eq!(options["preferCurrentTab"], true);
    assert_eq!(options["surfaceSwitching"], "exclude");
    assert_eq!(options["monitorTypeSurfaces"], "exclude");
    assert_eq!(options["audio"], false);
    assert_eq!(options["video"]["displaySurface"], "browser");
    assert_eq!(options["video"]["width"]["ideal"], 1600);
    assert_eq!(options["video"]["height"]["ideal"], 1200);
    assert_eq!(out["handleConfig"]["permittedOrigins"], serde_json::json!(["*"]));

    // A frame: the PNG without its data-URL prefix, at the video's size,
    // drawn while the overlay was hidden and restored after.
    assert_eq!(out["frame"]["png_base64"], "UE5HREFUQQ==");
    assert_eq!((out["frame"]["width"].as_u64(), out["frame"]["height"].as_u64()), (Some(1600), Some(1200)));
    assert_eq!(out["draws"], serde_json::json!([{ "draw": [1600, 1200], "overlay": "hidden" }]));
    assert_eq!(out["overlayAfter"], "");
    assert!(out["afterGrab"]["lastShot"].as_f64().is_some(), "the overlay can say when");

    // Stopping the share from the browser ends it here too.
    assert_eq!(out["ended"]["sharing"], false);
    assert_eq!(out["grabAfterEnd"], "the tab is not shared");
    assert!(out["notified"].as_u64().unwrap() >= 6, "each change reaches the overlay: {out}");
}

#[test]
fn a_browser_without_get_display_media_is_not_supported() {
    let Some(out) = run(&[("NO_MEDIA", "1")]) else { return skip() };
    assert_eq!(out["initial"]["supported"], false);
    assert_eq!(out["thisTab"]["err"], "this browser cannot share a tab");
    assert_eq!(out["thisTab"]["active"], false);
}

/// `backend-web` finds the controller on this global and dev-http
/// publishes it there; neither crate can depend on the other, so the
/// names are held together here.
#[test]
fn the_controller_is_published_where_the_web_backend_looks() {
    const BACKEND: &str = include_str!("../../../backend/web/src/robot_screenshot_source.rs");
    let decl = format!("pub const TAB_CAPTURE_GLOBAL: &str = \"{}\";", dev_http::TAB_CAPTURE_GLOBAL);
    assert!(BACKEND.contains(&decl), "backend-web declares `{decl}`");
    let script = dev_http::reload_script_tag("/__idealyst/reload");
    assert!(script.contains(&format!("window.{} = capture;", dev_http::TAB_CAPTURE_GLOBAL)));
}
