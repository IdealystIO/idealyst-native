//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler reproducing the old web handler's DOM call-for-call: one
//!   `<video>` element with the static attributes
//!   (autoplay/muted/controls/loop + aspect-preserving `object-fit`),
//!   the reactive source resolved through a world effect that sets
//!   `src` (URL) / `srcObject` (live stream) / clears, author style via
//!   [`attach_style`], ref fill with the web ops (playback ops shared
//!   with the old leg via `web_util`). The handler installs NO DOM
//!   event listeners and invokes NO author callbacks, so the "External
//!   web glue must call `schedule_flush` after author callbacks"
//!   residual (named in backend-web's `newcore.rs` module docs) has no
//!   call site here — if a future port adds media event listeners that
//!   reach author code (`on_ended` etc.), those invocations MUST be
//!   followed by `backend_web::newcore::schedule_flush()`.
//! - **Everywhere else** — [`register`] installs the frozen
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]), mirroring the old walker's
//!   posture for a backend with no registered handler (author style +
//!   ref fill + `release_external` teardown all still flow, exactly
//!   like `walker/external.rs`). The native players (iOS/macOS
//!   AVPlayer, Android `VideoView`) are old-core-only until the
//!   new-core native boots grow an external story — the codeblock
//!   precedent. When they are ported, the native external glue must
//!   wrap author callbacks with the backend's `schedule_flush` — the
//!   residual named in each backend's `newcore.rs` module docs.
//!
//! Same authored call shape as the old-core leg:
//! `Video(VideoProps { .. }).with_style(…).bind(v)` then element
//! coercion — a consuming app compiles the same source against either
//! core. App bootstrap passes [`register`] as the boot registration
//! seam (there is no inventory self-registration on the new core; the
//! registry is built explicitly at boot).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::Ref;
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface — same shapes as the old core.
// ============================================================================

/// Author-supplied props for a `Video` instance. Same field shapes as
/// the old core; carried inside the scene item payload and read back by
/// the registered handler.
///
/// `source` is reactive: the handler resolves it inside a world effect
/// and re-populates the player whenever signals read by the source
/// change.
///
/// (No `IdealystSchema` derive on this leg — the MCP catalog documents
/// the old-core surface, per the codeblock/table precedent.)
pub struct VideoProps {
    /// What to display — one extensible prop for any media source. Build it
    /// with [`url`] (a fetched URL: file / HLS / DASH / live / `data:`),
    /// [`stream`] (a live [`MediaStream`](media_stream::MediaStream) from
    /// `camera` / `screen-recorder`), or your own [`VideoSource`]. The
    /// backend resolves it inside a reactive effect, so a source that
    /// reads signals re-populates the view on change.
    pub source: Box<dyn VideoSource>,
    /// Begin playback immediately on mount. On the **web** this implies a
    /// muted start regardless of [`muted`](Self::muted) — browsers block
    /// unmuted autoplay without a user gesture (the viewer un-mutes via the
    /// controls).
    pub autoplay: bool,
    /// Start with the audio track muted. Defaults to `false`. On the web,
    /// [`autoplay`](Self::autoplay) additionally forces a muted start.
    pub muted: bool,
    /// Show native playback controls (play/pause scrubber, volume,
    /// fullscreen) — browser `<video controls>` on the web leg.
    pub controls: bool,
    /// Restart from the beginning when playback reaches the end. Field
    /// name avoids the `loop` keyword. Ignored for a live stream source.
    pub loop_playback: bool,
    /// How the frame fills its box when their aspect ratios differ. Mirrors
    /// CSS `object-fit`: [`ObjectFit::Contain`] (default) fits the whole frame
    /// inside the box (letterboxed); [`ObjectFit::Cover`] fills the box,
    /// cropping the overflow — what a self-view camera widget wants.
    pub object_fit: ObjectFit,
}

impl Default for VideoProps {
    fn default() -> Self {
        Self {
            source: Box::new(NoSource),
            autoplay: false,
            muted: false,
            controls: false,
            loop_playback: false,
            object_fit: ObjectFit::Contain,
        }
    }
}

