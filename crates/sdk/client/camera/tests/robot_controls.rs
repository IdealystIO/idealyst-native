//! A robot test feeds the app's camera from an image (`camera.fake`). A
//! robot build: `cargo test -p camera --features robot`.

#![cfg(feature = "robot")]

use std::future::Future;
use std::task::{Context, Poll, Waker};

use base64::Engine as _;
use runtime_vocabulary::robot::bridge::invoke_command;
use serde_json::json;

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("a faked camera resolves at once; this one would open a device"),
    }
}

fn call(name: &str, args: serde_json::Value) -> Result<String, String> {
    invoke_command("call_function", &json!({ "name": name, "args": args }))
}

/// want_18fc6ca7 (CrewForge): the kiosk's photo step and face match need a
/// camera a test can point at a known face. With a fake set, opening the
/// camera streams the test image (no device, no prompt), `devices` lists
/// the test camera, and the stream's frames are the image byte for byte.
#[test]
fn regression_a_faked_camera_streams_the_test_image() {
    camera::register_robot_controls();
    // A 2×2 image: red, green, blue, white.
    let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
    let b64 = base64::engine::general_purpose::STANDARD.encode(&rgba);
    call("camera.fake", json!({ "width": 2, "height": 2, "rgba": b64 })).unwrap();

    let cam = camera::Camera::new();
    block_on(cam.request_permission()).expect("no prompt");
    let devices = block_on(cam.devices()).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].label, "Robot test camera");

    let stream = block_on(cam.open(camera::CameraConfig::default())).expect("the fake opens");
    let mut buf = Vec::new();
    assert_eq!(stream.latest(&mut buf), Some((2, 2)));
    assert_eq!(buf, rgba, "the frame is the test image");
    let status = call("camera.status", json!({})).unwrap();
    assert!(status.contains("\"fake\":true") && status.contains("\"opened\":1"), "{status}");
    drop(stream);

    let err = call("camera.fake", json!({ "width": 3, "height": 3, "rgba": b64 })).unwrap_err();
    assert!(err.contains("36 bytes"), "{err}");
    call("camera.clear", json!({})).unwrap();
    assert!(call("camera.status", json!({})).unwrap().contains("\"fake\":false"));
}
