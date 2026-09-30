//! `MediaStream` / `MediaStreamTrack` — the handle the media SDKs pass each
//! other as a live stream's web `native_source` — in a real browser.
//!
//! The source is a `<canvas>`'s `captureStream()`: headless Chrome has no
//! camera, and a canvas track is a real `MediaStreamTrack` with the same
//! lifecycle (`live` until `stop()`).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::{MediaStream, MediaStreamTrack, MediaStreamTrackState};
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn canvas_stream() -> MediaStream {
    let doc = web_glue::dom::window().unwrap().document().unwrap();
    let canvas = doc.create_element("canvas").unwrap();
    let stream = canvas.as_js().call_method("captureStream", &[]).expect("captureStream");
    stream.dyn_into::<MediaStream>().expect("captureStream returns a MediaStream")
}

fn first_video_track(s: &MediaStream) -> MediaStreamTrack {
    s.get_video_tracks().get(0).dyn_into::<MediaStreamTrack>().expect("a video track")
}

/// `Clone` on the handle must be the SAME stream. web-sys binds the JS
/// `MediaStream.clone()` method as an inherent `clone()`, so there
/// `stream.clone()` was a new stream with copied tracks — stopping the
/// capture then left the copy running (the camera / microphone bug).
#[wasm_bindgen_test]
fn regression_media_stream_clone_is_the_same_stream() {
    let a = canvas_stream();
    let b = a.clone();
    assert!(a.as_js().strict_eq(b.as_js()), "a cloned handle must be the same JS object");
    assert_eq!(a.id(), b.id());
    first_video_track(&a).stop();
    assert_eq!(first_video_track(&b).ready_state(), MediaStreamTrackState::Ended);
}

#[wasm_bindgen_test]
fn track_lifecycle_and_accessors() {
    let s = canvas_stream();
    assert!(!s.id().is_empty());
    assert!(s.active());
    assert_eq!(s.get_tracks().length(), 1);
    assert_eq!(s.get_audio_tracks().length(), 0);
    let t = first_video_track(&s);
    assert_eq!(t.kind(), "video");
    assert!(!t.id().is_empty());
    assert!(t.enabled());
    t.set_enabled(false);
    assert!(!t.enabled());
    assert_eq!(t.ready_state(), MediaStreamTrackState::Live);
    t.stop();
    assert_eq!(t.ready_state(), MediaStreamTrackState::Ended);
}

#[wasm_bindgen_test]
fn compose_tracks_into_a_new_stream() {
    let src = canvas_stream();
    let track = first_video_track(&src);

    let empty = MediaStream::new().expect("new MediaStream()");
    assert_eq!(empty.get_tracks().length(), 0);
    empty.add_track(&track);
    assert_eq!(empty.get_video_tracks().length(), 1);
    assert!(empty.get_tracks().get(0).strict_eq(track.as_js()), "addTrack shares the track");
    empty.remove_track(&track);
    assert_eq!(empty.get_tracks().length(), 0);

    let tracks = web_glue::js::Array::new();
    tracks.push(track.as_js());
    let with = MediaStream::new_with_tracks(&tracks).expect("new MediaStream(tracks)");
    assert_eq!(with.get_tracks().length(), 1);
    assert_ne!(with.id(), src.id(), "a new stream has its own id");
    // Not a MediaStream: the checked cast refuses it.
    assert!(JsValue::from_str("x").dyn_into::<MediaStream>().is_err());
}
