//! Linux desktop camera capture via **GStreamer V4L2**.
//!
//! GStreamer is the native Linux media stack (the same engine the `audio`
//! SDK's Linux backend drives). Camera capture is a small hand-built pipeline
//!
//! ```text
//!   v4l2src device=<dev> ! videoconvert ! video/x-raw,format=RGBA ! appsink
//! ```
//!
//! - **`v4l2src`** reads raw frames from a Video4Linux2 device (`/dev/videoN`)
//!   — a UVC webcam, an internal laptop camera, a capture card. We don't build
//!   the element from a hand-parsed path: a [`gst::DeviceMonitor`] enumerates
//!   `Video/Source` devices GStreamer-natively and hands back a preconfigured
//!   `v4l2src` via [`Device::create_element`] with its `device` property
//!   already pointing at the right node.
//! - **`videoconvert`** converts the device's native pixel layout (commonly
//!   `YUY2`/`NV12`/`MJPG`-decoded) into the SDK's normalized `RGBA8`.
//! - **`appsink`** with `caps = video/x-raw,format=RGBA` pulls each decoded
//!   frame on a `new-sample` callback; we map it read-only and push it into
//!   the [`FrameWriter`] as tightly-packed top-down `RGBA8` — the exact same
//!   `MediaStream` frame model the Apple / Android / web backends publish.
//!
//! ## Frame delivery — CPU `RGBA8`, no zero-copy native source
//!
//! Unlike Apple (which publishes an `IOSurface`/`CMSampleBuffer` as the
//! stream's [`native_source`](crate::MediaStream::native_source) for a
//! zero-copy display fast-path), the V4L2 path exposes no native surface, so
//! `open` returns `None` for the native source and the ONLY delivery channel
//! is the CPU `RGBA8` writer. Consequently we **always** write frames (a
//! display consumer reads them via `latest()`), rather than gating on
//! [`FrameWriter::wants_cpu_frames`] the way Apple does — same posture as the
//! iOS-simulator synthetic backend, whose only channel is also the CPU writer.
//!
//! ## Async state changes + the bus thread (no GLib main loop assumed)
//!
//! GStreamer state changes are asynchronous and their outcomes (negotiation
//! done, errors) arrive as bus messages. This SDK may run outside GTK, so we
//! must NOT assume a GLib main loop is spinning. `open` blocks the pipeline to
//! `PLAYING` up front (so a bad resolution/fps request surfaces as
//! [`CameraError::UnsupportedConfig`] at open time, matching Apple), then a
//! dedicated bus thread owns the bus and drains ongoing errors until `Drop`
//! sets the pipeline to `Null` and joins it (RAII stop). This mirrors the
//! `audio` backend's [`bus_loop`] shape (`timed_pop(100ms)` + `quit` flag).
//!
//! ## Permissions
//!
//! Desktop-Linux V4L2 access is **filesystem-permission-based** — the user
//! must be able to read `/dev/videoN` (typically via the `video` group). There
//! is no OS prompt, so [`request_permission`] delegates to the shared
//! `permissions` SDK, which reports `Unsupported` on Linux (i.e. "nothing to
//! grant, usable"). An unreadable device surfaces later as a
//! [`CameraError::Backend`] from the failed pipeline start, never a panic.
//!
//! ## Follow-on: the PipeWire / xdg-desktop-portal path
//!
//! The modern desktop-portal camera path (`pipewiresrc` behind
//! `org.freedesktop.portal.Camera`) is the sandbox-friendly successor and
//! belongs here as a future source variant. It needs the `gst-plugin-pipewire`
//! GStreamer plugin, which is not assumed present; until then V4L2 is the
//! working path. Adding it slots in as an alternate source element behind the
//! same `build_pipeline` / `StreamHandle` contract — no public-API change.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};

use crate::{CameraConfig, CameraError, CameraFacing, NativeSource};
use media_stream::FrameWriter;

/// One-time, thread-safe, idempotent `gst::init()`. Stores the outcome so a
/// failed init (missing core libs) is reported honestly on every call. Copied
/// from the `audio` backend's `ensure_gst` (the GStreamer "media cluster"
/// pattern), retargeted to [`CameraError`].
fn ensure_gst() -> Result<(), CameraError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(CameraError::Backend)
}

/// Make a plain element by factory name; a missing element (plugin not
/// installed) is a `Backend` (environment) failure.
fn make(name: &str) -> Result<gst::Element, CameraError> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| CameraError::Backend(format!("missing GStreamer element `{name}`: {e}")))
}

