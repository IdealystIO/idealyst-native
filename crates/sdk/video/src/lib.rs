//! Third-party `Video` SDK for the idealyst framework.
//!
//! Provides a `Video` primitive backed by the framework's
//! `Element::External` extension mechanism. Mirrors the framework's
//! other reactive primitives — typed props, `.bind(...)`-able handle,
//! `.with_style(...)`.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! video::register(&mut backend);
//!
//! // Inside a `ui!` block:
//! let src = signal("https://example.com/clip.mp4".to_string());
//! let v: Ref<VideoHandle> = Ref::new();
//! ui! {
//!     View {
//!         { video::Video(VideoProps {
//!             src: video::src(move || src.get()),
//!             autoplay: true,
//!             controls: true,
//!             ..Default::default()
//!         }).bind(v.clone()) }
//!     }
//! }
//! // Imperative ops at any later point:
//! v.with(|h| h.play());
//! v.with(|h| h.seek(10.0));
//! ```
//!
//! # Architecture
//!
//! - The `Element::External` payload type is [`VideoProps`] — all
//!   props (src + autoplay/controls/loop) are owned by the SDK, not the
//!   framework.
//! - Per-backend `register(&mut backend)` impls live in cfg-gated
//!   `web` / `android` / `ios` modules below. Each one calls
//!   `backend.register_external::<VideoProps, _>(handler)` to install a
//!   builder closure keyed by `TypeId::of::<VideoProps>()`.
//! - `VideoHandle` is the typed ref-target. It carries a type-erased
//!   `Rc<dyn Any>` to the native node + a `&'static dyn VideoOps`
//!   pointer that the active backend module exposes as a static.
//! - Reactive `src` flows through `Effect::new(...)` *inside* the
//!   backend handler closure — the per-backend impl subscribes itself
//!   when it builds the native view. No framework-level
//!   `update_video_src` plumbing involved.
#![deny(missing_docs)]

use runtime_core::{Bound, Element, IdealystSchema, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Video` instance. Owned by the SDK, not
/// the framework — the framework just type-erases this struct behind
/// `Element::External { payload: Rc<dyn Any>, .. }` and hands it back
/// to the registered backend handler on mount.
///
/// `src` is reactive: pass a closure that reads from a `Signal`/`Source`
/// to swap the playing clip from app state. `autoplay`, `controls`, and
/// `loop_playback` are static at construction time — re-rendering with
/// different values would tear down and re-mount the view, which is
/// what the author wants in those cases anyway.
#[derive(IdealystSchema)]
pub struct VideoProps {
    /// What to display — one extensible prop for any media source. Build it
    /// with [`url`] (a fetched URL: file / HLS / DASH / live / `data:`),
    /// [`stream`] (a live [`MediaStream`](media_stream::MediaStream) from
    /// `camera` / `screen-recorder`), or your own [`VideoSource`]. The
    /// backend resolves it inside a reactive `Effect`, so a source that
    /// reads signals re-populates the view on change.
    #[schema(constraint = "a VideoSource — url(...) / stream(...) / custom")]
    pub source: Box<dyn VideoSource>,
    /// Begin playback immediately on mount. Most platforms require the
    /// video to be muted for autoplay to work without a user gesture;
    /// the per-backend impls pair `autoplay = true` with a silent
    /// start automatically.
    pub autoplay: bool,
    /// Show native playback controls (play/pause scrubber, volume,
    /// fullscreen). Whether this renders matches the platform's native
    /// look — iOS UIKit controls, Android MediaController, browser
    /// `<video controls>`.
    pub controls: bool,
    /// Restart from the beginning when playback reaches the end. Field
    /// name avoids the `loop` keyword. Ignored for a live stream source.
    pub loop_playback: bool,
}

impl Default for VideoProps {
    fn default() -> Self {
        Self {
            source: Box::new(NoSource),
            autoplay: false,
            controls: false,
            loop_playback: false,
        }
    }
}

// ============================================================================
// Media source — one extensible abstraction for "what a Video displays".
// ============================================================================

/// What a [`VideoSource`] resolves to: the small, platform-agnostic set of
/// ways a video view can be populated. Backends match on this closed set;
/// source authors produce it. `#[non_exhaustive]` so a new mechanism (e.g. a
/// GPU-texture handoff for the compositing layer) can be added without
/// breaking implementors.
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
/// inside a reactive `Effect`, so any signal read there makes the view
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
/// reads of captured signals re-load the player). Replaces the old `src(...)`
/// helper.
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
/// Video(VideoProps { source: stream(camera_stream),         ..Default::default() })
/// Video(VideoProps { source: stream(move || sig.get()),     ..Default::default() })
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
// trait, so this dodges the closure-coherence conflict (see the crate docs).
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
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `Video`. Filled by `Ref::fill` after the
/// primitive mounts; users hold a `Ref<VideoHandle>` at the call site
/// and reach imperative ops via `r.with(|h| h.play())`.
///
/// The `ops` pointer is set by the active backend's module via the
/// `OPS` static (see the cfg-gated re-export at the bottom of this
/// file). The `node` is type-erased — each backend's ops downcasts it
/// internally to the concrete native type (`HtmlMediaElement` /
/// `Retained<NSObject>` (AVPlayer) / `GlobalRef`).
#[derive(Clone)]
pub struct VideoHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn VideoOps,
}

