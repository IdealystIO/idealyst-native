//! Pure wasm32 DOM helpers for the web leg: `<video>` element
//! construction, media population (URL / live-stream / clear), and the
//! imperative playback ops over the type-erased node. Kept as its own
//! module so this half stays framework-free (web-glue handles +
//! `media_stream`, no scene/world types).
//!
//! `object_fit_css` and the `MediaStream` argument are already-lowered
//! values: the caller matches its `ObjectFit` / `MediaContent` before
//! calling, so nothing here needs the SDK's own enums.

use std::any::Any;

use web_glue::dom::{Element, Node};
use web_glue::{JsCast, JsValue};

web_glue::import! {
    // `play()`. Its Promise rejects under the autoplay policy (no user
    // gesture) or when a newer `src` interrupts it; nothing here waits on
    // it, so the rejection is observed in the binding instead of surfacing
    // as an unhandled rejection.
    fn js_play(v: u32) = "(v) => { const p = G.get(v).play(); if (p) p.catch(() => {}); }";
    fn js_pause(v: u32) = "(v) => { G.get(v).pause(); }";
    fn js_set_current_time(v: u32, t: f64) = "(v, t) => { G.get(v).currentTime = t; }";
    fn js_current_time(v: u32) -> f64 = "(v) => G.get(v).currentTime";
    fn js_duration(v: u32) -> f64 = "(v) => G.get(v).duration";
    fn js_set_muted(v: u32, m: u32) = "(v, m) => { G.get(v).muted = m !== 0; }";
    // `srcObject = s` (handle 1 is `null`, which clears it).
    fn js_set_src_object(v: u32, s: u32) = "(v, s) => { G.get(v).srcObject = G.get(s); }";
    // 1 when `v` is an `HTMLMediaElement` (`<video>` / `<audio>`).
    fn js_is_media(v: u32) -> u32 = "(v) => G.get(v) instanceof HTMLMediaElement ? 1 : 0";
}

fn raw(el: &Element) -> u32 {
    el.as_js().raw()
}

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
) -> Element {
    let document = web_glue::dom::window()
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
        // SAFETY: a live `<video>` handle.
        unsafe { js_set_muted(raw(&video), 1) };
    }
    if controls {
        let _ = video.set_attribute("controls", "");
    }
    if loop_playback {
        let _ = video.set_attribute("loop", "");
    }
    // Stable literal — introspection/devtools key on it.
    let _ = video.set_attribute("data-external-kind", "video::VideoProps");

    // object-fit: contain (letterbox) vs cover (fill + crop). Set the single
    // CSS property so the framework's width/height style on the external node
    // isn't clobbered. `<video>` defaults to `fill` (stretch), which we never
    // want — always pin one of the aspect-preserving modes.
    if let Some(html) = video.dyn_ref::<web_glue::dom::HtmlElement>() {
        let _ = html.style().set_property("object-fit", object_fit_css);
    }

    video
}

/// Populate the element from a URL source: clear any live `srcObject`,
/// then point `src` at the clip.
pub(crate) fn apply_url(video: &Element, url: &str) {
    // SAFETY: a live `<video>` handle; slot 1 is `null`.
    unsafe { js_set_src_object(raw(video), JsValue::NULL.raw()) };
    let _ = video.set_attribute("src", url);
}

/// Populate the element from a live stream source.
///
/// Zero-copy web path: attach the stream's native `MediaStream`
/// (camera/screen-recorder publish theirs) as `srcObject` — the browser
/// renders the live feed with no per-frame copy. A stream with only a CPU
/// frame channel (no native source) would need the GPU/blit path — the
/// compositing layer's job, not wired here.
pub(crate) fn apply_stream(
    video: &Element,
    stream: &media_stream::MediaStream,
    autoplay: bool,
) {
    let _ = video.remove_attribute("src");
    let Some(native) = stream.native_source() else { return };
    // HYBRID-BRIDGE: native_source MediaStream — switches with the media
    // SDKs. The producers (camera, screen-recorder, canvas self-capture,
    // video-compose) still publish a `web_sys::MediaStream`; it crosses
    // into the glue slab here (one JS call, same object).
    let Some(media_stream) = native.downcast_ref::<web_sys::MediaStream>() else { return };
    let media_stream: JsValue = web_glue::bridge::from_bindgen(media_stream.as_ref());
    // SAFETY: live handles.
    unsafe { js_set_src_object(raw(video), media_stream.raw()) };
    let _ = video.set_attribute("playsinline", "");
    if autoplay {
        // SAFETY: a live `<video>` handle.
        unsafe { js_play(raw(video)) };
    }
}

/// Clear the element — no URL, no stream.
pub(crate) fn apply_none(video: &Element) {
    // SAFETY: a live `<video>` handle; slot 1 is `null`.
    unsafe { js_set_src_object(raw(video), JsValue::NULL.raw()) };
    let _ = video.remove_attribute("src");
}

// ============================================================================
// Imperative ops over the type-erased node — the whole body of the web
// `VideoOps` impl.
// ============================================================================

/// The framework hands us a `Rc<dyn Any>` whose concrete type is the
/// backend's node (`web_glue::dom::Node`, what the registry handler
/// returned). Both `<video>` and `<audio>` are `HTMLMediaElement`
/// subclasses; anything else is refused (the ops then no-op).
pub(crate) fn downcast_media(node: &dyn Any) -> Option<&Element> {
    let el = node.downcast_ref::<Node>()?.dyn_ref::<Element>()?;
    // SAFETY: a live element handle.
    (unsafe { js_is_media(raw(el)) } != 0).then_some(el)
}

/// Start (or resume) playback on the mounted media element.
pub(crate) fn play(node: &dyn Any) {
    let Some(el) = downcast_media(node) else { return };
    // Browsers may reject if autoplay rules block playback; the binding
    // observes the rejection (not worth surfacing here).
    // SAFETY: a live media element handle.
    unsafe { js_play(raw(el)) };
}

/// Pause playback, leaving the current position intact.
pub(crate) fn pause(node: &dyn Any) {
    let Some(el) = downcast_media(node) else { return };
    // SAFETY: a live media element handle.
    unsafe { js_pause(raw(el)) };
}

/// Seek to the given offset in seconds.
pub(crate) fn seek(node: &dyn Any, seconds: f32) {
    let Some(el) = downcast_media(node) else { return };
    // SAFETY: a live media element handle.
    unsafe { js_set_current_time(raw(el), seconds as f64) };
}

/// Mute/unmute the live audio track.
pub(crate) fn set_muted(node: &dyn Any, muted: bool) {
    let Some(el) = downcast_media(node) else { return };
    // SAFETY: a live media element handle.
    unsafe { js_set_muted(raw(el), muted as u32) };
}

/// Current playback position in seconds, `0.0` when unknown.
pub(crate) fn position(node: &dyn Any) -> f32 {
    let Some(el) = downcast_media(node) else { return 0.0 };
    // SAFETY: a live media element handle.
    unsafe { js_current_time(raw(el)) as f32 }
}

/// Total media duration in seconds, `0.0` when unknown.
pub(crate) fn duration(node: &dyn Any) -> f32 {
    let Some(el) = downcast_media(node) else { return 0.0 };
    // `duration` is NaN before metadata loads and Infinity for a live
    // stream; both are useless as a scrubber denominator → report 0.0.
    // SAFETY: a live media element handle.
    let d = unsafe { js_duration(raw(el)) };
    if d.is_finite() {
        d as f32
    } else {
        0.0
    }
}
