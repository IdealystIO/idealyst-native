//! What to ask the device for. Resolution and frame rate are optional —
//! `None` means "let the platform pick its native default", which is what
//! most callers want (the device's preferred mode is the cheapest path,
//! no rescaling). Facing has an explicit `Default` for the same reason.

/// Which physical camera to open.
///
/// `Default` defers to the platform's idea of the primary camera — the
/// back camera on a phone, the only camera on a laptop. On a device with
/// just one camera, `Front`/`Back` that can't be satisfied surface as
/// [`CameraError::NoCamera`](crate::CameraError::NoCamera) rather than
/// silently opening the wrong one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraFacing {
    /// The platform's primary camera (back on mobile, the built-in on a
    /// laptop).
    #[default]
    Default,
    /// The user-facing ("selfie") camera.
    Front,
    /// The world-facing (rear) camera.
    Back,
}

/// Requested capture parameters. A `None` field defers to the device's
/// preferred value. Construct with [`CameraConfig::default`] (device
/// defaults, primary camera) or the small builders below.
///
/// These are *requests*. A backend that can't honour an explicit value
/// returns [`CameraError::UnsupportedConfig`](crate::CameraError::UnsupportedConfig)
/// rather than silently substituting — so the actual `width`/`height` you
/// observe on each [`VideoFrame`](crate::VideoFrame) are authoritative.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraConfig {
    /// Desired frame width in pixels. `None` = the device default.
    pub width: Option<u32>,

    /// Desired frame height in pixels. `None` = the device default.
    pub height: Option<u32>,

    /// Desired frame rate in frames per second. `None` = the device
    /// default (commonly 30).
    pub fps: Option<u32>,

    /// Which camera to open. Defaults to [`CameraFacing::Default`].
    pub facing: CameraFacing,
}

impl CameraConfig {
    /// Device defaults for everything, primary camera — the recommended
    /// starting point.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request a specific frame resolution, leaving frame rate at the
    /// device default.
    pub fn with_resolution(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Request a specific frame rate.
    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Open the user-facing (front) camera.
    pub fn front(mut self) -> Self {
        self.facing = CameraFacing::Front;
        self
    }

    /// Open the world-facing (back) camera.
    pub fn back(mut self) -> Self {
        self.facing = CameraFacing::Back;
        self
    }
}
