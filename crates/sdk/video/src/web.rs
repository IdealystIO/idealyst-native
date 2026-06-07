//! Web (`target_arch = "wasm32"`) implementation of the Video SDK.
//!
//! Builds a `<video>` element per mount. Reactive src changes flow
//! through `Effect::new(...)` inside the handler (the framework runs
//! us inside the walker's active scope, so the effect is owned by the
//! scope and survives past handler return).

use crate::{MediaContent, VideoOps, VideoProps};
use backend_web::WebBackend;
use runtime_core::effect;
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

// Self-register at backend construction (no app-side `register` call needed).
// Survives the release `wasm-opt -Oz` pass (code fn-pointer, not prunable
// data). See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebExternalRegistrar(register)
}

fn build_video(props: &Rc<VideoProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let video = document
        .create_element("video")
        .expect("create_element(video) failed");

    if props.autoplay {
        let _ = video.set_attribute("autoplay", "");
    }
    // Mute when asked, OR whenever autoplaying — browsers block UNMUTED autoplay
    // without a user gesture, so an autoplaying clip must start silent (the
    // viewer un-mutes via the controls). `muted` only reliably takes via the
    // PROPERTY (the attribute alone is ignored by the autoplay gate in some
    // browsers), so set it on the media element too.
    if props.muted || props.autoplay {
        let _ = video.set_attribute("muted", "");
        if let Some(media) = video.dyn_ref::<web_sys::HtmlMediaElement>() {
            media.set_muted(true);
        }
    }
    if props.controls {
        let _ = video.set_attribute("controls", "");
    }
    if props.loop_playback {
        let _ = video.set_attribute("loop", "");
    }
    let _ = video.set_attribute("data-external-kind", "video::VideoProps");

    // object-fit: contain (letterbox) vs cover (fill + crop). Set the single
    // CSS property so the framework's width/height style on the external node
    // isn't clobbered. `<video>` defaults to `fill` (stretch), which we never
    // want — always pin one of the aspect-preserving modes.
    let fit = match props.object_fit {
        crate::ObjectFit::Contain => "contain",
        crate::ObjectFit::Cover => "cover",
    };
    if let Some(html) = video.dyn_ref::<web_sys::HtmlElement>() {
        let _ = html.style().set_property("object-fit", fit);
    }

    // One reactive populate effect: resolve the source each run, then set
    // `src` (URL) or `srcObject` (stream) / clear. The walker calls us inside
    // its active scope, so the Effect's slot is owned by that scope. Because
    // `resolve()` runs HERE, any signal it reads re-runs this and re-populates
    // — one mechanism for URL change, stream change, or swap-to-none.
    let video_for_effect = video.clone();
    let props_clone = props.clone();
    effect!({
        let video_el = video_for_effect.dyn_ref::<web_sys::HtmlVideoElement>();
        match props_clone.source.resolve() {
            MediaContent::Url(u) => {
                if let Some(v) = video_el {
                    v.set_src_object(None);
                }
                let _ = video_for_effect.set_attribute("src", &u);
            }
            // Zero-copy web path: attach the stream's native `web_sys::MediaStream`
            // (camera/screen-recorder publish theirs) as `srcObject` — the browser
            // renders the live feed with no per-frame copy. A stream with only a CPU
            // frame channel (no native source) would need the GPU/blit path — the
            // compositing layer's job, not wired here.
            MediaContent::Stream(s) => {
                let _ = video_for_effect.remove_attribute("src");
                if let (Some(v), Some(native)) = (video_el, s.native_source()) {
                    if let Some(media_stream) = native.downcast_ref::<web_sys::MediaStream>() {
                        v.set_src_object(Some(media_stream));
                        let _ = v.set_attribute("playsinline", "");
                        if props_clone.autoplay {
                            let _ = v.play();
                        }
                    }
                }
            }
            MediaContent::None => {
                if let Some(v) = video_el {
                    v.set_src_object(None);
                }
                let _ = video_for_effect.remove_attribute("src");
            }
        }
    });

    video
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
