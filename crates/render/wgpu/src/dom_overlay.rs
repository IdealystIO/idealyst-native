//! Host-side "external rendering plane" — DOM elements stamped
//! over the wgpu canvas at the screen rect of nodes whose paint
//! is delegated to the platform rather than the GPU.
//!
//! The wgpu preview renders everything into a single
//! `wgpu::TextureView`, but the `Video` primitive doesn't fit
//! that model on the web target. There's no in-process H.264
//! decoder on wasm; the natural compositor is the browser
//! itself. This trait lets the host shell (currently `host-web`)
//! own a sibling DOM layer over the canvas and reposition
//! `<video>` children to track the framework's layout each frame.
//!
//! Lifecycle, called by the renderer:
//!
//! 1. [`DomOverlay::begin_frame`] — reset the "seen this frame"
//!    bookkeeping. The host clears whichever set it uses to
//!    detect stale children.
//! 2. [`DomOverlay::place_video`] — fired once per visible Video
//!    node during the tree walk, after layout-resolution.
//!    Idempotent: the same `key` for the same node every frame;
//!    the host should find-or-create the matching DOM child and
//!    sync attributes. `rect` is logical CSS px against the
//!    canvas origin.
//! 3. [`DomOverlay::end_frame`] — the renderer is done emitting
//!    placements for the frame. The host drops any DOM children
//!    not touched between `begin_frame` and now.
//!
//! Native shells (`host-winit`) don't install an overlay; the
//! `Renderer::set_dom_overlay` slot stays `None` and the methods
//! are never called. The wgpu side composites openh264 into its
//! own texture as before.
//!
//! Why a trait rather than a fixed `host-web` callback: the
//! renderer crate is platform-agnostic on purpose (no `web-sys`,
//! no `wasm-bindgen`). Punting the actual DOM work to a host
//! impl keeps that contract intact.

/// Stable identity of a node for the lifetime of an overlay
/// session — derived from the underlying `Rc<RefCell<NodeData>>`
/// pointer. Two `place_*` calls with the same key are the same
/// node; the host should update in place rather than recreating.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct DomOverlayKey(pub usize);

/// Per-frame snapshot of a `<video>` placement. Kept as a struct
/// rather than positional args so adding flags (preload mode,
/// crossorigin, …) later doesn't ripple through every shell.
pub struct DomVideoSpec<'a> {
    /// Source URL or file path. The host is responsible for
    /// rewriting `file://` / relative paths if its runtime needs
    /// them in a different shape.
    pub src: &'a str,
    pub autoplay: bool,
    pub loop_playback: bool,
    pub muted: bool,
    pub volume: f32,
    /// `Some(target_secs)` if the framework's controls fired a
    /// seek since the previous frame. The host consumes it and
    /// clears the slot. `None` means "no new seek".
    pub seek: Option<f64>,
    /// `true` while the controls overlay wants the video playing;
    /// the host should sync `playPause` accordingly. Matched
    /// against the `<video>.paused` state so we only call into
    /// the element when it diverges.
    pub playing: bool,
}

pub trait DomOverlay {
    /// Mark the start of a frame's placement batch. Hosts use
    /// this to reset whatever "still visible" tracking they keep.
    fn begin_frame(&self);

    /// Place / update a `<video>` at `rect` with the playback
    /// state in `spec`. The host owns the video element lifecycle
    /// keyed by `key`.
    fn place_video(
        &self,
        key: DomOverlayKey,
        spec: DomVideoSpec<'_>,
        rect: (f32, f32, f32, f32),
        opacity: f32,
    );

    /// All placements for the frame have been emitted. The host
    /// drops DOM children whose `key` wasn't touched this frame.
    fn end_frame(&self);
}
