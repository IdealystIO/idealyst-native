//! Web (`target_arch = "wasm32"`) implementation of the Video SDK —
//! old-core leg.
//!
//! Builds a `<video>` element per mount. Reactive src changes flow
//! through an `effect!` inside the handler (the framework runs
//! us inside the walker's active scope, so the effect is owned by the
//! scope and survives past handler return).
//!
//! The DOM mechanics (element construction, media population, playback
//! ops) live in `web_util` — shared with the new-core web leg so the
//! two cores can't drift.

use crate::{web_util, MediaContent, VideoOps, VideoProps};
use backend_web::WebBackend;
use runtime_core::effect;
use std::any::Any;
use std::rc::Rc;

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
    let fit = match props.object_fit {
        crate::ObjectFit::Contain => "contain",
        crate::ObjectFit::Cover => "cover",
    };
    let video = web_util::create_video_element(
        props.autoplay,
        props.muted,
        props.controls,
        props.loop_playback,
        fit,
    );

    // One reactive populate effect: resolve the source each run, then set
    // `src` (URL) or `srcObject` (stream) / clear. The walker calls us inside
    // its active scope, so the Effect's slot is owned by that scope. Because
    // `resolve()` runs HERE, any signal it reads re-runs this and re-populates
    // — one mechanism for URL change, stream change, or swap-to-none.
    let video_for_effect = video.clone();
    let props_clone = props.clone();
    effect!({
        match props_clone.source.resolve() {
            MediaContent::Url(u) => web_util::apply_url(&video_for_effect, &u),
            MediaContent::Stream(s) => {
                web_util::apply_stream(&video_for_effect, &s, props_clone.autoplay)
            }
            MediaContent::None => web_util::apply_none(&video_for_effect),
        }
    });

    video
}

// ============================================================================
// Imperative ops — thin dispatch onto the shared `web_util` free fns.
// ============================================================================

struct WebVideoOps;

impl VideoOps for WebVideoOps {
    fn play(&self, node: &dyn Any) {
        web_util::play(node);
    }

    fn pause(&self, node: &dyn Any) {
        web_util::pause(node);
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        web_util::seek(node, seconds);
    }

    fn set_muted(&self, node: &dyn Any, muted: bool) {
        web_util::set_muted(node, muted);
    }

    fn position(&self, node: &dyn Any) -> f32 {
        web_util::position(node)
    }

    fn duration(&self, node: &dyn Any) -> f32 {
        web_util::duration(node)
    }
}