// ---------------------------------------------------------------------------
// Enumeration.
// ---------------------------------------------------------------------------

/// A discovered V4L2 video source.
struct VideoDevice {
    /// Human-readable name from the device monitor (e.g. "Integrated Camera").
    #[allow(dead_code)] // Kept for diagnostics / future facing heuristics.
    name: String,
    /// The GStreamer `Device`. A preconfigured `v4l2src` — its `device`
    /// property already pointing at the right `/dev/videoN` — is made from it
    /// with `create_element(None)`, so we never hand-parse a device path.
    device: gst::Device,
}

/// Enumerate the host's V4L2 capture devices the GStreamer-native way: a
/// [`gst::DeviceMonitor`] filtered to `Video/Source`. An **empty list (no
/// camera attached) is a normal result, not an error** — the caller decides
/// (a `Default`/`Front`/`Back` open on an empty list is `NoCamera`).
///
/// The monitor is started synchronously, its current device list snapshotted,
/// then stopped — we don't hold a live monitor (hot-plug tracking is not part
/// of this SDK's surface).
fn enumerate_devices() -> Result<Vec<VideoDevice>, CameraError> {
    ensure_gst()?;

    let monitor = gst::DeviceMonitor::new();
    // Restrict to camera-like sources; GStreamer classifies V4L2 capture
    // devices as `Video/Source`. (A `null` caps filter = any caps.)
    let _ = monitor.add_filter(Some("Video/Source"), None);
    monitor
        .start()
        .map_err(|e| CameraError::Backend(format!("device monitor start failed: {e}")))?;

    let mut list = Vec::new();
    for device in monitor.devices() {
        list.push(VideoDevice {
            name: device.display_name().to_string(),
            device,
        });
    }
    monitor.stop();

    Ok(list)
}

/// Resolve a [`CameraFacing`] to a preconfigured `v4l2src` source element.
///
/// **V4L2 exposes no reliable front/back facing metadata** — "facing" is a
/// mobile-sensor concept, and a desktop UVC webcam reports none. So every
/// facing resolves to the primary (first-enumerated) video source: the honest
/// best effort on desktop Linux. This is a documented limitation, not a silent
/// wrong-camera open — an empty device list is `NoCamera` regardless of facing.
/// (When the PipeWire-portal path lands it can carry richer device metadata.)
fn source_for_facing(facing: CameraFacing) -> Result<gst::Element, CameraError> {
    let _ = facing; // No facing distinction available on V4L2 (see above).
    let devices = enumerate_devices()?;
    let Some(primary) = devices.first() else {
        return Err(CameraError::NoCamera);
    };
    primary
        .device
        .create_element(None)
        .map_err(|e| CameraError::Backend(format!("v4l2src creation failed: {e}")))
}

// ---------------------------------------------------------------------------
// Pipeline.
// ---------------------------------------------------------------------------

/// A constructed (not-yet-started) pipeline.
struct Built {
    pipeline: gst::Pipeline,
}

/// Assemble `<source> ! videoconvert ! appsink(caps=RGBA[,WxH][,fps])`, wiring
/// the appsink's `new-sample` callback to push mapped `RGBA8` frames into
/// `writer`. `source` is a `v4l2src` in production and a `videotestsrc` in the
/// synthetic frame test — the rest of the pipeline (and the whole map/deliver
/// path) is identical, which is the point: the test exercises the real appsink
/// code without a physical camera.
///
/// Requested `width`/`height`/`fps` are pinned as `appsink` caps fields, which
/// propagate upstream through negotiation; a device that can't produce that
/// mode fails the state change, surfacing as `UnsupportedConfig` (see
/// [`start`]). There is deliberately no `videoscale`/`videorate` — an explicit
/// request means "the device natively does this", matching the Apple backend's
/// `activeFormat` posture rather than silently rescaling.
fn build_pipeline(
    source: gst::Element,
    config: &CameraConfig,
    writer: FrameWriter,
) -> Result<Built, CameraError> {
    let pipeline = gst::Pipeline::default();
    let videoconvert = make("videoconvert")?;

    // Force RGBA output (videoconvert converts from the device's native
    // layout); pin resolution / framerate when the caller asked for them.
    let mut caps = gst::Caps::builder("video/x-raw").field("format", "RGBA");
    if let (Some(w), Some(h)) = (config.width, config.height) {
        caps = caps.field("width", w as i32).field("height", h as i32);
    }
    if let Some(fps) = config.fps {
        caps = caps.field("framerate", gst::Fraction::new(fps as i32, 1));
    }
    let appsink = AppSink::builder().caps(&caps.build()).build();

    // Deliver each decoded frame. `new-sample` runs on the streaming thread.
    let writer_cb = writer;
    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink
                    .pull_sample()
                    .map_err(|_| gst::FlowError::Eos)?;
                deliver_sample(&sample, &writer_cb);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline
        .add_many([&source, &videoconvert, appsink.upcast_ref::<gst::Element>()])
        .map_err(|e| CameraError::Backend(format!("pipeline add failed: {e}")))?;
    gst::Element::link_many([&source, &videoconvert, appsink.upcast_ref::<gst::Element>()])
        .map_err(|e| CameraError::Backend(format!("capture chain link failed: {e}")))?;

    Ok(Built { pipeline })
}

