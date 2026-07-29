//! Pure wasm32 DOM helpers shared by BOTH cores' web legs (no core
//! types, no feature gates): `<video>` element construction, media
//! population (URL / live-stream / clear), and the imperative playback
//! ops over the type-erased node. Extracted from the old-core `web.rs`
//! when the `new-core` leg landed so the two legs can't drift.
//!
//! `media_stream::MediaStream` appears here deliberately — the
//! media-stream crate is core-agnostic data (no runtime-core dep), so
//! it is shareable exactly like web-sys. The per-core `MediaContent`
//! enums stay in each leg; both match into these free fns.

use std::any::Any;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Build the `<video>` element with the static (construction-time)
/// props applied: autoplay/muted/controls/loop attributes, the
/// `data-external-kind` marker, and the aspect-preserving `object-fit`.
///
/// `object_fit_css` is the already-lowered CSS keyword (`"contain"` /
/// `"cover"`) — each leg matches its own `ObjectFit` enum before
/// calling.
pub(crate) fn create_video_element(
    autoplay: bool,
    muted: bool,
    controls: bool,
    loop_playback: bool,
    object_fit_css: &str,
) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let video = document
        .create_element("video")
        .expect("create_element(video) failed");

    if autoplay {
        let _ = video.set_attribute("autoplay", "");
    }
    // Mute when asked, OR whenever autoplaying — browsers block UNMUTED autoplay
    // without a user gesture, so an autoplaying clip must start silent (the
    // viewer un-mutes via the controls). `muted` only reliably takes via the
    // PROPERTY (the attribute alone is ignored by the autoplay gate in some
    // browsers), so set it on the media element too.
    if muted || autoplay {
        let _ = video.set_attribute("muted", "");
        if let Some(media) = video.dyn_ref::<web_sys::HtmlMediaElement>() {
            media.set_muted(true);
        }
    }
    if controls {
        let _ = video.set_attribute("controls", "");
    }
    if loop_playback {
        let _ = video.set_attribute("loop", "");
    }
    // Same literal on both cores — introspection/devtools key on it.
    let _ = video.set_attribute("data-external-kind", "video::VideoProps");

    // object-fit: contain (letterbox) vs cover (fill + crop). Set the single
    // CSS property so the framework's width/height style on the external node
    // isn't clobbered. `<video>` defaults to `fill` (stretch), which we never
    // want — always pin one of the aspect-preserving modes.
    if let Some(html) = video.dyn_ref::<web_sys::HtmlElement>() {
        let _ = html.style().set_property("object-fit", object_fit_css);
    }

    video
}

/// Populate the element from a URL source: clear any live `srcObject`,
/// then point `src` at the clip.
pub(crate) fn apply_url(video: &web_sys::Element, url: &str) {
    if let Some(v) = video.dyn_ref::<web_sys::HtmlVideoElement>() {
        v.set_src_object(None);
    }
    let _ = video.set_attribute("src", url);
}

/// Populate the element from a live stream source.
///
/// Zero-copy web path: attach the stream's native `web_sys::MediaStream`
/// (camera/screen-recorder publish theirs) as `srcObject` — the browser
/// renders the live feed with no per-frame copy. A stream with only a CPU
/// frame channel (no native source) would need the GPU/blit path — the
/// compositing layer's job, not wired here.
pub(crate) fn apply_stream(
    video: &web_sys::Element,
    stream: &media_stream::MediaStream,
    autoplay: bool,
) {
    let _ = video.remove_attribute("src");
    if let (Some(v), Some(native)) = (
        video.dyn_ref::<web_sys::HtmlVideoElement>(),
        stream.native_source(),
    ) {
        if let Some(media_stream) = native.downcast_ref::<web_sys::MediaStream>() {
            v.set_src_object(Some(media_stream));
            let _ = v.set_attribute("playsinline", "");
            if autoplay {
                let _ = v.play();
            }
        }
    }
}

/// Clear the element — no URL, no stream.
pub(crate) fn apply_none(video: &web_sys::Element) {
    if let Some(v) = video.dyn_ref::<web_sys::HtmlVideoElement>() {
        v.set_src_object(None);
    }
    let _ = video.remove_attribute("src");
}

// ============================================================================
// Imperative ops over the type-erased node — the whole body of the web
// `VideoOps` impl, shared by both cores' ops structs.
// ============================================================================

/// The framework hands us a `Rc<dyn Any>` whose concrete type is
/// `web_sys::Node` (what the registry handler returned). Both `<video>`
/// and `<audio>` are `HtmlMediaElement` subclasses, so we downcast the
/// node to that for the playback ops.
pub(crate) fn downcast_media(node: &dyn Any) -> Option<web_sys::HtmlMediaElement> {
    node.downcast_ref::<Node>()
        .and_then(|n| n.clone().dyn_into::<web_sys::HtmlMediaElement>().ok())
}

/// Start (or resume) playback on the mounted media element.
pub(crate) fn play(node: &dyn Any) {
    let Some(el) = downcast_media(node) else { return };
    // play() returns a Promise; we ignore it. Browsers may reject if
    // autoplay rules block playback — caller can catch via JS if
    // they care, not worth surfacing here.
    let _ = el.play();
}

/// Pause playback, leaving the current position intact.
pub(crate) fn pause(node: &dyn Any) {
    let Some(el) = downcast_media(node) else { return };
    let _ = el.pause();
}

/// Seek to the given offset in seconds.
pub(crate) fn seek(node: &dyn Any, seconds: f32) {
    let Some(el) = downcast_media(node) else { return };
    el.set_current_time(seconds as f64);
}

/// Mute/unmute the live audio track.
pub(crate) fn set_muted(node: &dyn Any, muted: bool) {
    let Some(el) = downcast_media(node) else { return };
    el.set_muted(muted);
}

/// Current playback position in seconds, `0.0` when unknown.
pub(crate) fn position(node: &dyn Any) -> f32 {
    let Some(el) = downcast_media(node) else { return 0.0 };
    el.current_time() as f32
}

/// Total media duration in seconds, `0.0` when unknown.
pub(crate) fn duration(node: &dyn Any) -> f32 {
    let Some(el) = downcast_media(node) else { return 0.0 };
    // `duration` is NaN before metadata loads and Infinity for a live
    // stream; both are useless as a scrubber denominator → report 0.0.
    let d = el.duration();
    if d.is_finite() {
        d as f32
    } else {
        0.0
    }
}