/// How a [`Video`] frame fills its box when their aspect ratios differ —
/// the cross-platform analogue of CSS `object-fit`. Same shape as the
/// old core.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ObjectFit {
    /// Fit the entire frame inside the box, preserving aspect (letterboxed).
    #[default]
    Contain,
    /// Fill the box, preserving aspect, cropping whatever overflows.
    Cover,
}

// ============================================================================
// Media source — one extensible abstraction for "what a Video displays".
// ============================================================================

/// What a [`VideoSource`] resolves to: the small, platform-agnostic set of
/// ways a video view can be populated. Backends match on this closed set;
/// source authors produce it. `#[non_exhaustive]` so a new mechanism can be
/// added without breaking implementors — old-core parity.
#[non_exhaustive]
pub enum MediaContent {
    /// Nothing to display.
    None,
    /// A URL the platform player fetches/decodes — file, HLS, DASH, live,
    /// or `data:`.
    Url(String),
    /// A live [`MediaStream`](media_stream::MediaStream) — camera, screen
    /// capture, or generated frames. Its `native_source` hides the
    /// per-platform pipe.
    Stream(media_stream::MediaStream),
}

/// The single, extensible media-source abstraction a [`Video`] displays.
///
/// Build the common cases with [`url`] / [`stream`]. Implement this directly
/// for a custom source — the backend calls [`resolve`](VideoSource::resolve)
/// inside a reactive effect, so any signal read there makes the view
/// re-populate on change.
pub trait VideoSource: 'static {
    /// Resolve the current content. Runs inside the backend's reactive
    /// effect — reads of signals here re-populate the video when they change.
    fn resolve(&self) -> MediaContent;
}

/// The default source: displays nothing until a real source is set.
struct NoSource;
impl VideoSource for NoSource {
    fn resolve(&self) -> MediaContent {
        MediaContent::None
    }
}

/// A URL source. Accepts `&str`, `String`, or `Fn() -> String` (reactive —
/// reads of captured signals re-load the player).
///
/// ```ignore
/// Video(VideoProps { source: url("https://…/clip.mp4"), ..Default::default() })
/// Video(VideoProps { source: url(move || sig.get()),    ..Default::default() })
/// ```
pub fn url<S: IntoUrl>(s: S) -> Box<dyn VideoSource> {
    Box::new(UrlSource(s.into_url()))
}

struct UrlSource(Box<dyn Fn() -> String>);
impl VideoSource for UrlSource {
    fn resolve(&self) -> MediaContent {
        MediaContent::Url((self.0)())
    }
}

/// Coercion target for [`url`] — `&str`, `String`, or any `Fn() -> String`.
pub trait IntoUrl {
    /// Box the receiver into the reactive URL closure a [`url`] source holds.
    fn into_url(self) -> Box<dyn Fn() -> String>;
}
impl IntoUrl for &str {
    fn into_url(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}
impl IntoUrl for String {
    fn into_url(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}
impl<F: Fn() -> String + 'static> IntoUrl for F {
    fn into_url(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}

/// A live-stream source. Accepts a [`MediaStream`](media_stream::MediaStream)
/// directly, or `Fn() -> Option<MediaStream>` (reactive — swap the stream, or
/// clear the view with `None`).
///
/// ```ignore
/// Video(VideoProps { source: stream(camera_stream),     ..Default::default() })
/// Video(VideoProps { source: stream(move || sig.get()), ..Default::default() })
/// ```
pub fn stream<S: IntoStream>(s: S) -> Box<dyn VideoSource> {
    s.into_stream()
}

/// Coercion target for [`stream`] — a `MediaStream` or `Fn() -> Option<MediaStream>`.
pub trait IntoStream {
    /// Box the receiver into a [`VideoSource`].
    fn into_stream(self) -> Box<dyn VideoSource>;
}
impl IntoStream for media_stream::MediaStream {
    fn into_stream(self) -> Box<dyn VideoSource> {
        Box::new(StaticStream(self))
    }
}
// One `Fn` blanket per `Into*` trait — no two `Fn`-output blankets share a
// trait, so this dodges the closure-coherence conflict (old-core parity).
impl<F: Fn() -> Option<media_stream::MediaStream> + 'static> IntoStream for F {
    fn into_stream(self) -> Box<dyn VideoSource> {
        Box::new(ReactiveStream(Box::new(self)))
    }
}

struct StaticStream(media_stream::MediaStream);
impl VideoSource for StaticStream {
    fn resolve(&self) -> MediaContent {
        MediaContent::Stream(self.0.clone())
    }
}
struct ReactiveStream(Box<dyn Fn() -> Option<media_stream::MediaStream>>);
impl VideoSource for ReactiveStream {
    fn resolve(&self) -> MediaContent {
        match (self.0)() {
            Some(s) => MediaContent::Stream(s),
            None => MediaContent::None,
        }
    }
}

// ============================================================================
// Handle + ops trait — same shapes as the old core.
// ============================================================================

/// Typed handle to a mounted `Video`. Filled at mount time when the
/// author chained [`VideoBind::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct VideoHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn VideoOps,
}

