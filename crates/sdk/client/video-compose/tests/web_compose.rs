//! The web compositing driver in a real browser: the input's native source
//! and the published output are `web_glue::dom::MediaStream`s, and the output
//! carries the composited frame (input + watermark).
//!
//! Run with `cargo test -p video-compose --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS).

#![cfg(target_arch = "wasm32")]

use std::any::Any;
use std::rc::Rc;

use canvas_core::ImageSource;
use media_stream::MediaStream;
use video_compose::{Corner, VideoPipeline};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::MediaStream as WebMediaStream;
use web_glue::js::Function;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// A 64×48 canvas painted solid blue, repainted every 16 ms, captured.
fn blue_input() -> MediaStream {
    let web: WebMediaStream = Function::new_no_args(
        "const c = document.createElement('canvas'); c.width = 64; c.height = 48; \
         const x = c.getContext('2d'); \
         const paint = () => { x.fillStyle = 'rgb(20, 40, 220)'; x.fillRect(0, 0, 64, 48); }; \
         paint(); const s = c.captureStream(); setInterval(paint, 16); return s;",
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
async fn web_pipeline_publishes_a_composited_glue_media_stream() {
    // A 4×4 opaque yellow watermark in the top-left corner, no margin.
    let yellow = ImageSource::from_rgba8(1, 4, 4, [250u8, 230, 20, 255].repeat(16));
    let out = VideoPipeline::new(blue_input()).watermark(yellow, Corner::TopLeft, 0.0, || 1.0).build();

    let native = out.native_source().expect("the output publishes its captureStream");
    let ms = native.downcast_ref::<WebMediaStream>().expect("a web_glue::dom::MediaStream");
    assert_eq!(ms.get_video_tracks().length(), 1);

    let shot = out.screenshot().await.expect("an output frame");
    assert_eq!((shot.width, shot.height), (64, 48), "output sized to the input");
    let px = |x: u32, y: u32| {
        let i = ((y * shot.width + x) * 4) as usize;
        [shot.data[i], shot.data[i + 1], shot.data[i + 2]]
    };
    let [r, g, b] = px(40, 30);
    assert!(b > 180 && r < 70 && g < 90, "input composited: {:?}", [r, g, b]);
    let [r, g, b] = px(1, 1);
    assert!(r > 200 && g > 190 && b < 80, "watermark composited: {:?}", [r, g, b]);
}
