//! The web screen-recorder leg in a real browser.
//!
//! `getDisplayMedia` needs a user gesture and a picker, neither of which a
//! headless test has, so the test stands in for the picker: it replaces
//! `navigator.mediaDevices.getDisplayMedia` with one that resolves to a
//! painted `<canvas>`'s `captureStream()` — a real `MediaStream` with a real
//! video track — and records the constraints it was called with. Everything
//! after the call is the SDK's own code.
//!
//! Run with `cargo test -p screen-recorder --target wasm32-unknown-unknown`.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use screen_recorder::{RecordingConfig, ScreenRecorder, Source, VideoFrame};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::{MediaStream as WebMediaStream, MediaStreamTrack, MediaStreamTrackState};
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// Install the stand-in picker; returns the object it records into
/// (`{ stream, constraints }`).
fn fake_display_media() -> JsValue {
    Function::new_no_args(
        "const rec = {}; \
         const c = document.createElement('canvas'); c.width = 48; c.height = 24; \
         const x = c.getContext('2d'); \
         const paint = () => { x.fillStyle = 'rgb(20, 180, 40)'; x.fillRect(0, 0, 48, 24); }; \
         paint(); setInterval(paint, 16); \
         navigator.mediaDevices.getDisplayMedia = (k) => { \
           rec.constraints = k; rec.stream = c.captureStream(); return Promise.resolve(rec.stream); }; \
         return rec;",
    )
    .call0(&JsValue::UNDEFINED)
    .unwrap()
}

async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

fn states(ms: &WebMediaStream) -> Vec<MediaStreamTrackState> {
    ms.get_tracks().iter().map(|t| t.unchecked_into::<MediaStreamTrack>().ready_state()).collect()
}

/// Regression: the web recorder published the capture stream but kept
/// `stream.clone()` for teardown — web-sys's inherent `clone()` is the JS
/// `MediaStream.clone()`, so stopping the recording stopped a copy's tracks
/// and the published capture (what a preview shows) kept running, and with
/// it the browser's "sharing this screen" UI. The native source must BE the
/// capture stream, and stopping the recording must end its tracks.
#[wasm_bindgen_test]
async fn regression_stopping_the_recording_ends_the_published_capture() {
    let rec = fake_display_media();
    let config = RecordingConfig { source: Source::ThisApp, ..RecordingConfig::default() };
    let stream = ScreenRecorder::new().start(config).await.expect("start");

    let captured: WebMediaStream = rec.get("stream").unwrap().dyn_into().unwrap();
    let constraints = rec.get("constraints").unwrap();
    assert_eq!(constraints.get("video").unwrap().as_bool(), Some(true));
    assert_eq!(constraints.get("preferCurrentTab").unwrap().as_bool(), Some(true));

    let native = stream.native_source().expect("web publishes a native source");
    let ms = native.downcast_ref::<WebMediaStream>().expect("a web_glue::dom::MediaStream");
    assert!(ms.as_js().strict_eq(captured.as_js()), "the native source IS the capture stream");
    assert!(states(&captured).iter().all(|s| *s == MediaStreamTrackState::Live));

    drop(native);
    drop(stream);
    assert!(
        states(&captured).iter().all(|s| *s == MediaStreamTrackState::Ended),
        "stopping the recording must end the capture's tracks: {:?}",
        states(&captured)
    );
}

/// The CPU tap: a subscriber receives the stand-in screen's RGBA8 frames.
#[wasm_bindgen_test]
async fn screen_recorder_subscriber_receives_rgba_frames() {
    let _rec = fake_display_media();
    let stream = ScreenRecorder::new().start(RecordingConfig::default()).await.expect("start");
    let got: Rc<RefCell<Option<(u32, u32, [u8; 4])>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    let _sub = stream.subscribe(move |f: &VideoFrame| {
        let px = [f.data[0], f.data[1], f.data[2], f.data[3]];
        *sink.borrow_mut() = Some((f.width, f.height, px));
    });
    for _ in 0..100 {
        if got.borrow().is_some() {
            break;
        }
        sleep(20).await;
    }
    let (w, h, px) = got.borrow().expect("a frame within 2 s");
    assert_eq!((w, h), (48, 24));
    assert!(px[1] > 140 && px[0] < 70 && px[3] == 255, "pixel {px:?}");
}

/// Regression (same bug class as the camera pump's teardown error): the
/// pump's `setInterval` id crossed into wasm as an `i32`. Under a fake clock
/// that numbers timers from `1e12` (Playwright's `page.clock`, which
/// `setFixedTime` installs) the id was truncated, `Drop`'s `clearInterval`
/// missed, and the interval kept firing into the dropped pump closure —
/// `web-glue: callback #N called after its Rust owner dropped it` on every
/// tick. Stopping the recording must clear the host's interval.
#[wasm_bindgen_test]
async fn regression_stopping_the_recording_clears_its_interval_under_a_fake_clock() {
    let _rec = fake_display_media();
    // A manual setInterval with Playwright-style ids. Installed AFTER the
    // stand-in picker, whose own paint interval stays on the real clock.
    Function::new_no_args(
        "const w = window; const real = { si: w.setInterval, ci: w.clearInterval }; \
         let next = 1e12; const live = new Map(); \
         w.setInterval = (f) => { const id = next++; live.set(id, f); return id; }; \
         w.clearInterval = (id) => { live.delete(Number(id)); }; \
         w.__fakeInterval = { \
           live: () => live.size, \
           tick: () => { const errs = []; for (const f of live.values()) { \
                            try { f(); } catch (e) { errs.push(String(e.message || e)); } } \
                          return errs.join('\\n'); }, \
           restore: () => { w.setInterval = real.si; w.clearInterval = real.ci; delete w.__fakeInterval; }, \
         };",
    )
    .call0(&JsValue::UNDEFINED)
    .unwrap();
    let fake = |m: &str| {
        Function::new_no_args(&format!("return window.__fakeInterval.{m}();"))
            .call0(&JsValue::UNDEFINED)
            .unwrap()
    };

    let stream = ScreenRecorder::new().start(RecordingConfig::default()).await;
    let before = fake("live").as_f64();
    drop(stream);
    let after = fake("live").as_f64();
    let errors = fake("tick").as_string().unwrap_or_default();
    fake("restore");

    assert_eq!(before, Some(1.0), "the recording runs one pump interval");
    assert_eq!(after, Some(0.0), "stopping the recording must clear its interval");
    assert_eq!(errors, "", "no tick may fire into the dropped pump");
}