impl VideoHandle {
    /// Wrap a type-erased native node + backend ops into a handle.
    /// Called by the `RefFill::External` closure that [`VideoBind::bind`]
    /// installs; user code receives the handle through `Ref::with`.
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
}

/// Imperative-ops dispatch. Implementations live in each cfg-gated
/// backend module and downcast `node` to their concrete native type.
/// Defaults all no-op so a backend that hasn't wired a particular op
/// degrades silently rather than panicking.
///
/// `Sync` bound: the trait object lives in a `static OPS: &dyn
/// VideoOps` slot per backend module, which Rust requires to be `Sync`.
/// The ZST impls each backend ships are trivially `Sync`.
pub trait VideoOps: Sync {
    /// Start (or resume) playback. Default no-op.
    fn play(&self, _node: &dyn Any) {}
    /// Pause playback. Default no-op.
    fn pause(&self, _node: &dyn Any) {}
    /// Seek to the given offset in seconds. Default no-op.
    fn seek(&self, _node: &dyn Any, _seconds: f32) {}
}

/// Fallback ops used on targets with no `Video` impl. Every method is
/// a no-op; user code keeps compiling but the framework's `External`
/// placeholder is what actually renders.
pub struct UnsupportedOps;
impl VideoOps for UnsupportedOps {}

// ============================================================================
// Constructor + bind
// ============================================================================

/// Build a `Video` primitive. Returns a typed `Bound<VideoHandle>` so
/// `.bind(...)` is type-checked against `Ref<VideoHandle>`.
///
/// PascalCase intentionally — matches the visual cadence of first-party
/// primitives (`View`, `Button`, `Image`) inside a `ui!` block.
/// Interpolate as `{ video::Video(VideoProps { .. }) }`.
///
/// Under the hood this is `Element::External` with a `VideoProps`
/// payload — same machinery as any other third-party SDK. The marker
/// type on `Bound<H>` is `VideoHandle` so the `.bind(...)` from
/// [`VideoBind`] resolves with type-checked refs.
#[allow(non_snake_case)]
pub fn Video(props: VideoProps) -> Bound<VideoHandle> {
    Bound::new(Element::External {
        type_id: TypeId::of::<VideoProps>(),
        type_name: std::any::type_name::<VideoProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        children: Vec::new(),
        style: None,
        ref_fill: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

/// Adds `.bind(r)` to `Bound<VideoHandle>` via an extension trait (the
/// orphan rule blocks an inherent `impl Bound<VideoHandle>` here —
/// `Bound` is foreign). Bring this trait into scope to use the builder-
/// style `.bind(...)` on the value returned by [`Video`].
///
/// Most users don't import this directly — the `prelude` re-export
/// gives them the trait + the constructor + the props struct in one
/// line.
pub trait VideoBind {
    /// Bind a `Ref<VideoHandle>` for imperative access. At mount time
    /// the framework calls the `RefFill::External` closure with the
    /// type-erased native node; we wrap it in a `VideoHandle` using
    /// the cfg-selected backend's `OPS` static and fill the ref.
    fn bind(self, r: Ref<VideoHandle>) -> Self;
}

impl VideoBind for Bound<VideoHandle> {
    fn bind(mut self, r: Ref<VideoHandle>) -> Self {
        if let Element::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(VideoHandle::new(node_any, OPS));
            })));
        }
        self
    }
}

/// One-stop import for typical use: `use video::prelude::*;` brings in
/// the constructor, props struct, handle type, the `.bind(...)`
/// extension trait, and the `src(...)` coercion helper.
pub mod prelude {
    pub use super::{
        stream, url, MediaContent, Video, VideoBind, VideoHandle, VideoProps, VideoSource,
    };
}

// ============================================================================
// Backend selector
// ============================================================================

// Each platform module exposes:
//   - `pub fn register(backend: &mut <ConcreteBackend>)`
//   - `pub static OPS: &dyn VideoOps`
// Only one is compiled per target via cfg; the umbrella re-exports both
// from whichever module matches. On targets with no backend support,
// fallbacks here keep user code compiling — the framework's External
// placeholder is what renders at runtime.

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;
#[cfg(target_arch = "wasm32")]
static OPS: &dyn VideoOps = web::OPS;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
static OPS: &dyn VideoOps = android::OPS;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
static OPS: &dyn VideoOps = ios::OPS;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
mod fallback {
    use runtime_core::Backend;

    /// No-op register for unsupported targets. User code calls this
    /// unconditionally; the framework's External placeholder shows up
    /// at runtime to make the missing binding obvious.
    pub fn register<B: Backend>(_backend: &mut B) {}
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
pub use fallback::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
static OPS: &dyn VideoOps = &UnsupportedOps;
