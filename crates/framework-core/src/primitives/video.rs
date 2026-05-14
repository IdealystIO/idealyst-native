//! Video primitive.
//!
//! Backends use their native players — codec/format support is
//! whatever the platform handles. mp4 is universal; webm/hls/m3u8
//! depend on the platform.
//!   - Web: `<video>` element.
//!   - iOS: `AVPlayerLayer` / `AVPlayer`.
//!   - Android: `VideoView` or `ExoPlayer`.
//!
//! URL is reactive; props for autoplay/controls/loop are static at
//! construction time. The handle exposes `play()`, `pause()`,
//! `seek(seconds)` for programmatic control.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct VideoHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn VideoOps,
}

impl VideoHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn VideoOps) -> Self {
        Self { node, ops }
    }

    pub fn play(&self) {
        self.ops.play(&*self.node);
    }

    pub fn pause(&self) {
        self.ops.pause(&*self.node);
    }

    /// Seek to the given offset in seconds.
    pub fn seek(&self, seconds: f32) {
        self.ops.seek(&*self.node, seconds);
    }
}

pub trait VideoOps {
    fn play(&self, node: &dyn Any);
    fn pause(&self, node: &dyn Any);
    fn seek(&self, node: &dyn Any, seconds: f32);
}

pub trait IntoVideoSrc {
    fn into_video_src(self) -> Box<dyn Fn() -> String>;
}

impl IntoVideoSrc for &str {
    fn into_video_src(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}

impl IntoVideoSrc for String {
    fn into_video_src(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl<F> IntoVideoSrc for F
where
    F: Fn() -> String + 'static,
{
    fn into_video_src(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}

/// Construct a Video primitive. Defaults: no autoplay, no controls,
/// no loop. Use the builder methods below to opt in.
pub fn video<S: IntoVideoSrc>(src: S) -> Bound<VideoHandle> {
    Bound::new(Primitive::Video {
        src: src.into_video_src(),
        autoplay: false,
        controls: false,
        loop_playback: false,
        style: None,
        ref_fill: None,
    })
}

impl Bound<VideoHandle> {
    pub fn autoplay(mut self, v: bool) -> Self {
        if let Primitive::Video { autoplay, .. } = &mut self.primitive {
            *autoplay = v;
        }
        self
    }

    pub fn controls(mut self, v: bool) -> Self {
        if let Primitive::Video { controls, .. } = &mut self.primitive {
            *controls = v;
        }
        self
    }

    pub fn loop_playback(mut self, v: bool) -> Self {
        if let Primitive::Video { loop_playback, .. } = &mut self.primitive {
            *loop_playback = v;
        }
        self
    }

    pub fn bind(mut self, r: Ref<VideoHandle>) -> Self {
        if let Primitive::Video { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Video(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
