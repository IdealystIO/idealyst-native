//! Browser tests for the web `Video` handler: the `<video>` element and its
//! construction-time props, the reactive source (URL / live stream /
//! none), and the `VideoHandle` ops reaching the element through the host
//! node.
//!
//! Run with `cargo test -p video --target wasm32-unknown-unknown` (the
//! workspace runner supplies web-glue's JS; `wasm-pack test` cannot).

#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use runtime_shared::Ref;
use runtime_vocabulary::glue::IntoElement;
use runtime_world::signal;
use video::prelude::*;
use video::ObjectFit;
use wasm_bindgen_test::*;
use web_glue::dom::{window, Element};
use web_glue::js::Promise;
use web_glue::{JsFuture, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

async fn next_frames() {
    for _ in 0..2 {
        let promise = Promise::new(&mut |resolve, _| {
            window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 30)
                .unwrap();
        });
        JsFuture::from(JsValue::from(promise)).await.unwrap();
    }
}

fn fresh_host() -> Element {
    backend_web::newcore::stop();
    let doc = window().unwrap().document().unwrap();
    if let Some(old) = doc.get_element_by_id("app") {
        old.remove();
    }
    let host = doc.create_element("div").unwrap();
    host.set_id("app");
    doc.body().unwrap().append_child(&host).unwrap();
    host
}

fn video_el(host: &Element) -> Element {
    host.query_selector("video[data-external-kind='video::VideoProps']")
        .unwrap()
        .expect("the <video> is mounted")
}

fn prop(el: &Element, key: &str) -> JsValue {
    el.as_js().get(key).unwrap()
}

#[wasm_bindgen_test]
async fn url_source_props_and_ops_reach_the_element() {
    let host = fresh_host();
    let r: Ref<VideoHandle> = Ref::new();
    let r_app = r.clone();
    backend_web::newcore::start_in("#app", video::register, move || {
        Video(VideoProps {
            source: url("clip.webm"),
            autoplay: true,
            controls: true,
            object_fit: ObjectFit::Cover,
            ..Default::default()
        })
        .bind(r_app.clone())
        .into_element()
    });
    next_frames().await;

    let v = video_el(&host);
    assert!(v.get_attribute("src").unwrap_or_default().ends_with("clip.webm"));
    assert!(v.has_attribute("autoplay") && v.has_attribute("controls"));
    assert!(!v.has_attribute("loop"));
    assert_eq!(prop(&v, "muted").as_bool(), Some(true), "autoplay forces the muted PROPERTY");
    let style = v.get_attribute("style").unwrap_or_default();
    assert!(style.contains("object-fit: cover"), "{style}");

    // The ops find the element behind the host node.
    let h = r.with(|h| h.clone()).expect("ref filled at mount");
    h.set_muted(false);
    assert_eq!(prop(&v, "muted").as_bool(), Some(false));
    h.pause();
    assert_eq!(prop(&v, "paused").as_bool(), Some(true));
    // No decodable media: duration is NaN in the element, reported as 0.
    assert_eq!(h.duration(), 0.0);
    // play() on a src that never loads rejects; the binding observes the
    // rejection, so no unhandled rejection reaches the page.
    h.play();
    backend_web::newcore::stop();
}

/// A custom reactive source (the extension point `VideoSource` is).
struct Pick(Box<dyn Fn() -> MediaContent>);
impl VideoSource for Pick {
    fn resolve(&self) -> MediaContent {
        (self.0)()
    }
}

/// A live stream source attaches the stream's native `MediaStream` (a
/// `web_glue::dom::MediaStream`, what the capture SDKs publish) as
/// `srcObject`; switching the reactive source to a URL clears it, and
/// `None` clears both.
#[wasm_bindgen_test]
async fn stream_source_sets_src_object_and_switches_reactively() {
    let host = fresh_host();
    let native = web_glue::dom::MediaStream::new().unwrap();
    let (ms, _writer) = media_stream::MediaStream::new();
    ms.set_native_source(Rc::new(native.clone()));

    let which = Rc::new(std::cell::Cell::new(None));
    let which_app = which.clone();
    let ms_app = ms.clone();
    backend_web::newcore::start_in("#app", video::register, move || {
        let pick = signal(0u8);
        which_app.set(Some(pick));
        let ms = ms_app.clone();
        Video(VideoProps {
            source: Box::new(Pick(Box::new(move || match pick.get() {
                0 => MediaContent::Stream(ms.clone()),
                1 => MediaContent::Url("next.webm".to_string()),
                _ => MediaContent::None,
            }))),
            ..Default::default()
        })
        .into_element()
    });
    next_frames().await;

    let v = video_el(&host);
    let so = prop(&v, "srcObject");
    assert!(so.strict_eq(native.as_js()), "the native stream is the srcObject");
    assert!(v.has_attribute("playsinline"));
    assert!(!v.has_attribute("src"));

    let pick = which.get().expect("signal created");
    pick.set(1);
    backend_web::newcore::flush_sync();
    assert!(prop(&v, "srcObject").is_null(), "a URL source clears srcObject");
    assert!(v.get_attribute("src").unwrap_or_default().ends_with("next.webm"));

    pick.set(2);
    backend_web::newcore::flush_sync();
    assert!(prop(&v, "srcObject").is_null());
    assert!(!v.has_attribute("src"), "None clears the URL too");
    backend_web::newcore::stop();
}

/// Anything other than a `web_glue::dom::MediaStream` as the native source
/// (another platform's type, a stale producer) is ignored: no `srcObject`.
#[wasm_bindgen_test]
async fn stream_source_ignores_a_foreign_native_source() {
    let host = fresh_host();
    let (ms, _writer) = media_stream::MediaStream::new();
    ms.set_native_source(Rc::new(7u32));
    backend_web::newcore::start_in("#app", video::register, move || {
        Video(VideoProps { source: video::stream(ms.clone()), ..Default::default() }).into_element()
    });
    next_frames().await;
    let v = video_el(&host);
    assert!(prop(&v, "srcObject").is_null(), "a foreign native source is not attached");
    backend_web::newcore::stop();
}
