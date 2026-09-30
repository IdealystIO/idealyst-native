//! The web recorder end to end in a real browser: a native video
//! `MediaStream` (a painted canvas's `captureStream`) plus a synthetic audio
//! stream (media-stream's WebAudio bridge) — both `web_glue::dom::MediaStream`
//! native sources — recorded by `MediaRecorder` into a file store.
//!
//! `webdriver.json` next to this crate's manifest relaxes the autoplay policy
//! so the audio bridge's `AudioContext` runs without a gesture.
//!
//! Run with `cargo test -p media-writer --target wasm32-unknown-unknown`.

#![cfg(target_arch = "wasm32")]

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use files::{FileFuture, FileStore};
use media_stream::{AudioStream, MediaStream};
use media_writer::{MediaInputs, MediaWriter, RecordConfig};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::MediaStream as WebMediaStream;
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// An in-memory store — the recorder's destination, readable back here.
#[derive(Default)]
struct MemStore(Mutex<HashMap<String, Vec<u8>>>);

impl FileStore for MemStore {
    fn read(&self, path: &str) -> FileFuture<'_, Option<Vec<u8>>> {
        let v = self.0.lock().unwrap().get(path).cloned();
        Box::pin(async move { Ok(v) })
    }
    fn write(&self, path: &str, bytes: &[u8]) -> FileFuture<'_, ()> {
        self.0.lock().unwrap().insert(path.to_string(), bytes.to_vec());
        Box::pin(async { Ok(()) })
    }
    fn delete(&self, path: &str) -> FileFuture<'_, ()> {
        self.0.lock().unwrap().remove(path);
        Box::pin(async { Ok(()) })
    }
    fn exists(&self, path: &str) -> FileFuture<'_, bool> {
        let e = self.0.lock().unwrap().contains_key(path);
        Box::pin(async move { Ok(e) })
    }
    fn list(&self, _dir: &str) -> FileFuture<'_, Vec<String>> {
        let v = self.0.lock().unwrap().keys().cloned().collect();
        Box::pin(async move { Ok(v) })
    }
    fn local_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }
}

async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

fn painted_video() -> MediaStream {
    let web: WebMediaStream = Function::new_no_args(
        "const c = document.createElement('canvas'); c.width = 64; c.height = 48; \
         const x = c.getContext('2d'); let n = 0; \
         const paint = () => { x.fillStyle = `rgb(${n++ % 255}, 90, 160)`; x.fillRect(0, 0, 64, 48); }; \
         paint(); const s = c.captureStream(30); setInterval(paint, 16); return s;",
    )
    .call0(&JsValue::UNDEFINED)
    .unwrap()
    .dyn_into()
    .unwrap();
    let (stream, _writer) = MediaStream::new();
    stream.set_native_source(Rc::new(web) as Rc<dyn Any>);
    stream
}

#[wasm_bindgen_test]
async fn records_native_video_and_bridged_audio_to_the_store() {
    let video = painted_video();
    let (audio, pcm) = AudioStream::new();
    let store = Arc::new(MemStore::default());
    let rec = MediaWriter::new()
        .record(
            MediaInputs { video: Some(&video), audio: Some(&audio) },
            RecordConfig::new(store.clone(), "clip.mp4"),
        )
        .await
        .expect("record");
    for _ in 0..40 {
        pcm.write_pcm_f32(48_000, 1, &[0.25f32; 480]);
        sleep(15).await;
    }
    let path = rec.stop().await.expect("stop");
    assert_eq!(path, "clip.mp4");
    let bytes = store.0.lock().unwrap().get("clip.mp4").cloned().expect("written to the store");
    assert!(bytes.len() > 256, "a real recording ({} bytes)", bytes.len());
    let webm = bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]);
    let mp4 = bytes.len() > 8 && &bytes[4..8] == b"ftyp";
    assert!(webm || mp4, "a WebM or MP4 container: {:02x?}", &bytes[..8]);
}
