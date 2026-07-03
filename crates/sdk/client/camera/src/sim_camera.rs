//! iOS **Simulator** camera — a synthetic video stream.
//!
//! The iOS Simulator has no camera hardware (`AVCaptureSession` finds no device),
//! so camera-dependent UI can't be exercised there at all. This stand-in produces
//! an animated test pattern delivered as a normal [`MediaStream`], so the exact
//! author code that opens a real camera works on the simulator — and it's
//! deliberately, obviously synthetic so it's never mistaken for a real feed.
//!
//! Compiled ONLY for the simulator (`cfg(target_abi = "sim")`, wired in `lib.rs`);
//! on a real iOS device — and on macOS — the AVFoundation backend in
//! [`apple.rs`](super) runs instead, so none of this ships to devices. This is
//! the iOS analogue of the Android emulator's built-in synthetic camera.
//!
//! Threading mirrors the real backend: frames are generated on a background
//! thread and pushed through the [`FrameWriter`] (which is `Send + Sync`), the
//! same producer/consumer split the AVFoundation delegate uses on its private
//! dispatch queue. Consumers read via `latest()` / `subscribe` exactly as they
//! would for a real camera.

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Default synthetic resolution when the caller requests none — a light 4:3 the
/// corner-of-screen preview / recording handles comfortably.
const DEFAULT_W: u32 = 640;
const DEFAULT_H: u32 = 480;
/// Default cadence (the common real-camera default).
const DEFAULT_FPS: u32 = 30;
/// Cap an explicit request so a stray large value can't peg the CPU on the
/// per-pixel software renderer — this is a dev stand-in, not a real sensor.
const MAX_DIM: u32 = 1280;

/// Owns the generator thread; dropping it stops capture (mirrors the real
/// backend's `StreamHandle`, whose `Drop` tears down the `AVCaptureSession`).
pub(crate) struct StreamHandle {
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            // The thread checks `running` once per frame, so it exits within one
            // frame period — joining keeps the `FrameWriter` alive until then.
            let _ = j.join();
        }
    }
}

/// No OS permission on the simulator — the synthetic camera is always available.
pub(crate) async fn request_permission() -> Result<(), CameraError> {
    Ok(())
}

/// Start the synthetic stream. Honors `config` resolution/fps where given
/// (clamped), defers to a sensible default otherwise — same contract as a real
/// backend, where the observed `VideoFrame` dimensions are authoritative.
pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    let w = config.width.unwrap_or(DEFAULT_W).clamp(16, MAX_DIM);
    let h = config.height.unwrap_or(DEFAULT_H).clamp(16, MAX_DIM);
    let fps = config.fps.unwrap_or(DEFAULT_FPS).clamp(1, 60);
    let facing = config.facing;
    let period = Duration::from_micros(1_000_000 / fps as u64);

    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();
    let join = thread::Builder::new()
        .name("sim-camera".into())
        .spawn(move || {
            let (wu, hu) = (w as usize, h as usize);
            let mut frame = vec![0u8; wu * hu * 4];
            let mut t: u32 = 0;
            // Produce continuously while open, like a real capture session — not
            // gated on consumers (a puller may attach at any time via `latest()`).
            while running_thread.load(Ordering::Relaxed) {
                render_test_pattern(&mut frame, w, h, t, facing);
                writer.write_rgba8(w, h, &frame);
                t = t.wrapping_add(1);
                thread::sleep(period);
            }
        })
        .map_err(|e| CameraError::Backend(format!("sim camera thread: {e}")))?;

    // No zero-copy native source on the simulator — consumers take the CPU
    // `latest()` / `subscribe` path (the same one the camera-in-canvas compositor
    // already uses on non-zero-copy targets).
    Ok((StreamHandle { running, join: Some(join) }, None))
}

/// Paint one frame of the stand-in: a CALM, static muted-colour gradient with a
/// soft-edged white ball bouncing slowly across it. Motion comes ONLY from the
/// ball — the gradient doesn't pulse or strobe (an earlier version drifted the
/// whole frame's brightness and swept a bright scanline, which reads as flicker
/// to the eye even though every frame is individually correct). A faint
/// facing-based accent keeps `front`/`back` visibly distinct. Output is
/// tightly-packed top-down RGBA8 (what [`FrameWriter::write_rgba8`] expects).
fn render_test_pattern(buf: &mut [u8], w: u32, h: u32, t: u32, facing: CameraFacing) {
    let (wf, hf) = (w as f32, h as f32);
    // Slow phase — the ball drifts gently, nothing strobes.
    let phase = t as f32 * 0.02;

    // Bouncing ball: independent triangle-wave position in each axis.
    let r = wf.min(hf) * 0.14;
    let bx = tri(phase * 0.6) * (wf - 2.0 * r) + r;
    let by = tri(phase * 0.8 + 0.3) * (hf - 2.0 * r) + r;
    let r2 = r * r;

    // Faint facing accent: front=warm, back=cool, default=neutral. Constant per
    // frame (no animation) so it never flickers.
    let (ar, ag, ab) = match facing {
        CameraFacing::Front => (28i32, 0, 8),
        CameraFacing::Back => (0, 10, 28),
        CameraFacing::Default => (0, 0, 0),
    };

    let stride = w as usize * 4;
    for y in 0..h {
        let v = y as f32 / hf;
        let row = y as usize * stride;
        for x in 0..w {
            let u = x as f32 / wf;
            // Muted, fully STATIC two-axis gradient (soft slate→teal→indigo). No
            // per-frame term, so it can't pulse.
            let mut rr = (70.0 + u * 70.0 + (1.0 - v) * 24.0) as i32 + ar;
            let mut gg = (96.0 + v * 86.0 + u * 16.0) as i32 + ag;
            let mut bb = (132.0 + (1.0 - u) * 74.0 + v * 16.0) as i32 + ab;

            // Soft-edged white ball.
            let dx = x as f32 - bx;
            let dy = y as f32 - by;
            let d2 = dx * dx + dy * dy;
            if d2 < r2 {
                // `powf` softens the falloff so the edge is gentle, not a hard disc.
                let k = (1.0 - d2 / r2).clamp(0.0, 1.0).powf(0.6);
                rr = (rr as f32 * (1.0 - k) + 250.0 * k) as i32;
                gg = (gg as f32 * (1.0 - k) + 250.0 * k) as i32;
                bb = (bb as f32 * (1.0 - k) + 250.0 * k) as i32;
            }

            let i = row + x as usize * 4;
            buf[i] = rr.clamp(0, 255) as u8;
            buf[i + 1] = gg.clamp(0, 255) as u8;
            buf[i + 2] = bb.clamp(0, 255) as u8;
            buf[i + 3] = 255;
        }
    }
}

/// Triangle wave in `[0, 1]` from a continuous phase (ping-pong, no trig):
/// `0 → 1 → 0` over each unit of `p`.
fn tri(p: f32) -> f32 {
    let f = p - p.floor(); // fractional part in [0, 1)
    1.0 - (2.0 * f - 1.0).abs()
}
