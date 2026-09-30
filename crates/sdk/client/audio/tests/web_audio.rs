//! Browser tests for the web audio backend (`HTMLAudioElement`).
//!
//! Headless Chrome won't play audio without a gesture, so `globalThis.Audio`
//! is replaced per test with a stand-in that records what the SDK does to
//! it; the Blob / object-URL half runs against the real browser.
//!
//! Run with `cargo test -p audio --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use audio::{load, AudioSource};
use wasm_bindgen_test::*;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// A stand-in `Audio` class: every instance is pushed to `__audios`.
struct AudioOverride;

impl AudioOverride {
    fn install() -> AudioOverride {
        eval(
            "globalThis.__savedAudio = globalThis.Audio; globalThis.__audios = []; \
             globalThis.Audio = class { \
               constructor(src) { this.src = src; this.paused = true; this.ended = false; \
                 this.volume = 1; this.loop = false; this.plays = 0; __audios.push(this); } \
               play() { this.plays++; this.paused = false; \
                 return Promise.reject(new DOMException('no gesture', 'NotAllowedError')); } \
               pause() { this.paused = true; } };",
        );
        AudioOverride
    }
}

impl Drop for AudioOverride {
    fn drop(&mut self) {
        eval("globalThis.Audio = globalThis.__savedAudio; delete globalThis.__audios;");
    }
}

fn audio(i: usize) -> JsValue {
    JsValue::global().get("__audios").unwrap().get(&i.to_string()).unwrap()
}

/// Does `fetch(url)` still resolve (i.e. the object URL is live)?
async fn url_is_live(url: &str) -> bool {
    let p = eval(&format!("return fetch('{url}').then(() => true, () => false);"));
    JsFuture::new(&p).await.unwrap().as_bool().unwrap()
}

#[wasm_bindgen_test]
async fn bytes_play_from_a_blob_url_that_is_revoked_with_the_sound() {
    let _a = AudioOverride::install();
    let sound = load(AudioSource::bytes(vec![1u8, 2, 3, 4])).await.expect("load");
    let playback = sound.play();
    let src = audio(0).get("src").unwrap().as_string().unwrap();
    assert!(src.starts_with("blob:"), "{src}");
    assert!(url_is_live(&src).await, "the blob URL serves the bytes while the sound lives");
    assert_eq!(audio(0).get("plays").unwrap().as_f64(), Some(1.0));

    drop(playback);
    drop(sound);
    assert!(!url_is_live(&src).await, "dropping the sound revokes its object URL");
}

#[wasm_bindgen_test]
async fn playback_controls_reach_the_element() {
    let _a = AudioOverride::install();
    let sound = load(AudioSource::url("https://example.invalid/a.mp3")).await.unwrap();
    let a = sound.play();
    let el = audio(0);
    assert_eq!(el.get("src").unwrap().as_string().as_deref(), Some("https://example.invalid/a.mp3"));
    assert!(a.is_playing());

    a.set_volume(0.25);
    a.set_looping(true);
    assert_eq!(el.get("volume").unwrap().as_f64(), Some(0.25));
    assert_eq!(el.get("loop").unwrap().as_bool(), Some(true));

    a.pause();
    assert!(!a.is_playing());
    a.resume();
    assert_eq!(el.get("plays").unwrap().as_f64(), Some(2.0));

    // Overlapping voices: each play() is a fresh element.
    let _b = sound.play();
    assert!(!audio(1).is_undefined());

    // Stop releases the element: paused, source detached.
    a.stop();
    assert_eq!(el.get("paused").unwrap().as_bool(), Some(true));
    assert_eq!(el.get("src").unwrap().as_string().as_deref(), Some(""));
}