impl VideoHandle {
    /// Wrap a type-erased native node + backend ops into a handle.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn VideoOps) -> Self {
        Self { node, ops }
    }

    /// Start (or resume) playback.
    pub fn play(&self) {
        self.ops.play(&*self.node);
    }

    /// Pause playback, leaving the current position intact.
    pub fn pause(&self) {
        self.ops.pause(&*self.node);
    }

    /// Seek to the given offset in seconds.
    pub fn seek(&self, seconds: f32) {
        self.ops.seek(&*self.node, seconds);
    }

    /// Mute or unmute the audio track without touching playback
    /// position — flips muting on a *live* player, unlike the
    /// [`muted`](VideoProps::muted) prop (fixed at construction).
    pub fn set_muted(&self, muted: bool) {
        self.ops.set_muted(&*self.node, muted);
    }

    /// Current playback position in seconds, or `0.0` if unknown / not yet
    /// loaded. Poll it (e.g. from a `raf_loop`) to drive a progress scrubber.
    pub fn position(&self) -> f32 {
        self.ops.position(&*self.node)
    }

    /// Total duration of the loaded media in seconds, or `0.0` if unknown
    /// (still loading, a live stream, or an empty player).
    pub fn duration(&self) -> f32 {
        self.ops.duration(&*self.node)
    }
}

/// Imperative-ops dispatch. Same trait shape as the old core; the
/// active target's `OPS` static supplies the impl.
pub trait VideoOps: Sync {
    /// Start (or resume) playback. Default no-op.
    fn play(&self, _node: &dyn Any) {}
    /// Pause playback. Default no-op.
    fn pause(&self, _node: &dyn Any) {}
    /// Seek to the given offset in seconds. Default no-op.
    fn seek(&self, _node: &dyn Any, _seconds: f32) {}
    /// Mute/unmute the audio track. Default no-op.
    fn set_muted(&self, _node: &dyn Any, _muted: bool) {}
    /// Current playback position in seconds. Default `0.0`.
    fn position(&self, _node: &dyn Any) -> f32 {
        0.0
    }
    /// Total media duration in seconds. Default `0.0` (unknown).
    fn duration(&self, _node: &dyn Any) -> f32 {
        0.0
    }
}

/// Fallback ops used on targets with no `Video` impl (every non-web
/// target on the new core — the placeholder posture).
pub struct UnsupportedOps;
impl VideoOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn VideoOps = web_glue::OPS;
#[cfg(not(target_arch = "wasm32"))]
static OPS: &dyn VideoOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the old `Bound<VideoHandle>` call shape
// (`.with_style(…)` / `.bind(…)` then element coercion).
// ============================================================================

/// Scene payload for the `Video` item. Single-take slots (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the style/ref-fill must move at
/// mount.
struct VideoPrim {
    props: Rc<VideoProps>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`Video`] — mirrors the old-core
/// `Bound<VideoHandle>` call shape.
pub struct VideoBound {
    props: Rc<VideoProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `Video` primitive — the new-core stand-in for the old-core
/// `Video(props)`, same call shape at every author site.
#[allow(non_snake_case)]
pub fn Video(props: VideoProps) -> VideoBound {
    VideoBound {
        props: Rc::new(props),
        style: None,
        ref_fill: None,
    }
}

impl VideoBound {
    /// Attach the author style — lands on the outer node (the `<video>`
    /// element on web), exactly like `.with_style` on the old-core
    /// `Bound`.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` — same trait name/shape as the old core so author
/// imports (`use video::prelude::*`) compile unchanged.
pub trait VideoBind {
    /// Bind a `Ref<VideoHandle>` for imperative access. At mount time
    /// the handler wraps the native node in a `VideoHandle` using the
    /// active target's ops and fills the ref.
    fn bind(self, r: Ref<VideoHandle>) -> Self;
}

impl VideoBind for VideoBound {
    fn bind(mut self, r: Ref<VideoHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(VideoHandle::new(node_any, OPS));
        }));
        self
    }
}

