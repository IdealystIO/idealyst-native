//! `Primitive::Video` — a `<video>` element with optional autoplay,
//! controls, and loop attributes.

use crate::WebBackend;
use runtime_core::primitives::video::{VideoHandle, VideoOps};
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) fn create(
    b: &mut WebBackend,
    src: &str,
    autoplay: bool,
    controls: bool,
    loop_playback: bool,
) -> Node {
    let video = b
        .doc
        .create_element("video")
        .expect("create_element video failed");
    let _ = video.set_attribute("src", src);
    if autoplay {
        let _ = video.set_attribute("autoplay", "");
        // Most browsers require `muted` for autoplay to work without
        // user gesture; matches RN's autoplay-friendly default.
        let _ = video.set_attribute("muted", "");
    }
    if controls {
        let _ = video.set_attribute("controls", "");
    }
    if loop_playback {
        let _ = video.set_attribute("loop", "");
    }
    video.unchecked_into::<Node>()
}

pub(crate) fn update_src(node: &Node, src: &str) {
    if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
        let _ = el.set_attribute("src", src);
    }
}

/// `HtmlMediaElement` exposes play/pause/currentTime, so we downcast
/// to that. Both `<video>` and `<audio>` are HtmlMediaElement
/// subclasses.
pub(crate) fn make_handle(node: &Node) -> VideoHandle {
    let el: web_sys::HtmlMediaElement = node
        .clone()
        .dyn_into()
        .expect("video node is not an HtmlMediaElement");
    VideoHandle::new(Rc::new(el), &WebVideoOps)
}

struct WebVideoOps;
impl VideoOps for WebVideoOps {
    fn play(&self, node: &dyn Any) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            // play() returns a Promise; we ignore it. Browsers may
            // reject if autoplay rules block playback — caller can
            // catch via JS if they care, not worth surfacing here.
            let _ = v.play();
        }
    }
    fn pause(&self, node: &dyn Any) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            let _ = v.pause();
        }
    }
    fn seek(&self, node: &dyn Any, seconds: f32) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            v.set_current_time(seconds as f64);
        }
    }
}
