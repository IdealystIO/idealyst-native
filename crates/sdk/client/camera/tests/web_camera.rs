//! The web camera leg in a real browser. `webdriver.json` next to this
//! crate's manifest gives headless Chrome a fake camera
//! (`--use-fake-device-for-media-stream`) that grants without a prompt.
//!
//! Run with `cargo test -p camera --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS and carries `webdriver.json`).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use camera::{Camera, CameraConfig, VideoFrame};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::{MediaStream as WebMediaStream, MediaStreamTrack, MediaStreamTrackState};
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

fn states(ms: &WebMediaStream) -> Vec<MediaStreamTrackState> {
    ms.get_tracks().iter().map(|t| t.unchecked_into::<MediaStreamTrack>().ready_state()).collect()
}

/// Regression: camera published `Rc::new(stream.clone())`, and web-sys's
/// inherent `MediaStream::clone()` is the JS `clone()` — a NEW stream with
/// cloned tracks. A consumer (the `video` SDK's `<video srcObject>`) showed
/// the copy, and dropping the camera stream stopped only the original's
/// tracks: the preview kept running and the camera stayed on. The published
/// native source must BE the capture stream, so stopping the capture ends
/// what the consumer shows.
#[wasm_bindgen_test]
async fn regression_camera_native_source_is_the_capture_stream() {
    let stream = Camera::new().open(CameraConfig::default()).await.expect("fake camera opens");
    let native = stream.native_source().expect("web publishes a native source");
    let ms = native.downcast_ref::<WebMediaStream>().expect("a web_glue::dom::MediaStream").clone();

    // A consumer, as the video SDK wires it: a <video> showing the stream.
    let consumer = Function::new_with_args(
        "s",
        "const v = document.createElement('video'); v.muted = true; v.srcObject = s; \
         v.play().catch(() => {}); return v;",
    )
    .call1(&JsValue::UNDEFINED, ms.as_js())
    .unwrap();
    let shown: WebMediaStream = consumer.get("srcObject").unwrap().dyn_into().unwrap();
    assert!(!states(&shown).is_empty(), "the capture has tracks");
    assert!(states(&shown).iter().all(|s| *s == MediaStreamTrackState::Live), "capture is live");

    // Stop the capture: drop every `MediaStream` clone (the stopper runs).
    drop(native);
    drop(stream);

    assert!(
        states(&shown).iter().all(|s| *s == MediaStreamTrackState::Ended),
        "stopping the capture must end the tracks the consumer shows: {:?}",
        states(&shown)
    );
}

/// The CPU tap: a subscriber receives real RGBA8 frames pumped off the
/// capture `<video>` through the offscreen canvas.
#[wasm_bindgen_test]
async fn camera_subscriber_receives_rgba_frames() {
    let stream = Camera::new().open(CameraConfig::default()).await.expect("fake camera opens");
    let got: Rc<RefCell<Option<(u32, u32, usize)>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    let _sub = stream.subscribe(move |f: &VideoFrame| {
        *sink.borrow_mut() = Some((f.width, f.height, f.data.len()));
    });
    for _ in 0..100 {
        if got.borrow().is_some() {
            break;
        }
        sleep(20).await;
    }
    let (w, h, len) = got.borrow().expect("a frame within 2 s");
    assert!(w > 0 && h > 0);
    assert_eq!(len, (w * h * 4) as usize, "tightly packed RGBA8");
}