/// Map one `appsink` sample read-only and push it into the writer as
/// tightly-packed top-down `RGBA8`.
///
/// **Row-padding invariant:** GStreamer rounds a raw-video row stride up to a
/// multiple of 4 bytes; RGBA's 4-byte pixel makes `width * 4` already a
/// multiple of 4, so an RGBA buffer is tightly packed with NO row padding —
/// `buffer.len() == width * height * 4`. The fast path passes the mapped slice
/// straight through. We still keep a defensive repack for the (not-expected)
/// padded case so a nonstandard `videoconvert` can never feed misaligned rows
/// into the frame channel.
fn deliver_sample(sample: &gst::Sample, writer: &FrameWriter) {
    let Some(buffer) = sample.buffer() else { return };
    let Some(caps) = sample.caps() else { return };
    let Some(s) = caps.structure(0) else { return };
    let (Ok(width), Ok(height)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
        return;
    };
    if width <= 0 || height <= 0 {
        return;
    }
    let Ok(map) = buffer.map_readable() else { return };
    let (w, h) = (width as u32, height as u32);
    let expected = w as usize * h as usize * 4;
    let data = map.as_slice();

    if data.len() == expected {
        // Tightly packed (the RGBA invariant) — the common and only real path.
        writer.write_rgba8(w, h, data);
    } else if data.len() > expected {
        // Defensive: padded rows. Repack row-by-row into a tight RGBA buffer.
        let row = w as usize * 4;
        let stride = data.len() / h as usize;
        if stride >= row {
            let mut tight = Vec::with_capacity(expected);
            for y in 0..h as usize {
                let start = y * stride;
                tight.extend_from_slice(&data[start..start + row]);
            }
            writer.write_rgba8(w, h, &tight);
        }
    }
    // A short buffer (< expected) is dropped rather than delivering a partial
    // frame — `write_rgba8` would reject it anyway.
}

/// Map a GStreamer error to a [`CameraError`]. A `ResourceError` (device busy /
/// not found / unreadable `/dev/videoN`) is always a `Backend` environment
/// failure. Otherwise, if the caller pinned an explicit resolution/fps
/// (`wants_config`), a negotiation/stream failure means the device can't do
/// that mode → `UnsupportedConfig`; with no explicit request it's `Backend`.
fn classify_error(err: &glib::Error, detail: String, wants_config: bool) -> CameraError {
    if err.kind::<gst::ResourceError>().is_some() {
        CameraError::Backend(detail)
    } else if wants_config {
        CameraError::UnsupportedConfig(detail)
    } else {
        CameraError::Backend(detail)
    }
}

/// Pop the first `Error` on the pipeline's bus and classify it; falls back to a
/// generic failure when the state change left no detail.
fn drain_error(pipeline: &gst::Pipeline, wants_config: bool) -> CameraError {
    if let Some(bus) = pipeline.bus() {
        while let Some(msg) =
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(200), &[gst::MessageType::Error])
        {
            if let gst::MessageView::Error(e) = msg.view() {
                let detail = match e.debug() {
                    Some(d) if !d.is_empty() => format!("{} ({d})", e.error()),
                    _ => e.error().to_string(),
                };
                return classify_error(&e.error(), detail, wants_config);
            }
        }
    }
    let msg = "GStreamer pipeline failed to reach PLAYING (no error detail)".to_string();
    if wants_config {
        CameraError::UnsupportedConfig(msg)
    } else {
        CameraError::Backend(msg)
    }
}

