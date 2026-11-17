//! Robot test controls: a fake camera fed from a test image, for flows a
//! test can't point a real camera at (a kiosk's photo step, face match).
//! Compiled only into robot builds; elsewhere [`fake`] is always `None`.
//!
//! While a fake is set (`camera.fake {width, height, rgba}`), every
//! [`Camera::open`](crate::Camera::open) streams that image instead of a
//! device: `latest` / `subscribe` deliver it, and a display shows it (on
//! web through a canvas-backed browser stream, so `<video>` plays it like a
//! real camera; natively through the CPU frame channel). Permission is
//! granted with no prompt and `devices()` lists one "Robot test camera".
//! `camera.clear` goes back to the real camera; streams already open keep
//! the image until closed.

/// The test image: straight RGBA8, top-down, `width * height * 4` bytes.
#[derive(Clone)]
pub(crate) struct FakeImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: std::sync::Arc<Vec<u8>>,
}

/// The id of the fake camera in `devices()`.
pub(crate) const FAKE_DEVICE_ID: &str = "robot-test-camera";

runtime_vocabulary::robot_select! {
    on: {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};

        use base64::Engine as _;
        use runtime_core::robot_fn;
        use runtime_vocabulary::robot_fn::register as register_fn;

        /// Read when a camera opens, which may be off the UI thread.
        static FAKE: Mutex<Option<FakeImage>> = Mutex::new(None);
        static OPENED: AtomicU64 = AtomicU64::new(0);

        pub(crate) fn fake() -> Option<FakeImage> {
            FAKE.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        pub(crate) fn note_opened() {
            OPENED.fetch_add(1, Ordering::Relaxed);
        }

        /// Feed every camera the app opens from now on with this image:
        /// `rgba` is base64 of `width * height * 4` bytes, straight RGBA8,
        /// top-down. Permission is granted without a prompt, and `devices()`
        /// lists one "Robot test camera".
        #[robot_fn(name = "camera.fake")]
        fn set_fake(width: u32, height: u32, rgba: String) -> Result<(), String> {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(rgba.as_bytes())
                .map_err(|e| format!("camera.fake: rgba: {e}"))?;
            let want = width as usize * height as usize * 4;
            if width == 0 || height == 0 || bytes.len() != want {
                return Err(format!(
                    "camera.fake: {width}x{height} RGBA8 is {want} bytes, got {}",
                    bytes.len()
                ));
            }
            *FAKE.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(FakeImage { width, height, rgba: Arc::new(bytes) });
            Ok(())
        }

        /// Back to the real camera for streams opened from now on.
        #[robot_fn(name = "camera.clear")]
        fn clear() {
            *FAKE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }

        /// Whether a fake is set, and how many streams it has served:
        /// `{fake, opened}`.
        #[robot_fn(name = "camera.status")]
        fn status() -> runtime_core::__serde_json::Value {
            runtime_core::__serde_json::json!({
                "fake": fake().is_some(),
                "opened": OPENED.load(Ordering::Relaxed),
            })
        }

        pub(crate) fn register() {
            register_fn(set_fake::robot());
            register_fn(clear::robot());
            register_fn(status::robot());
        }
    }
    off: {
        pub(crate) fn fake() -> Option<FakeImage> {
            None
        }
        pub(crate) fn note_opened() {}
        pub(crate) fn register() {}
    }
}

/// Native: a thread writes the image at a camera's cadence until the
/// stream's last clone drops.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open(img: FakeImage, writer: media_stream::FrameWriter) -> impl Drop {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Frames a second: enough for a preview and for a consumer that waits
    /// for the next frame; the image never changes.
    const FPS: u64 = 15;

    struct Handle {
        running: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }
    let running = Arc::new(AtomicBool::new(true));
    let run = running.clone();
    // Written once now, so `latest()` has the image the moment `open`
    // returns, then repeated for subscribers that attach later.
    writer.write_rgba8(img.width, img.height, &img.rgba);
    let join = std::thread::Builder::new()
        .name("robot-camera".into())
        .spawn(move || {
            while run.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(1000 / FPS));
                writer.write_rgba8(img.width, img.height, &img.rgba);
            }
        })
        .ok();
    Handle { running, join }
}
