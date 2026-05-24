//! Wasm32 stub for the wgpu video pipeline.
//!
//! The native `video.rs` decodes H.264 mp4 files via the bundled
//! Cisco `openh264` C++ source plus `re_mp4` and pipes the audio
//! track through `cpal`. Neither stack compiles to wasm32 — see
//! `Cargo.toml` for the `[target.'cfg(not(target_arch = "wasm32"))']`
//! gating that drops them on the web target.
//!
//! This file preserves the public surface that `node.rs`,
//! `backend_impl.rs`, and `renderer.rs` rely on — `VideoFrame`,
//! `VideoSharedState`, `VideoDecoder`. The `latest_frame` slot
//! always stays `None` so the renderer's wgpu pre-pass writes
//! nothing; the actual <video> element is mounted by the host
//! shell through the [`crate::dom_overlay::DomOverlay`] hook and
//! sized to the node's screen rect every frame.
//!
//! `src` is held verbatim so the host can read it back during
//! `DomOverlay::place_video` and create / update the
//! `HTMLVideoElement`'s `src` attribute. Playback state
//! (autoplay / loop) is stored alongside; the controls + audio
//! mute toggle round-trip through the same atomics the native
//! pipeline uses so the renderer's `paint_video_controls` code
//! still works (it overlays a play/pause + scrubber bar via the
//! standard rect pipeline regardless of who actually decodes
//! the frames).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct VideoSharedState {
    pub latest_frame: Mutex<Option<VideoFrame>>,
    pub playing: AtomicBool,
    pub shutdown: AtomicBool,
    pub frame_counter: AtomicU64,
    pub current_time_micros: AtomicU64,
    pub duration_micros: AtomicU64,
    pub seek_request: Mutex<Option<u64>>,
}

impl VideoSharedState {
    fn new() -> Self {
        Self {
            latest_frame: Mutex::new(None),
            playing: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            frame_counter: AtomicU64::new(0),
            current_time_micros: AtomicU64::new(0),
            duration_micros: AtomicU64::new(0),
            seek_request: Mutex::new(None),
        }
    }
}

/// Wasm `VideoDecoder` stub. Holds the metadata the DOM overlay
/// needs to mount and update a `<video>` element. No threads, no
/// decoder, no audio mixer — the browser does all of that.
pub struct VideoDecoder {
    pub shared: Arc<VideoSharedState>,
    src: String,
    autoplay: bool,
    loop_playback: bool,
    /// User-facing mute toggle. Round-tripped to the
    /// `<video>.muted` attribute by the host. Stored here so
    /// `is_audio_muted()` can report the same value the controls
    /// overlay reads each frame.
    muted: AtomicBool,
    volume: Mutex<f32>,
}

impl VideoDecoder {
    pub fn spawn(src: String, autoplay: bool, loop_playback: bool) -> Self {
        let shared = Arc::new(VideoSharedState::new());
        shared.playing.store(autoplay, Ordering::Release);
        Self {
            shared,
            src,
            autoplay,
            loop_playback,
            muted: AtomicBool::new(false),
            volume: Mutex::new(1.0),
        }
    }

    pub fn src(&self) -> &str {
        &self.src
    }
    pub fn autoplay(&self) -> bool {
        self.autoplay
    }
    pub fn loop_playback(&self) -> bool {
        self.loop_playback
    }
    pub fn volume(&self) -> f32 {
        self.volume.lock().map(|g| *g).unwrap_or(1.0)
    }

    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
    }

    pub fn set_playing(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Release);
    }

    pub fn set_volume(&self, vol: f32) {
        if let Ok(mut v) = self.volume.lock() {
            *v = vol.clamp(0.0, 1.0);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    pub fn is_audio_muted(&self) -> Option<bool> {
        Some(self.muted.load(Ordering::Acquire))
    }

    pub fn seek(&self, target_secs: f64) {
        let target_micros = (target_secs.max(0.0) * 1_000_000.0) as u64;
        if let Ok(mut g) = self.shared.seek_request.lock() {
            *g = Some(target_micros);
        }
    }
}
