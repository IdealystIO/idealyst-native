//! What to record — the input to [`crate::ScreenRecorder::start`]. The
//! frame sink is *not* here: it's a separate callback argument to `start`
//! (see [`crate::FrameCallback`]), because it has to satisfy a cfg'd
//! `Send` bound the backend's capture thread requires, exactly like
//! `microphone`'s `open(config, callback)`.

/// Default capture frame rate when the caller doesn't set one. Extracted
/// rather than inlined per the repo's no-magic-numbers rule.
pub const DEFAULT_FPS: u32 = 30;

/// What the recording captures.
pub enum Source {
    /// This app's own rendered content. The common case, and the only
    /// source that can pair with a [`crate::PrivateLayer`] to exclude an
    /// overlay (on every backend that supports exclusion).
    ThisApp,
    /// The user picks a window or screen via the OS source picker.
    /// Desktop (macOS/Windows/Linux) and web. On iOS/Android there is no
    /// picker; backends there treat this as [`Source::ThisApp`].
    UserChoice,
    /// The entire screen / primary display.
    FullScreen,
    /// A specific *other* window. Desktop-only (macOS/Windows); other
    /// backends return [`crate::RecorderError::UnsupportedSource`].
    Window(WindowSelector),
}

/// Opaque selector for [`Source::Window`]. Intentionally minimal in the
/// skeleton; desktop impls will grow real selectors (owning PID + native
/// window id) as they're built.
pub struct WindowSelector {
    /// A substring of the target window's title, used as a best-effort
    /// hint for the desktop picker/enumeration. Real impls add precise
    /// native identifiers alongside this.
    pub title_hint: Option<String>,
}

/// Which audio track(s), if any, to capture alongside video. Audio
/// frames are out of scope for the video-frame skeleton callback; this
/// enum reserves the shape so the API doesn't churn when audio lands.
pub enum AudioSource {
    /// No audio.
    None,
    /// This app's / the captured app's audio output.
    App,
    /// The whole system's audio output.
    System,
    /// The microphone.
    Microphone,
    /// App output mixed with the microphone.
    AppAndMic,
}

/// A full recording request. Construct with [`RecordingConfig::new`] and
/// refine with the builder setters. The frame sink is a separate argument
/// to [`crate::ScreenRecorder::start`], not a field here.
pub struct RecordingConfig {
    /// What to capture.
    pub source: Source,
    /// Which audio to capture (reserved; see [`AudioSource`]).
    pub audio: AudioSource,
    /// Target frame rate. The backend may clamp to what its capture API
    /// supports.
    pub fps: u32,
    /// Target output size in pixels, or `None` for the source's native
    /// size.
    pub size: Option<(u32, u32)>,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingConfig {
    /// A default request: [`Source::ThisApp`], no audio, [`DEFAULT_FPS`],
    /// native size. Refine with the builder setters.
    pub fn new() -> Self {
        Self {
            source: Source::ThisApp,
            audio: AudioSource::None,
            fps: DEFAULT_FPS,
            size: None,
        }
    }

    /// Set what to capture.
    pub fn source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// Set which audio to capture.
    pub fn audio(mut self, audio: AudioSource) -> Self {
        self.audio = audio;
        self
    }

    /// Set the target frame rate.
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    /// Set the target output size in pixels.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }
}
