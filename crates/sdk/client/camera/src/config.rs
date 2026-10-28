//! What to ask the device for, and what [`Camera::devices`](crate::Camera::devices)
//! reports. Resolution and frame rate are optional — `None` means "let the
//! platform pick its native default", which is what most callers want (the
//! device's preferred mode is the cheapest path, no rescaling). Facing has an
//! explicit `Default` for the same reason.

use crate::CameraError;

/// Which physical camera to open.
///
/// `Default` defers to the platform's idea of the primary camera — the
/// back camera on a phone, the only camera on a laptop. On a device with
/// just one camera, `Front`/`Back` that can't be satisfied surface as
/// [`CameraError::NoCamera`](crate::CameraError::NoCamera) rather than
/// silently opening the wrong one — on every backend.
///
/// A camera that reports no facing (a desktop webcam; everything V4L2
/// exposes) counts as user-facing: `Front` opens it, `Back` is `NoCamera`.
/// So `Back` on a laptop fails the same way everywhere, and an app can hide
/// a front/back switch when the other side fails to open.
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

    /// A specific camera to open: the [`id`](CameraDevice::id) of one entry
    /// from [`Camera::devices`](crate::Camera::devices). `None` resolves the
    /// camera from [`facing`](Self::facing). An id no attached camera has is
    /// [`CameraError::NoCamera`](crate::CameraError::NoCamera). Setting it
    /// together with a `Front`/`Back` facing is
    /// [`CameraError::UnsupportedConfig`](crate::CameraError::UnsupportedConfig):
    /// the device already decides the facing, and the two could disagree.
    pub device: Option<String>,
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

    /// Open the camera with this [`CameraDevice::id`], from
    /// [`Camera::devices`](crate::Camera::devices).
    pub fn with_device(mut self, id: impl Into<String>) -> Self {
        self.device = Some(id.into());
        self
    }

    /// The config error a backend must not get past: a pinned device together
    /// with a `Front`/`Back` facing. Checked once, in [`Camera::open`](crate::Camera::open),
    /// so every backend rejects it identically.
    pub(crate) fn conflict(&self) -> Option<CameraError> {
        match (&self.device, self.facing) {
            (Some(_), CameraFacing::Front | CameraFacing::Back) => Some(CameraError::UnsupportedConfig(
                "a device id and a front/back facing can't both be set".into(),
            )),
            _ => None,
        }
    }
}

/// Which way a listed camera points, as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceFacing {
    /// The user-facing ("selfie") camera.
    Front,
    /// The world-facing (rear) camera.
    Back,
    /// The platform reports no front/back for this camera: desktop and USB
    /// webcams, every V4L2 and Media Foundation device, an Android external
    /// camera. [`CameraFacing::Front`] opens such a camera when it is the
    /// platform's primary one (see [`CameraFacing`]).
    Unreported,
}

/// One camera attached to the device, from [`Camera::devices`](crate::Camera::devices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    /// The platform's stable identifier for this camera. Pass it to
    /// [`CameraConfig::with_device`] to open this camera. Opaque — its format
    /// differs per platform (an `AVCaptureDevice.uniqueID`, a Camera2 id, a web
    /// `deviceId`, a `/dev/videoN` path, a Media Foundation symbolic link) —
    /// and stable across launches on the platforms that keep one, so an app
    /// can store the user's choice.
    pub id: String,
    /// A human-readable name to show in a picker ("FaceTime HD Camera",
    /// "Back camera"). Never empty. Platforms that name no cameras (Android)
    /// get one built from the facing.
    pub label: String,
    /// Which way the camera points.
    pub facing: DeviceFacing,
}
