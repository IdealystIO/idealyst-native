//! The web legs of media-stream, in a real browser: a stream's native source
//! is a `web_glue::dom::MediaStream` — what `screenshot()` reads and what the
//! synthetic-audio bridge publishes.
//!
//! Run with `cargo test -p media-stream --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS).

#![cfg(target_arch = "wasm32")]

use std::any::Any;
use std::rc::Rc;

use media_stream::{AudioStream, MediaStream};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::MediaStream as WebMediaStream;
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// A 64×32 canvas painted solid `rgb(200, 30, 60)` and repainted every
/// 16 ms (so `captureStream` keeps producing frames), and its capture stream.
fn painted_canvas_stream() -> WebMediaStream {
    let f = Function::new_no_args(
        "const c = document.createElement('canvas'); c.width = 64; c.height = 32; \
         const x = c.getContext('2d'); \
         const paint = () => { x.fillStyle = 'rgb(200, 30, 60)'; x.fillRect(0, 0, 64, 32); }; \
         paint(); const s = c.captureStream(); setInterval(paint, 16); return s;",
    );
    f.call0(&JsValue::UNDEFINED).expect("fixture").dyn_into().expect("a MediaStream")
}

#[wasm_bindgen_test]
async fn screenshot_reads_the_native_web_media_stream() {
    let (stream, _writer) = MediaStream::new();
    stream.set_native_source(Rc::new(painted_canvas_stream()) as Rc<dyn Any>);
    let shot = stream.screenshot().await.expect("a frame from the web MediaStream");
    assert_eq!((shot.width, shot.height), (64, 32));
    assert_eq!(shot.data.len(), 64 * 32 * 4);
    let mid = ((16 * 64 + 32) * 4) as usize;
    let px = &shot.data[mid..mid + 4];
    // Video decode may shift colours by a few levels.
    assert!(px[0] > 170 && px[1] < 70 && px[2] < 100 && px[3] == 255, "pixel {px:?}");
}

#[wasm_bindgen_test]
async fn screenshot_without_a_web_media_stream_is_none() {
    let (stream, _writer) = MediaStream::new();
    stream.set_native_source(Rc::new(42u32) as Rc<dyn Any>);
    assert!(stream.screenshot().await.is_none());
}

/// A synthetic `AudioStream` (fed by an `AudioWriter`, no producer-set source)
/// gets a WebAudio-backed native source on web: a `web_glue::dom::MediaStream`
/// with one live audio track, built once and cached.
#[wasm_bindgen_test]
fn synthetic_audio_stream_publishes_a_glue_media_stream() {
    let (audio, _writer) = AudioStream::new();
    let native = audio.native_source().expect("the WebAudio bridge");
    let ms = native.downcast_ref::<WebMediaStream>().expect("a web_glue::dom::MediaStream");
    let tracks = ms.get_audio_tracks();
    assert_eq!(tracks.length(), 1, "one audio track");
    let again = audio.native_source().expect("cached");
    let again = again.downcast_ref::<WebMediaStream>().unwrap();
    assert!(again.as_js().strict_eq(ms.as_js()), "the bridge is built once and cached");
}

/// Resolves after `ms` milliseconds.
async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

/// The bridge's render path end to end: PCM written through the `AudioWriter`
/// comes out of the published track. A second `AudioContext` taps the track
/// with an `AnalyserNode` (pulled by connecting it to its destination) and
/// reports the peak it hears. Needs the autoplay policy relaxed so both
/// contexts run without a gesture — `webdriver.json` next to this crate's
/// manifest.
#[wasm_bindgen_test]
async fn synthetic_audio_pcm_reaches_the_published_track() {
    let (audio, writer) = AudioStream::new();
    let native = audio.native_source().expect("the WebAudio bridge");
    let ms = native.downcast_ref::<WebMediaStream>().unwrap().clone();
    let tap = Function::new_with_args(
        "s",
        "const c = new AudioContext(); c.resume(); const a = c.createAnalyser(); a.fftSize = 2048; \
         c.createMediaStreamSource(s).connect(a); a.connect(c.destination); \
         return { c, peak: () => { const d = new Float32Array(a.fftSize); a.getFloatTimeDomainData(d); \
           let m = 0; for (const v of d) m = Math.max(m, Math.abs(v)); return m; } };",
    )
    .call1(&JsValue::UNDEFINED, ms.as_js())
    .unwrap();
    // A 440 Hz tone at 0.5 amplitude, fed in 10 ms chunks for ~1.5 s.
    let mut peak = 0.0f64;
    let mut phase = 0.0f32;
    for round in 0..150 {
        let chunk: Vec<f32> = (0..480)
            .map(|_| {
                phase += 440.0 / 48_000.0 * std::f32::consts::TAU;
                0.5 * phase.sin()
            })
            .collect();
        writer.write_pcm_f32(48_000, 1, &chunk);
        sleep(10).await;
        if round > 50 {
            let p = tap.get("peak").unwrap().call(&tap, &[]).unwrap().as_f64().unwrap_or(0.0);
            peak = peak.max(p);
        }
    }
    let state = tap.get("c").unwrap().get("state").unwrap().as_string().unwrap_or_default();
    assert!(peak > 0.2, "the published track carries the written PCM (peak {peak}, tap context {state})");
}

