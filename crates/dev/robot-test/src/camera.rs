//! `app.camera()`: feed the app's camera from a test image — the `camera`
//! SDK's `camera.*` controls, typed. The app registers them once at boot
//! (`camera::register_robot_controls()`). The image is decoded here, on the
//! test's machine, and sent as RGBA, so the app carries no decoder.
//!
//! ```ignore
//! app.camera().fake_image("tests/fixtures/face.jpg");
//! app.test_id("take-photo").click();        // the kiosk's photo step sees the face
//! app.camera().clear();
//! ```

use std::path::Path;

use base64::Engine as _;
use serde_json::json;

use crate::App;

/// The app's camera, through the robot. From [`App::camera`].
pub struct Camera<'a> {
    pub(crate) app: &'a mut App,
}

impl Camera<'_> {
    /// Every camera the app opens from now on streams this image (PNG or
    /// JPEG on the test's disk).
    pub fn fake_image(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path = path.as_ref();
        let img = image::open(path)
            .unwrap_or_else(|e| panic!("camera().fake_image({}): {e}", path.display()))
            .to_rgba8();
        let (w, h) = img.dimensions();
        self.fake_rgba(w, h, img.as_raw())
    }

    /// Every camera the app opens from now on streams these pixels:
    /// straight RGBA8, top-down, `width * height * 4` bytes.
    pub fn fake_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> &mut Self {
        let b64 = base64::engine::general_purpose::STANDARD.encode(rgba);
        self.app.call_function("camera.fake", json!({ "width": width, "height": height, "rgba": b64 }));
        self
    }

    /// How many camera streams the fake has served.
    pub fn opened(&mut self) -> u64 {
        let v = self.app.call_function("camera.status", json!({}));
        v["opened"].as_u64().unwrap_or(0)
    }

    /// Back to the real camera, for streams opened from now on.
    pub fn clear(&mut self) -> &mut Self {
        self.app.call_function("camera.clear", json!({}));
        self
    }
}