/// The bus loop on the live capture handle's dedicated thread. Live capture has
/// no EOS to handle; it just drains ongoing errors (never panicking) and exits
/// when `quit` is set. Copied in shape from the `audio` backend's `bus_loop`.
fn bus_loop(bus: gst::Bus, quit: Arc<AtomicBool>) {
    loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        if let gst::MessageView::Error(e) = msg.view() {
            eprintln!(
                "camera(linux): capture error: {} ({:?})",
                e.error(),
                e.debug()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Stream handle. Owns the pipeline + bus thread; `Drop` stops capture (state →
// `Null`) and joins the thread — RAII stop, the same shape as the `audio`
// backend's `PlaybackHandle`.
// ---------------------------------------------------------------------------

pub(crate) struct StreamHandle {
    pipeline: gst::Pipeline,
    quit: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Signal the bus thread first, then tear the pipeline down, then join.
        self.quit.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(thread) = self.bus_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Drive a built pipeline to `PLAYING`, blocking until it either negotiates and
/// runs or fails — so a bad resolution/fps request is reported at open time
/// (matching Apple's synchronous `activeFormat` validation), not swallowed.
/// On success spawns the bus thread and returns the RAII handle.
fn start(built: Built, config: &CameraConfig) -> Result<StreamHandle, CameraError> {
    let pipeline = built.pipeline;
    let wants_config = config.width.is_some() || config.height.is_some() || config.fps.is_some();

    let bus = pipeline
        .bus()
        .ok_or_else(|| CameraError::Backend("pipeline has no bus".into()))?;

    if pipeline.set_state(gst::State::Playing).is_err() {
        let err = drain_error(&pipeline, wants_config);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    // Block up to 5 s for the async state change to reach PLAYING (a live V4L2
    // source prerolls once it opens the device + negotiates a format). A
    // failure here — device unreadable, or the requested caps unsupported —
    // surfaces now as a typed error rather than a silently dead stream.
    let (result, _current, _pending) = pipeline.state(gst::ClockTime::from_seconds(5));
    if result.is_err() {
        let err = drain_error(&pipeline, wants_config);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    let quit = Arc::new(AtomicBool::new(false));
    let bus_thread = {
        let quit = quit.clone();
        std::thread::spawn(move || bus_loop(bus, quit))
    };

    Ok(StreamHandle {
        pipeline,
        quit,
        bus_thread: Some(bus_thread),
    })
}

/// Build + start a capture pipeline from an already-resolved source element.
/// Shared by production [`open`] (a `v4l2src`) and the synthetic frame test (a
/// `videotestsrc`), so the test drives the real build/map/deliver path.
fn open_source(
    source: gst::Element,
    config: &CameraConfig,
    writer: FrameWriter,
) -> Result<StreamHandle, CameraError> {
    let built = build_pipeline(source, config, writer)?;
    start(built, config)
}

// ---------------------------------------------------------------------------
// Public backend contract (mirrors apple.rs / android.rs / sim_camera.rs).
// ---------------------------------------------------------------------------

/// Desktop-Linux camera access is filesystem-permission-based (readable
/// `/dev/videoN`), with no OS prompt. Delegated to the shared `permissions`
/// SDK, which reports `Unsupported` on Linux — i.e. "usable, nothing to grant"
/// — exactly as the Apple backend consults it. A genuinely unreadable device
/// surfaces later as a `Backend` error from the failed pipeline start.
pub(crate) async fn request_permission() -> Result<(), CameraError> {
    let status = permissions::request(permissions::Permission::Camera).await;
    if status.is_usable() {
        Ok(())
    } else {
        Err(CameraError::PermissionDenied)
    }
}

/// Open the primary V4L2 camera and start capture. Returns the RAII
/// [`StreamHandle`] and `None` for the native source (no zero-copy surface on
/// the V4L2 path — see the module note). Frames flow into `writer` as
/// tightly-packed `RGBA8` until the handle drops.
pub(crate) async fn open(
    config: CameraConfig,
    writer: FrameWriter,
) -> Result<(StreamHandle, Option<NativeSource>), CameraError> {
    request_permission().await?;
    ensure_gst()?;

    let source = source_for_facing(config.facing)?;
    let handle = open_source(source, &config, writer)?;
    Ok((handle, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use media_stream::{MediaStream, VideoFrame};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Enumeration must never panic and must return `Ok` even with **no camera
    /// attached** (an empty list is a normal result). This is the headless CI
    /// guard — it runs the real `GstDeviceMonitor` `Video/Source` path. On a
    /// host with no `/dev/videoN` the list is simply empty.
    #[test]
    fn enumerate_never_panics_without_camera() {
        let devices = enumerate_devices().expect("enumeration must return Ok, not error");
        // Empty is valid (no camera). Non-empty is valid too (a dev box with a
        // webcam). Either way we must not have panicked getting here.
        let _ = devices.len();
    }

    /// **The core proof**: the real appsink map/deliver path produces a mapped,
    /// correctly-shaped `RGBA8` frame — exercised WITHOUT a physical camera by
    /// substituting `videotestsrc` for `v4l2src`. Everything downstream
    /// (`videoconvert ! appsink(RGBA) ! new-sample ! deliver_sample !
    /// write_rgba8`) is byte-for-byte the production path. If this delivers a
    /// `64x48` `Rgba8` frame whose `data.len() == width*height*4`, the capture
    /// backend genuinely works; it fails against the old `Unsupported` stub
    /// (which never wired a pipeline at all).
    #[test]
    fn videotestsrc_delivers_mapped_rgba_frame() {
        ensure_gst().expect("gst init");

        let (stream, writer) = MediaStream::new();
        let got: Arc<Mutex<Option<(u32, u32, usize)>>> = Arc::new(Mutex::new(None));
        let sub = {
            let got = got.clone();
            stream.subscribe(move |f: &VideoFrame| {
                if f.format == PixelFormat::Rgba8 && f.data.len() == f.byte_len() {
                    *got.lock().unwrap() = Some((f.width, f.height, f.data.len()));
                }
            })
        };

        let source = make("videotestsrc").expect("videotestsrc element");
        let config = CameraConfig::default().with_resolution(64, 48);
        let handle = open_source(source, &config, writer).expect("synthetic pipeline must start");

        // Wait (polling) for the first frame to be mapped and fanned out.
        let deadline = Instant::now() + Duration::from_secs(5);
        let observed = loop {
            if let Some(v) = *got.lock().unwrap() {
                break Some(v);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        drop(sub);
        drop(handle); // RAII stop must not hang or panic.

        let (w, h, len) = observed.expect("a synthetic RGBA frame must be delivered");
        assert_eq!((w, h), (64, 48), "delivered frame carries the negotiated size");
        assert_eq!(len, 64 * 48 * 4, "delivered frame is tightly-packed RGBA8");
    }

    /// The error-classifier split must be honest: a `ResourceError` (device
    /// busy / unreadable) is always `Backend`; a negotiation/stream failure
    /// after an explicit config request is `UnsupportedConfig`; the same
    /// failure with no explicit request is `Backend`.
    #[test]
    fn error_classification_is_honest() {
        let resource = glib::Error::new(gst::ResourceError::OpenRead, "cannot open device");
        assert!(
            matches!(
                classify_error(&resource, "x".into(), true),
                CameraError::Backend(_)
            ),
            "ResourceError is always Backend, even with a config request"
        );

        let nego = glib::Error::new(gst::CoreError::Negotiation, "no common format");
        assert!(
            matches!(
                classify_error(&nego, "x".into(), true),
                CameraError::UnsupportedConfig(_)
            ),
            "a negotiation failure with an explicit config is UnsupportedConfig"
        );
        assert!(
            matches!(
                classify_error(&nego, "x".into(), false),
                CameraError::Backend(_)
            ),
            "a negotiation failure with no config request is Backend"
        );
    }

    /// Live capture against a real `/dev/videoN`. Ignored by default (CI has no
    /// camera); run with `--ignored` on a machine with a webcam and read-access
    /// to the device. Proves the full `v4l2src` open + real-frame path.
    #[test]
    #[ignore = "requires a real V4L2 camera device (/dev/videoN) with read access"]
    fn live_v4l2_delivers_a_real_frame() {
        ensure_gst().expect("gst init");

        let (stream, writer) = MediaStream::new();
        let got = Arc::new(Mutex::new(false));
        let sub = {
            let got = got.clone();
            stream.subscribe(move |f: &VideoFrame| {
                if f.data.len() == f.byte_len() {
                    *got.lock().unwrap() = true;
                }
            })
        };

        let source = source_for_facing(CameraFacing::Default)
            .expect("a real camera must enumerate on a host with one");
        let handle = open_source(source, &CameraConfig::default(), writer)
            .expect("real camera pipeline must start");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !*got.lock().unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let arrived = *got.lock().unwrap();

        drop(sub);
        drop(handle);
        assert!(arrived, "a real camera must deliver at least one RGBA frame");
    }
}