impl IntoElement for VideoBound {
    fn into_element(self) -> Element {
        item(
            VideoPrim {
                props: self.props,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            Vec::new(),
        )
    }
}

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<VideoBound> for Element {
    fn from(b: VideoBound) -> Element {
        b.into_element()
    }
}

/// One-stop import: same names as the old-core prelude.
pub mod prelude {
    pub use super::{
        stream, url, MediaContent, Video, VideoBind, VideoHandle, VideoProps, VideoSource,
    };
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail — the old walker's `external.rs` sequence after
/// node creation: (children are none for video) → author style →
/// ref fill (type-erased node clone) → scope-tied `release_external`.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &VideoPrim)
where
    H: ExternalOps + StyleServices,
{
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(backend, node, style);
    }
    if let Some(fill) = prim.ref_fill.borrow_mut().take() {
        let any_node: Rc<dyn Any> = Rc::new(node.clone());
        fill(any_node);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}

/// Placeholder handler for hosts with no real video player — the frozen
/// External degradation path (`create_external` with no registered
/// old-core handler renders each backend's "not supported" box; SSR
/// renders a bare `<div>`).
#[cfg(not(target_arch = "wasm32"))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<VideoPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<VideoProps>(),
        std::any::type_name::<VideoProps>(),
        &payload,
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the video payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<VideoPrim, _>(mount_placeholder::<H>);
}

/// Register the video payload handler on the web backend's scene
/// registry — the real `<video>` renderer.
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<VideoPrim, _>(web_glue::mount_video_web);
}

// ============================================================================
// Web glue (wasm32): the old web handler, call-for-call, over the
// scene contract.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web_glue {
    use super::*;
    use crate::web_util;
    use backend_web::WebBackend;

    pub(super) static OPS: &dyn VideoOps = &WebVideoOps;

    struct WebVideoOps;
    // Shared with the old-core leg (web_util) so the two cores' playback
    // ops can't drift.
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

    pub(super) fn mount_video_web(
        cx: &mut MountCx<'_, WebBackend>,
        prim: &Rc<VideoPrim>,
        _children: Vec<Element>,
    ) -> web_sys::Node {
        let backend = cx.backend().clone();
        let fit = match prim.props.object_fit {
            ObjectFit::Contain => "contain",
            ObjectFit::Cover => "cover",
        };
        // Static attribute setup (autoplay/muted/controls/loop +
        // object-fit) — the old-core web handler, verbatim (web_util).
        let video = web_util::create_video_element(
            prim.props.autoplay,
            prim.props.muted,
            prim.props.controls,
            prim.props.loop_playback,
            fit,
        );

        // One reactive populate effect: resolve the source each run, then
        // set `src` (URL) or `srcObject` (stream) / clear. World effect:
        // created during realize (world entered), so it is collected into
        // the enclosing subtree and dies at unmount; runs once immediately
        // and re-fires on signal reads inside `resolve()` — the old
        // `effect!` contract. The body only pokes the browser (no author
        // callbacks, no signal writes), so no `schedule_flush` is needed
        // here — see the module docs.
        let video_for_effect = video.clone();
        let props_for_effect = prim.props.clone();
        runtime_world::effect(move || {
            match props_for_effect.source.resolve() {
                MediaContent::Url(u) => web_util::apply_url(&video_for_effect, &u),
                MediaContent::Stream(s) => {
                    web_util::apply_stream(&video_for_effect, &s, props_for_effect.autoplay)
                }
                MediaContent::None => web_util::apply_none(&video_for_effect),
            }
        });

        let node: web_sys::Node = video.into();
        finish_mount(&backend, &node, prim);
        node
    }
}
