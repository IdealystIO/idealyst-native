//! Where a recording goes and how it's encoded.

use std::sync::Arc;

use files::FileStore;

/// Default target video frame rate when the config doesn't pin one.
pub const DEFAULT_FPS: u32 = 30;

/// Container/codec the file is written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Container {
    /// `.mp4` — H.264 video + AAC audio. The portable default; plays on every
    /// target's native player.
    #[default]
    Mp4,
}

impl Container {
    /// The file extension (no dot) for this container.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
        }
    }
}

/// Destination + encoding settings for a [`record`](crate::MediaWriter::record)
/// call.
///
/// The output is addressed through a [`FileStore`] + relative `path` so one
/// API works everywhere: a native muxer resolves the store's real
/// [`local_path`](files::FileStore::local_path); the web backend writes the
/// recorded blob back through the store. Build the store with
/// [`files::app_files`].
pub struct RecordConfig {
    pub(crate) store: Arc<dyn FileStore>,
    pub(crate) path: String,
    pub(crate) container: Container,
    pub(crate) fps: u32,
    pub(crate) video_bitrate: Option<u32>,
    pub(crate) audio_bitrate: Option<u32>,
}

impl RecordConfig {
    /// A recording written to `path` (relative) within `store`, as the default
    /// [`Container::Mp4`] at [`DEFAULT_FPS`].
    pub fn new(store: Arc<dyn FileStore>, path: impl Into<String>) -> Self {
        Self {
            store,
            path: path.into(),
            container: Container::default(),
            fps: DEFAULT_FPS,
            video_bitrate: None,
            audio_bitrate: None,
        }
    }

    /// Set the output [`Container`].
    pub fn container(mut self, container: Container) -> Self {
        self.container = container;
        self
    }

    /// Set the target video frame rate (a hint to the encoder; the real
    /// cadence follows the source's capture timestamps).
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    /// Set the target video bitrate in bits per second. `None` lets the
    /// encoder choose a default for the resolution.
    pub fn video_bitrate(mut self, bps: u32) -> Self {
        self.video_bitrate = Some(bps);
        self
    }

    /// Set the target audio bitrate in bits per second. `None` lets the
    /// encoder choose a default.
    pub fn audio_bitrate(mut self, bps: u32) -> Self {
        self.audio_bitrate = Some(bps);
        self
    }
}
