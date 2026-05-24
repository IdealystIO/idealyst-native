//! Web (`target_arch = "wasm32"`) implementation of the Video SDK.
//!
//! Builds a `<video>` element per mount. Reactive src changes flow
//! through `Effect::new(...)` inside the handler (the framework runs
//! us inside the walker's active scope, so the effect is owned by the
//! scope and survives past handler return).

use crate::{VideoOps, VideoProps};
use backend_web::WebBackend;
use runtime_core::Effect;
use std::any::Any;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::Node;

pub(crate) static OPS: &dyn VideoOps = &WebVideoOps;

/// Register the Video handler against a `WebBackend`. One-line call
/// from the app's bootstrap.
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<VideoProps, _>(|props, _backend| build_video(props));
}

fn build_video(props: &Rc<VideoProps>) -> Node {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let video = document
        .create_element("video")
        .expect("create_element(video) failed");

    if props.autoplay {
        let _ = video.set_attribute("autoplay", "");
        // Most browsers block unmuted autoplay without a user gesture;
        // pairing the two matches the cross-platform "autoplay = silent
        // autoplay" expectation that the iOS/Android impls also use.
        let _ = video.set_attribute("muted", "");
    }
    if props.controls {
        let _ = video.set_attribute("controls", "");
    }
    if props.loop_playback {
        let _ = video.set_attribute("loop", "");
    }
    let _ = video.set_attribute("data-external-kind", "video::VideoProps");

    // Reactive src. The walker calls us inside its active scope, so the
    // Effect's slot is owned by that scope — `_effect` going out of this
    // function is fine, the scope keeps it alive.
    let video_for_src = video.clone();
    let props_clone = props.clone();
    let _effect = Effect::new(move || {
        let url = (props_clone.src)();
        let _ = video_for_src.set_attribute("src", &url);
    });

    video.unchecked_into::<Node>()
}

// ============================================================================
// Imperative ops
// ============================================================================

struct WebVideoOps;

impl VideoOps for WebVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(el) = downcast_media(node) else { return };
        // play() returns a Promise; we ignore it. Browsers may reject if
        // autoplay rules block playback — caller can catch via JS if
        // they care, not worth surfacing here.
        let _ = el.play();
    }

    fn pause(&self, node: &dyn Any) {
        let Some(el) = downcast_media(node) else { return };
        let _ = el.pause();
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(el) = downcast_media(node) else { return };
        el.set_current_time(seconds as f64);
    }
}

/// The framework hands us a `Rc<dyn Any>` whose concrete type is
/// `web_sys::Node` (what the registry handler returned). Both `<video>`
/// and `<audio>` are `HtmlMediaElement` subclasses, so we downcast the
/// node to that for the playback ops.
fn downcast_media(node: &dyn Any) -> Option<web_sys::HtmlMediaElement> {
    node.downcast_ref::<Node>()
        .and_then(|n| n.clone().dyn_into::<web_sys::HtmlMediaElement>().ok())
}
