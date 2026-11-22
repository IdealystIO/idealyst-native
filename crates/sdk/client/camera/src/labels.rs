//! The names this crate gives a camera the platform leaves unnamed.

use std::sync::{Arc, RwLock};

/// What a camera is called when the platform names none: Android's
/// Camera2 names no camera, and a browser or Windows can leave one blank.
/// Repeats are numbered after ("Back camera 2"). English until the app
/// installs its own with [`install_camera_labels`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraLabels {
    /// A front-facing camera.
    pub front: String,
    /// A back-facing camera.
    pub back: String,
    /// A camera that reports no facing.
    pub camera: String,
}

impl CameraLabels {
    /// The English names.
    pub fn english() -> Self {
        Self { front: "Front camera".into(), back: "Back camera".into(), camera: "Camera".into() }
    }
}

impl Default for CameraLabels {
    fn default() -> Self {
        Self::english()
    }
}

// Process-wide rather than per-thread: a backend may list its cameras on a
// worker thread.
static INSTALLED: RwLock<Option<Arc<CameraLabels>>> = RwLock::new(None);

/// Make `labels` the names [`Camera::devices`](crate::Camera::devices)
/// gives unnamed cameras from now on.
pub fn install_camera_labels(labels: CameraLabels) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(labels));
}

/// The installed names, or [`CameraLabels::english`].
pub fn camera_labels() -> Arc<CameraLabels> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(CameraLabels::english()))
}
