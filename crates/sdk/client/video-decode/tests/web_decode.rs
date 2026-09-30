//! Browser tests for the web decode backend: the hidden `<video>`, the
//! offscreen-canvas RGBA pump, the transport, and teardown.
//!
//! The clip is made in the page itself — a solid-red canvas recorded with
//! `MediaRecorder` into a WebM — so the test needs no fixture file and
//! decodes real media. The frame pump runs on `raf_loop`, which needs a
//! scheduler; a minimal one over `setInterval` is installed here (the app
//! gets backend-web's).
//!
//! Run with `cargo test -p video-decode --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use video_decode::{DecodeConfig, DecodeSource, VideoDecoder};
use wasm_bindgen_test::*;
use web_glue::{Closure, JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

async fn sleep(ms: u32) {
    let p = eval(&format!("return new Promise((r) => setTimeout(r, {ms}));"));
    let _ = JsFuture::new(&p).await;
}

// ---- a minimal scheduler for raf_loop -------------------------------------

/// `setInterval(f, ms)` → id; `clearInterval(id)`.
fn set_interval(f: &Closure, ms: u32) -> f64 {
    let set = JsValue::global().get("setInterval").unwrap();
    set.call(&JsValue::undefined(), &[f.as_js(), &JsValue::from_f64(ms as f64)])
        .unwrap()
        .as_f64()
        .unwrap()
}

struct Interval {
    id: f64,
    _f: Closure,
}

impl ScheduleHandle for Interval {
    fn cancel(&mut self) {
        let clear = JsValue::global().get("clearInterval").unwrap();
        let _ = clear.call(&JsValue::undefined(), &[&JsValue::from_f64(self.id)]);
    }
}

impl Drop for Interval {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct TestScheduler;

impl Scheduler for TestScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        web_glue::queue_microtask(f);
    }
    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        self.after_ms(16, f)
    }
    fn after_ms(&self, delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        let f = RefCell::new(Some(f));
        let cb = Closure::new(move |_| {
            if let Some(f) = f.borrow_mut().take() {
                f();
            }
        });
        let id = set_interval(&cb, delay_ms.max(0) as u32);
        Box::new(Interval { id, _f: cb })
    }
    fn raf_loop(&self, mut f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let cb = Closure::new(move |_| f());
        let id = set_interval(&cb, 16);
        Box::new(Interval { id, _f: cb })
    }
}

// ---- the clip --------------------------------------------------------------

/// A ~0.8 s solid-red 64×48 WebM, recorded in the page; its blob URL.
async fn red_clip_url() -> String {
    let p = eval(
        "const c = document.createElement('canvas'); c.width = 64; c.height = 48; \
         const x = c.getContext('2d'); \
         const paint = () => { x.fillStyle = 'rgb(255,0,0)'; x.fillRect(0, 0, 64, 48); }; \
         paint(); const s = c.captureStream(30); \
         const r = new MediaRecorder(s, { mimeType: 'video/webm' }); const parts = []; \
         r.ondataavailable = (e) => parts.push(e.data); \
         const t = setInterval(paint, 30); r.start(); \
         return new Promise((ok) => { r.onstop = () => { clearInterval(t); \
           ok(URL.createObjectURL(new Blob(parts, { type: 'video/webm' }))); }; \
           setTimeout(() => r.stop(), 800); });",
    );
    JsFuture::new(&p).await.unwrap().as_string().unwrap()
}

fn hidden_videos() -> f64 {
    eval("return document.querySelectorAll('body > video').length;").as_f64().unwrap()
}

#[wasm_bindgen_test]
async fn decodes_rgba_frames_and_tears_down() {
    install_scheduler(Box::new(TestScheduler));
    let url = red_clip_url().await;
    let before = hidden_videos();

    let clip = VideoDecoder::new()
        .open(
            DecodeSource::url(url),
            DecodeConfig { autoplay: true, muted: true, loop_playback: true, max_dimension: Some(32) },
        )
        .await
        .expect("open");
    assert_eq!(hidden_videos(), before + 1.0, "the hidden <video> is in the document");

    let first: Rc<RefCell<Option<(u32, u32, [u8; 4])>>> = Rc::new(RefCell::new(None));
    let sink = first.clone();
    let _sub = clip.frames().subscribe(move |f: &media_stream::VideoFrame| {
        let mut s = sink.borrow_mut();
        if s.is_none() {
            let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
            *s = Some((f.width, f.height, [f.data[mid], f.data[mid + 1], f.data[mid + 2], f.data[mid + 3]]));
        }
    });

    for _ in 0..100 {
        if first.borrow().is_some() {
            break;
        }
        sleep(50).await;
    }
    let (w, h, px) = first.borrow().expect("a decoded frame reached the subscriber");
    // max_dimension 32 downscales 64×48 aspect-preserving.
    assert_eq!((w, h), (32, 24));
    // Solid red, allowing for codec loss.
    assert!(px[0] > 200 && px[1] < 60 && px[2] < 60 && px[3] == 255, "{px:?}");

    let t = clip.transport();
    assert!(t.is_muted());
    t.set_muted(false);
    assert!(!t.is_muted());
    t.pause();
    assert!(!t.is_playing());
    assert!(t.duration() >= 0.0);

    drop(_sub);
    drop(clip);
    assert_eq!(hidden_videos(), before, "dropping the clip removes its <video>");
}

/// A `Bytes` source plays from a Blob object URL that's revoked on teardown.
#[wasm_bindgen_test]
async fn bytes_source_uses_a_blob_url_revoked_on_drop() {
    install_scheduler(Box::new(TestScheduler));
    let clip = VideoDecoder::new()
        .open(DecodeSource::bytes(vec![0u8; 16]), DecodeConfig::default())
        .await
        .expect("open");
    let src = eval("const v = document.querySelector('body > video:last-of-type'); return v.src;")
        .as_string()
        .unwrap();
    assert!(src.starts_with("blob:"), "{src}");
    drop(clip);
    let live = JsFuture::new(&eval(&format!("return fetch('{src}').then(() => true, () => false);")))
        .await
        .unwrap()
        .as_bool()
        .unwrap();
    assert!(!live, "the clip's object URL is revoked on drop");
}
