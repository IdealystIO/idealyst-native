//! The web microphone leg in a real browser. `webdriver.json` next to this
//! crate's manifest gives headless Chrome a fake microphone
//! (`--use-fake-device-for-media-stream`, a beeping tone) that grants without
//! a prompt, and relaxes the autoplay policy so the capture `AudioContext`
//! runs without a gesture.
//!
//! Run with `cargo test -p microphone --target wasm32-unknown-unknown`.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use microphone::{AudioBuffer, AudioStreamConfig, Microphone};
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

/// Regression: microphone published `Rc::new(self.stream.clone())`, and
/// web-sys's inherent `MediaStream::clone()` is the JS `clone()` — a NEW
/// stream with cloned tracks. A consumer (media-writer's `MediaRecorder`, an
/// `<audio srcObject>`) got the copy, and stopping the microphone stopped
/// only the original's tracks. The published native source must BE the
/// capture stream.
#[wasm_bindgen_test]
async fn regression_microphone_native_source_is_the_capture_stream() {
    let stream = Microphone::new().open_stream(AudioStreamConfig::default()).await.expect("fake mic opens");
    let native = stream.native_source().expect("web publishes a native source");
    let ms = native.downcast_ref::<WebMediaStream>().expect("a web_glue::dom::MediaStream").clone();
    // A consumer holding the stream (as an <audio srcObject> would).
    let consumer = Function::new_with_args("s", "const a = document.createElement('audio'); a.srcObject = s; return a;")
        .call1(&JsValue::UNDEFINED, ms.as_js())
        .unwrap();
    let shown: WebMediaStream = consumer.get("srcObject").unwrap().dyn_into().unwrap();
    assert_eq!(shown.get_audio_tracks().length(), 1);
    assert!(states(&shown).iter().all(|s| *s == MediaStreamTrackState::Live));

    drop(native);
    drop(stream);

    assert!(
        states(&shown).iter().all(|s| *s == MediaStreamTrackState::Ended),
        "stopping the microphone must end the tracks the consumer holds: {:?}",
        states(&shown)
    );
}

/// The PCM path: the Web Audio graph delivers interleaved f32 chunks at the
/// context's rate.
#[wasm_bindgen_test]
async fn microphone_callback_receives_pcm() {
    let got: Rc<RefCell<Option<(u32, u16, usize)>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    let mic = Microphone::new()
        .open(AudioStreamConfig::default(), move |b: &AudioBuffer| {
            *sink.borrow_mut() = Some((b.sample_rate, b.channels, b.samples.len()));
        })
        .await
        .expect("fake mic opens");
    for _ in 0..100 {
        if got.borrow().is_some() {
            break;
        }
        sleep(20).await;
    }
    let (rate, channels, len) = got.borrow().expect("a PCM chunk within 2 s");
    assert!(rate > 0);
    assert_eq!(channels, 1, "mono by default");
    assert_eq!(len, 4096, "one ScriptProcessor block");
    mic.stop();
}
