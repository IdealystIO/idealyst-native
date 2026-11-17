//! The fake camera on the web, in a real browser: a canvas-backed browser
//! stream through the real pipeline. A robot build:
//! `cargo test -p camera --target wasm32-unknown-unknown --features robot --test web_robot_camera`.

#![cfg(all(target_arch = "wasm32", feature = "robot"))]

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine as _;
use camera::{Camera, CameraConfig, VideoFrame};
use runtime_vocabulary::robot::bridge::invoke_command;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::js::Function;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

async fn sleep(ms: u32) {
    let p = Function::new_with_args("ms", "return new Promise((r) => setTimeout(r, ms));")
        .call1(&JsValue::UNDEFINED, &JsValue::from_f64(ms as f64))
        .unwrap();
    let _ = web_glue::JsFuture::new(&p).await;
}

/// want_18fc6ca7 on the web: the faked camera's stream has a browser
/// `MediaStream` behind it (so a `<video>` preview plays it like a real
/// camera) and its frames are the test image.
#[wasm_bindgen_test]
async fn regression_web_fake_camera_plays_the_test_image() {
    camera::register_robot_controls();
    let (w, h) = (32u32, 32u32);
    let red: Vec<u8> = [220u8, 30, 30, 255].repeat((w * h) as usize);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&red);
    let args = serde_json::json!({ "name": "camera.fake", "args": { "width": w, "height": h, "rgba": b64 } });
    invoke_command("call_function", &args).unwrap();

    let stream = Camera::new().open(CameraConfig::default()).await.expect("the fake opens");
    assert!(stream.native_source().is_some(), "a browser stream backs it, for <video>");

    let got: Rc<RefCell<Option<(u32, u32, [u8; 4])>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    let _sub = stream.subscribe(move |f: &VideoFrame| {
        let mid = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
        let px = [f.data[mid], f.data[mid + 1], f.data[mid + 2], f.data[mid + 3]];
        *sink.borrow_mut() = Some((f.width, f.height, px));
    });
    for _ in 0..100 {
        if got.borrow().is_some() {
            break;
        }
        sleep(30).await;
    }
    let (fw, fh, px) = got.borrow().expect("a frame arrived");
    assert_eq!((fw, fh), (w, h));
    // Video conversion can move a channel a little; the colour must hold.
    assert!(px[0] > 180 && px[1] < 80 && px[2] < 80, "the test image's red: {px:?}");
    invoke_command("call_function", &serde_json::json!({ "name": "camera.clear", "args": {} })).unwrap();
}
