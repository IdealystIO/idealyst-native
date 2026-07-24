//! Linux (desktop) video/audio recording via **GStreamer** — the native Linux
//! media stack, the same engine the `audio` playback backend and GTK's
//! `GtkMediaFile` sit on. It is the writer peer of the "GStreamer media
//! cluster" the `audio` backend seeded (see its module docs): this file reuses
//! that backend's idempotent [`ensure_gst`] init and its honest
//! [`map_gst_error`] domain split verbatim.
//!
//! # Pipeline
//!
//! One hand-built encode pipeline. Video-only:
//!
//! ```text
//!   appsrc ! videoconvert ! vp8enc ! webmmux ! filesink location=<path>
//! ```
//!
//! With audio a second branch feeds the *same* muxer, so both media land in one
//! lip-synced file:
//!
//! ```text
//!   appsrc ! audioconvert ! audioresample ! opusenc ! webmmux
//! ```
//!
//! The caller's RGBA8 [`VideoFrame`]s and interleaved-`f32` [`AudioFrame`]s are
//! pushed into the two [`AppSrc`]s. `videoconvert` turns RGBA into the encoder's
//! planar format; `webmmux` muxes VP8 + Opus into WebM; `filesink` writes it.
//!
//! # Codec selection — why VP8/WebM is the default
//!
//! A *base* GStreamer install always ships VP8 (`vp8enc`), `webmmux` and Opus
//! (`opusenc`) — they're royalty-free — but **not** an H.264 encoder: `x264enc`
//! lives in `gstreamer1.0-plugins-ugly` and `openh264enc` in `-plugins-bad`,
//! both optional. So:
//!
//! - [`Container::WebM`] → `vp8enc`, `webmmux` and `opusenc`. Always available;
//!   the recommended Linux container.
//! - [`Container::Mp4`] → an H.264 encoder (`x264enc`/`openh264enc`), `mp4mux`
//!   and AAC, honored **only when an H.264 encoder plugin is installed**. When
//!   none is present we return an honest [`MediaWriterError::Backend`] naming
//!   the missing plugin — never a silently-broken file. (See [`select_codec`].)
//!
//! # Threading
//!
//! Video and audio taps fire on *different* producer threads. Rather than share
//! the pipeline across them, both taps copy each frame/chunk into a plain
//! `Send` message and push it down an `mpsc` channel; a single owned
//! [`encoder_thread`] is the sole toucher of the pipeline and the two `AppSrc`s.
//! Unlike the `audio` playback backend — which spawns a *separate* bus thread
//! because playback runs unattended after the API call returns — the recorder
//! has one natural driver (the encoder thread), so it also owns the bus and
//! drains it inline at finalize. `stop()` awaits the finalize result through the
//! parking [`crate::oneshot::sync_oneshot`] (same park-don't-block discipline as
//! the Apple backend) so it never freezes the caller's executor.
//!
//! # Invariants
//!
//! - **appsrc caps are set from the FIRST frame.** The stream carries no
//!   dimensions; we learn width/height from the first [`VideoFrame`] and set the
//!   `appsrc` caps once, before its first `push_buffer`, or the encoder can't
//!   negotiate a format.
//! - **PTS is stamped from `pts_micros`, relative to a shared epoch.** Both
//!   branches subtract the same epoch (the first sample of either kind) so the
//!   two timelines start at 0 and stay aligned for lip-sync; each pad is
//!   independently monotonic because the source clocks are.
//! - **EOS-to-finalize.** On `stop()` we send end-of-stream to every `appsrc`
//!   and WAIT for the pipeline's EOS on the bus before going to `Null`. Going to
//!   `Null` first would truncate the file mid-write (missing Matroska cues /
//!   cluster index) → a corrupt, sometimes-undecodable output. This wait is the
//!   whole reason `stop()` is async.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;

use crate::{Container, MediaInputs, MediaWriterError, RecordConfig};
use media_stream::{AudioFrame, AudioSubscription, Subscription, VideoFrame};

/// Max video frames queued but not yet encoded before the producer drops new
/// ones. A live render thread outruns the software VP8 encoder; an unbounded
/// channel would back up gigabytes of full-res RGBA and make `stop()` spend
/// seconds draining the backlog before it can finalize. Real-time policy: drop
/// at the source when the encoder is behind so the queue — and finalize latency
/// — stay tiny. Mirrors the Apple backend's `MAX_INFLIGHT_VIDEO_FRAMES`.
const MAX_INFLIGHT_VIDEO_FRAMES: usize = 4;

// ---------------------------------------------------------------------------
// Reused verbatim from the `audio` GStreamer backend (the cluster's shared
// pieces). See `crates/sdk/client/audio/src/linux.rs`.
// ---------------------------------------------------------------------------

/// One-time, thread-safe, idempotent `gst::init()`. Stores the outcome so a
/// failed init (missing core libs) is reported honestly on every call.
fn ensure_gst() -> Result<(), MediaWriterError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(MediaWriterError::Backend)
}

/// Split a GStreamer error into a [`MediaWriterError`]. `media-writer` has no
/// dedicated decode variant (it's an encoder, not a player), so both a
/// `StreamError` (the encoder/mux couldn't process a sample) and a
/// resource/core error surface as [`MediaWriterError::Backend`]; we label the
/// encode case so the message stays honest about which layer failed.
fn map_gst_error(err: &glib::Error, debug: Option<&str>) -> MediaWriterError {
    let detail = match debug {
        Some(d) if !d.is_empty() => format!("{err} ({d})"),
        _ => err.to_string(),
    };
    if err.kind::<gst::StreamError>().is_some() {
        MediaWriterError::Backend(format!("GStreamer encode/stream error: {detail}"))
    } else {
        MediaWriterError::Backend(format!("GStreamer pipeline error: {detail}"))
    }
}

/// Pop the first `Error` currently on the pipeline's bus and map it; falls back
/// to a generic `Backend` if the failure left no detail. Used when a state
/// change fails (it posts its ERROR before returning).
fn drain_error(pipeline: &gst::Pipeline) -> MediaWriterError {
    if let Some(bus) = pipeline.bus() {
        while let Some(msg) =
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(200), &[gst::MessageType::Error])
        {
            if let gst::MessageView::Error(e) = msg.view() {
                return map_gst_error(&e.error(), e.debug().as_deref());
            }
        }
    }
    MediaWriterError::Backend("GStreamer pipeline failed to start (no error detail)".into())
}

// ---------------------------------------------------------------------------
// Codec selection.
// ---------------------------------------------------------------------------

/// The encoder/muxer factory names for a chosen container.
struct Codec {
    video_enc: &'static str,
    audio_enc: &'static str,
    muxer: &'static str,
    /// True for the royalty-free VP8/Opus path, whose encoder properties (VP8
    /// `deadline`/`target-bitrate`, Opus `bitrate`) we know and can set safely.
    is_webm: bool,
}

/// Whether a GStreamer element factory is installed.
fn plugin_present(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// The first installed factory among `names`, if any.
fn first_available(names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|n| plugin_present(n))
}

/// Map the config's [`Container`] to concrete GStreamer factories, honoring an
/// explicit request when its encoder plugin exists and returning an honest
/// error (rather than a broken file) when it doesn't.
fn select_codec(container: Container) -> Result<Codec, MediaWriterError> {
    match container {
        // Royalty-free VP8 + Opus in WebM — always present in a base GStreamer.
        Container::WebM => {
            if !plugin_present("vp8enc") || !plugin_present("webmmux") {
                return Err(MediaWriterError::Backend(
                    "VP8/WebM encoder plugins missing (need `vp8enc` + `webmmux` from \
                     gstreamer1.0-plugins-good)"
                        .into(),
                ));
            }
            Ok(Codec {
                video_enc: "vp8enc",
                audio_enc: "opusenc",
                muxer: "webmmux",
                is_webm: true,
            })
        }
        // H.264 + AAC in MP4. H.264 *encoding* needs an optional plugin; if none
        // is installed we refuse honestly and point at `Container::WebM`.
        Container::Mp4 => {
            let video_enc = first_available(&["x264enc", "openh264enc"]).ok_or_else(|| {
                MediaWriterError::Backend(
                    "no H.264 video encoder plugin installed (need `x264enc` from \
                     gstreamer1.0-plugins-ugly or `openh264enc` from -plugins-bad). \
                     Record with `Container::WebM` for VP8/WebM, which a base GStreamer \
                     always provides."
                        .into(),
                )
            })?;
            if !plugin_present("mp4mux") {
                return Err(MediaWriterError::Backend(
                    "`mp4mux` muxer missing (need gstreamer1.0-plugins-good)".into(),
                ));
            }
            // AAC encoder name varies by distro packaging; pick whatever exists.
            let audio_enc =
                first_available(&["avenc_aac", "faac", "voaacenc"]).unwrap_or("avenc_aac");
            Ok(Codec {
                video_enc,
                audio_enc,
                muxer: "mp4mux",
                is_webm: false,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline construction.
// ---------------------------------------------------------------------------

/// A constructed (not-yet-finalized) encode pipeline plus the `AppSrc`s the
/// encoder thread pushes into.
struct Pipeline {
    pipeline: gst::Pipeline,
    video_src: Option<AppSrc>,
    audio_src: Option<AppSrc>,
    fps: u32,
}

/// Make a plain element by factory name; a missing element is `Backend`.
fn make(name: &str) -> Result<gst::Element, MediaWriterError> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| MediaWriterError::Backend(format!("missing GStreamer element `{name}`: {e}")))
}

/// Build an `AppSrc` in time-format, live, non-blocking mode. `block=false` so a
/// full internal queue never stalls the capture thread (we drop upstream via the
/// in-flight cap instead); `do-timestamp=false` so the encoder uses the PTS WE
/// stamp from `pts_micros` rather than wall-clock-at-push.
fn build_appsrc() -> AppSrc {
    let src = AppSrc::builder()
        .format(gst::Format::Time)
        .is_live(true)
        .build();
    src.set_property("block", false);
    src.set_property("do-timestamp", false);
    src
}

#[allow(clippy::too_many_arguments)]
fn build_pipeline(
    codec: &Codec,
    path: &Path,
    has_video: bool,
    has_audio: bool,
    fps: u32,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
) -> Result<Pipeline, MediaWriterError> {
    let pipeline = gst::Pipeline::default();

    let muxer = make(codec.muxer)?;
    let filesink = make("filesink")?;
    filesink.set_property("location", path.to_string_lossy().as_ref());
    pipeline
        .add_many([&muxer, &filesink])
        .map_err(|e| MediaWriterError::Backend(format!("pipeline add failed: {e}")))?;
    gst::Element::link_many([&muxer, &filesink])
        .map_err(|e| MediaWriterError::Backend(format!("muxer→filesink link failed: {e}")))?;

    let video_src = if has_video {
        let src = build_appsrc();
        let convert = make("videoconvert")?;
        let venc = make(codec.video_enc)?;
        if codec.is_webm {
            // VP8 `deadline=1` = real-time; the default 0 ("best quality") is
            // effectively unbounded CPU and stalls a live recording. Only the
            // VP8 path sets it — `is_webm` proves the factory is `vp8enc`.
            venc.set_property("deadline", 1i64);
            if let Some(bps) = video_bitrate {
                // vp8enc `target-bitrate` is bits/second (gint).
                venc.set_property("target-bitrate", bps as i32);
            }
        }
        pipeline
            .add_many([src.upcast_ref::<gst::Element>(), &convert, &venc])
            .map_err(|e| MediaWriterError::Backend(format!("video add failed: {e}")))?;
        gst::Element::link_many([src.upcast_ref::<gst::Element>(), &convert, &venc])
            .map_err(|e| MediaWriterError::Backend(format!("video chain link failed: {e}")))?;
        // Muxers expose request pads; `link` requests a compatible one for us.
        venc.link(&muxer)
            .map_err(|e| MediaWriterError::Backend(format!("video→muxer link failed: {e}")))?;
        Some(src)
    } else {
        None
    };

    let audio_src = if has_audio {
        let src = build_appsrc();
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let aenc = make(codec.audio_enc)?;
        if codec.is_webm {
            if let Some(bps) = audio_bitrate {
                // opusenc `bitrate` is bits/second (gint).
                aenc.set_property("bitrate", bps as i32);
            }
        }
        pipeline
            .add_many([src.upcast_ref::<gst::Element>(), &convert, &resample, &aenc])
            .map_err(|e| MediaWriterError::Backend(format!("audio add failed: {e}")))?;
        gst::Element::link_many([src.upcast_ref::<gst::Element>(), &convert, &resample, &aenc])
            .map_err(|e| MediaWriterError::Backend(format!("audio chain link failed: {e}")))?;
        aenc.link(&muxer)
            .map_err(|e| MediaWriterError::Backend(format!("audio→muxer link failed: {e}")))?;
        Some(src)
    } else {
        None
    };

    Ok(Pipeline {
        pipeline,
        video_src,
        audio_src,
        fps: fps.max(1),
    })
}

// ---------------------------------------------------------------------------
// Cross-thread messages: plain `Send` data, never a GStreamer handle from a tap.
// ---------------------------------------------------------------------------

enum Msg {
    Video {
        width: u32,
        height: u32,
        pts_us: u64,
        rgba: Vec<u8>,
    },
    Audio {
        sample_rate: u32,
        channels: u16,
        pts_us: u64,
        samples: Vec<f32>,
    },
    /// Finalize the file and exit; the encoder replies through a thread-safe,
    /// awaitable oneshot (parked, never a blocking recv — see `stop`).
    Stop(crate::oneshot::SyncTx<Result<(), MediaWriterError>>),
}

// ---------------------------------------------------------------------------
// Encoder — the sole owner of the pipeline + AppSrcs, on the encoder thread.
// ---------------------------------------------------------------------------

struct Encoder {
    pipe: Pipeline,
    path: PathBuf,
    /// Shared micros epoch (first sample of either kind); PTS is measured from it.
    epoch_us: Option<u64>,
    video_caps_set: bool,
    audio_caps_set: bool,
    pushed_any: bool,
    failed: Option<String>,
}

impl Encoder {
    fn new(pipe: Pipeline, path: PathBuf) -> Self {
        Self {
            pipe,
            path,
            epoch_us: None,
            video_caps_set: false,
            audio_caps_set: false,
            pushed_any: false,
            failed: None,
        }
    }

    /// PTS for a shared-clock `pts_micros`, relative to the shared epoch (set on
    /// the first sample). `saturating_sub` keeps it non-negative if a slightly
    /// earlier timestamp sneaks in after the epoch was fixed.
    fn pts(&mut self, pts_us: u64) -> gst::ClockTime {
        let epoch = *self.epoch_us.get_or_insert(pts_us);
        gst::ClockTime::from_useconds(pts_us.saturating_sub(epoch))
    }

    fn on_video(&mut self, width: u32, height: u32, pts_us: u64, mut rgba: Vec<u8>) {
        if self.failed.is_some() {
            return;
        }
        let Some(src) = self.pipe.video_src.clone() else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        let need = width as usize * height as usize * 4;
        if rgba.len() < need {
            return;
        }
        // Trim any tail so the buffer size matches the caps exactly.
        rgba.truncate(need);

        if !self.video_caps_set {
            // Caps MUST be set before the first buffer so downstream can
            // negotiate. Incoming frames are tightly-packed top-down RGBA8.
            let caps = gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .field("width", width as i32)
                .field("height", height as i32)
                .field("framerate", gst::Fraction::new(self.pipe.fps as i32, 1))
                .build();
            src.set_caps(Some(&caps));
            self.video_caps_set = true;
        }

        let pts = self.pts(pts_us);
        let mut buffer = gst::Buffer::from_mut_slice(rgba);
        {
            let b = buffer.get_mut().expect("fresh buffer is uniquely owned");
            b.set_pts(pts);
            b.set_duration(gst::ClockTime::from_nseconds(
                1_000_000_000 / self.pipe.fps as u64,
            ));
        }
        self.push(&src, buffer, "video");
    }

    fn on_audio(&mut self, sample_rate: u32, channels: u16, pts_us: u64, samples: Vec<f32>) {
        if self.failed.is_some() {
            return;
        }
        let Some(src) = self.pipe.audio_src.clone() else {
            return;
        };
        if sample_rate == 0 || channels == 0 || samples.is_empty() {
            return;
        }

        if !self.audio_caps_set {
            // Interleaved little-endian f32 PCM, matching `AudioFrame`'s layout.
            let caps = gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("rate", sample_rate as i32)
                .field("channels", channels as i32)
                .field("layout", "interleaved")
                .build();
            src.set_caps(Some(&caps));
            self.audio_caps_set = true;
        }

        let frame_count = samples.len() / channels as usize;
        let pts = self.pts(pts_us);
        // f32 samples → F32LE bytes.
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in &samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let mut buffer = gst::Buffer::from_mut_slice(bytes);
        {
            let b = buffer.get_mut().expect("fresh buffer is uniquely owned");
            b.set_pts(pts);
            b.set_duration(gst::ClockTime::from_nseconds(
                frame_count as u64 * 1_000_000_000 / sample_rate as u64,
            ));
        }
        self.push(&src, buffer, "audio");
    }

    /// Push a buffer, recording only genuine flow errors (a `Flushing`/`Eos`
    /// return is benign shutdown, not a failure).
    fn push(&mut self, src: &AppSrc, buffer: gst::Buffer, which: &str) {
        match src.push_buffer(buffer) {
            Ok(_) => self.pushed_any = true,
            Err(gst::FlowError::Flushing) | Err(gst::FlowError::Eos) => {}
            Err(e) => self.failed = Some(format!("{which} push_buffer failed: {e:?}")),
        }
    }

    /// Send EOS to every appsrc, wait for the pipeline's EOS (the muxer has
    /// flushed its trailer and the file is complete) on the bus, then go to
    /// `Null`. A bounded wait means a wedged encoder can never hang `stop()`.
    fn finish(&mut self) -> Result<(), MediaWriterError> {
        if let Some(e) = self.failed.take() {
            self.abort();
            return Err(MediaWriterError::Backend(e));
        }
        if !self.pushed_any {
            self.abort();
            return Err(MediaWriterError::Backend(
                "no media captured before stop".into(),
            ));
        }

        if let Some(src) = &self.pipe.video_src {
            let _ = src.end_of_stream();
        }
        if let Some(src) = &self.pipe.audio_src {
            let _ = src.end_of_stream();
        }

        let bus = match self.pipe.pipeline.bus() {
            Some(b) => b,
            None => {
                let _ = self.pipe.pipeline.set_state(gst::State::Null);
                return Err(MediaWriterError::Backend("pipeline has no bus".into()));
            }
        };

        let mut result =
            Err(MediaWriterError::Backend("finalize timed out waiting for EOS".into()));
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let Some(msg) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) else {
                continue;
            };
            match msg.view() {
                gst::MessageView::Eos(_) => {
                    result = Ok(());
                    break;
                }
                gst::MessageView::Error(e) => {
                    result = Err(map_gst_error(&e.error(), e.debug().as_deref()));
                    break;
                }
                _ => {}
            }
        }

        let _ = self.pipe.pipeline.set_state(gst::State::Null);
        if result.is_err() {
            // A failed finalize leaves a truncated file; don't hand back garbage.
            let _ = std::fs::remove_file(&self.path);
        }
        result
    }

    /// Tear the pipeline down and discard the partial file (dropped without
    /// `stop()`, or a pre-finalize failure).
    fn abort(&mut self) {
        let _ = self.pipe.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The encoder thread body: the only toucher of the pipeline. Blocks on the
/// channel, pushing each frame/chunk; finalizes on `Stop`; aborts if the handle
/// was dropped without stopping (channel disconnected).
fn encoder_thread(rx: Receiver<Msg>, pipe: Pipeline, path: PathBuf, inflight: Arc<AtomicUsize>) {
    let mut enc = Encoder::new(pipe, path);
    loop {
        match rx.recv() {
            Ok(Msg::Video {
                width,
                height,
                pts_us,
                rgba,
            }) => {
                // Release the reservation as soon as we own the frame.
                inflight.fetch_sub(1, Ordering::AcqRel);
                enc.on_video(width, height, pts_us, rgba);
            }
            Ok(Msg::Audio {
                sample_rate,
                channels,
                pts_us,
                samples,
            }) => enc.on_audio(sample_rate, channels, pts_us, samples),
            Ok(Msg::Stop(reply)) => {
                reply.send(enc.finish());
                return;
            }
            Err(_) => {
                enc.abort();
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handle returned to the public API.
// ---------------------------------------------------------------------------

pub(crate) struct RecordingHandle {
    /// `Option` so `stop()`/`Drop` can drop OUR sender before joining — the
    /// encoder thread's `recv()` only disconnects once every sender (ours + the
    /// taps' clones) is gone. Dropping the taps + this releases them all.
    tx: Option<Sender<Msg>>,
    join: Option<JoinHandle<()>>,
    _video_sub: Option<Subscription>,
    _audio_sub: Option<AudioSubscription>,
}

impl RecordingHandle {
    pub(crate) async fn stop(mut self) -> Result<(), MediaWriterError> {
        // Stop the taps first so no further samples enqueue.
        self._video_sub = None;
        self._audio_sub = None;

        let tx = self
            .tx
            .take()
            .ok_or_else(|| MediaWriterError::Backend("recording already stopped".into()))?;

        // AWAIT the finalize result — never block on it. `stop` may run on a
        // single-threaded executor; a synchronous recv here would freeze it. The
        // parking `SyncRx` yields until the encoder thread replies.
        let (done_tx, done_rx) = crate::oneshot::sync_oneshot();
        tx.send(Msg::Stop(done_tx))
            .map_err(|_| MediaWriterError::Backend("encoder thread gone".into()))?;
        drop(tx); // release our sender now that Stop is queued

        let result = done_rx
            .await
            .ok_or_else(|| MediaWriterError::Backend("encoder thread dropped before finishing".into()))?;

        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        // Dropped without `stop()`: release the taps' sender clones and ours so
        // the encoder thread's `recv()` disconnects and it aborts (discards the
        // partial file), then join. No-op if `stop()` already ran.
        self._video_sub = None;
        self._audio_sub = None;
        self.tx = None;
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

// ---------------------------------------------------------------------------
// start(): resolve path, select codec, build + start the pipeline, wire taps.
// ---------------------------------------------------------------------------

pub(crate) async fn start(
    inputs: MediaInputs<'_>,
    config: &RecordConfig,
) -> Result<RecordingHandle, MediaWriterError> {
    let path = config
        .store
        .local_path(&config.path)
        .ok_or(MediaWriterError::NoLocalPath)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&path);

    ensure_gst()?;

    // Eager codec check: an unsatisfiable H.264/MP4 request fails HERE (at
    // `record()`), not silently at stop with a broken file.
    let codec = select_codec(config.container)?;

    let has_video = inputs.video.is_some();
    let has_audio = inputs.audio.is_some();
    let pipe = build_pipeline(
        &codec,
        &path,
        has_video,
        has_audio,
        config.fps,
        config.video_bitrate,
        config.audio_bitrate,
    )?;

    // Go live. appsrc caps are set lazily from the first frame (we learn W×H
    // from it), so PLAYING here just readies the pipeline to accept buffers.
    if pipe.pipeline.set_state(gst::State::Playing).is_err() {
        let err = drain_error(&pipe.pipeline);
        let _ = pipe.pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    let inflight = Arc::new(AtomicUsize::new(0));
    let join = {
        let inflight = inflight.clone();
        let path = path.clone();
        std::thread::Builder::new()
            .name("media-writer".into())
            .spawn(move || encoder_thread(rx, pipe, path, inflight))
            .map_err(|e| MediaWriterError::Backend(format!("spawn encoder thread: {e}")))?
    };

    let video_sub = inputs.video.map(|stream| {
        let tx = tx.clone();
        let inflight = inflight.clone();
        stream.subscribe(move |f: &VideoFrame| {
            // Real-time drop: if the encoder is already `MAX_INFLIGHT_VIDEO_FRAMES`
            // behind, skip this frame rather than grow the backlog `stop()` must
            // later drain.
            if inflight.load(Ordering::Acquire) >= MAX_INFLIGHT_VIDEO_FRAMES {
                return;
            }
            inflight.fetch_add(1, Ordering::AcqRel);
            if tx
                .send(Msg::Video {
                    width: f.width,
                    height: f.height,
                    pts_us: f.pts_micros,
                    rgba: f.data.to_vec(),
                })
                .is_err()
            {
                inflight.fetch_sub(1, Ordering::AcqRel);
            }
        })
    });

    let audio_sub = inputs.audio.map(|stream| {
        let tx = tx.clone();
        stream.subscribe(move |f: &AudioFrame| {
            let _ = tx.send(Msg::Audio {
                sample_rate: f.sample_rate,
                channels: f.channels,
                pts_us: f.pts_micros,
                samples: f.samples.to_vec(),
            });
        })
    });

    Ok(RecordingHandle {
        tx: Some(tx),
        join: Some(join),
        _video_sub: video_sub,
        _audio_sub: audio_sub,
    })
}

// ---------------------------------------------------------------------------
// Tests — fully headless (no capture device needed to encode).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid-color tightly-packed RGBA8 frame, so the encoder has real pixels.
    fn rgba_frame(width: u32, height: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            v.extend_from_slice(&[0x33, 0x88, 0xcc, 0xff]);
        }
        v
    }

    /// Drive `filesrc ! decodebin ! fakesink` to EOS to prove the file is a
    /// valid, decodable container (round-trip). One `fakesink` per decoded pad
    /// so an audio+video file decodes both streams. Panics with the GStreamer
    /// error if the file is corrupt / undecodable.
    fn assert_decodable(path: &Path) {
        let pipeline = gst::Pipeline::default();
        let src = gst::ElementFactory::make("filesrc")
            .property("location", path.to_string_lossy().as_ref())
            .build()
            .expect("filesrc");
        let dec = gst::ElementFactory::make("decodebin").build().expect("decodebin");
        pipeline.add_many([&src, &dec]).unwrap();
        src.link(&dec).unwrap();

        // Each decoded stream gets its own fakesink, added + started live.
        let pipeline_weak = pipeline.downgrade();
        dec.connect_pad_added(move |_dbin, pad| {
            let Some(pipeline) = pipeline_weak.upgrade() else {
                return;
            };
            let sink = gst::ElementFactory::make("fakesink").build().expect("fakesink");
            pipeline.add(&sink).unwrap();
            sink.sync_state_with_parent().unwrap();
            let sink_pad = sink.static_pad("sink").unwrap();
            if !sink_pad.is_linked() {
                pad.link(&sink_pad).expect("link decoded pad → fakesink");
            }
        });

        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let mut reached_eos = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let Some(msg) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(200),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) else {
                continue;
            };
            match msg.view() {
                gst::MessageView::Eos(_) => {
                    reached_eos = true;
                    break;
                }
                gst::MessageView::Error(e) => {
                    let _ = pipeline.set_state(gst::State::Null);
                    panic!("recorded file is not decodable: {} ({:?})", e.error(), e.debug());
                }
                _ => {}
            }
        }
        let _ = pipeline.set_state(gst::State::Null);
        assert!(reached_eos, "decodebin never reached EOS — file not decodable");
    }

    /// WebM selects the always-available VP8/Opus/webmmux stack.
    #[test]
    fn select_codec_webm_is_vp8_opus_webm() {
        ensure_gst().expect("gst init");
        let c = select_codec(Container::WebM).expect("webm must be available");
        assert_eq!(c.video_enc, "vp8enc");
        assert_eq!(c.audio_enc, "opusenc");
        assert_eq!(c.muxer, "webmmux");
    }

    /// The codec-selection / unsupported path: `Container::Mp4` is honored iff an
    /// H.264 encoder plugin is installed. On a box without one it must return an
    /// honest `Backend` error naming the missing plugin — NOT a broken file, not
    /// a panic. Portable: asserts the branch that matches this host.
    #[test]
    fn select_codec_mp4_requires_h264_encoder() {
        ensure_gst().expect("gst init");
        let has_h264 = plugin_present("x264enc") || plugin_present("openh264enc");
        match (has_h264, select_codec(Container::Mp4)) {
            (true, Ok(c)) => {
                assert_eq!(c.muxer, "mp4mux");
                assert!(c.video_enc == "x264enc" || c.video_enc == "openh264enc");
            }
            (false, Err(MediaWriterError::Backend(msg))) => {
                assert!(
                    msg.contains("H.264"),
                    "error must name the missing H.264 encoder, got: {msg}"
                );
            }
            (true, Err(e)) => panic!("H.264 encoder present but Mp4 rejected: {e:?}"),
            (false, Ok(_)) => panic!("no H.264 encoder yet Mp4 was accepted (would write a broken file)"),
            (false, Err(other)) => panic!("expected an H.264-encoder Backend error, got: {other:?}"),
        }
    }

    /// The core headless proof: encode a handful of synthetic RGBA frames to a
    /// real `.webm` via the appsrc→vp8enc→webmmux pipeline, finalize with the
    /// EOS-to-finish handshake, then assert the file exists, is non-empty, is a
    /// real Matroska/WebM container (EBML magic), AND re-opens + decodes end to
    /// end. This is what a silently-degrading encoder (empty/corrupt file) would
    /// fail. Also the regression guard vs. the old stub, which returned
    /// `Unsupported` and wrote nothing.
    #[test]
    fn writes_and_reopens_a_valid_webm() {
        ensure_gst().expect("gst init");
        let codec = select_codec(Container::WebM).expect("webm available");
        let path = std::env::temp_dir().join("mw_linux_video_only.webm");
        let _ = std::fs::remove_file(&path);

        let pipe = build_pipeline(&codec, &path, true, false, 30, None, None)
            .expect("build video-only webm pipeline");
        pipe.pipeline
            .set_state(gst::State::Playing)
            .expect("pipeline to PLAYING");

        let mut enc = Encoder::new(pipe, path.clone());
        let (w, h) = (64u32, 48u32);
        let frame = rgba_frame(w, h);
        // 12 frames at 30fps on the shared-clock timeline (µs).
        for i in 0..12u64 {
            enc.on_video(w, h, i * (1_000_000 / 30), frame.clone());
        }
        enc.finish().expect("finalize webm");

        let meta = std::fs::metadata(&path).expect("output file exists");
        assert!(meta.len() > 0, "output webm is empty");
        let head = std::fs::read(&path).expect("read output");
        // Matroska/WebM begins with the EBML magic `1A 45 DF A3`.
        assert_eq!(
            &head[0..4],
            &[0x1A, 0x45, 0xDF, 0xA3],
            "output is not a Matroska/WebM (EBML) container"
        );
        assert_decodable(&path);
        let _ = std::fs::remove_file(&path);
    }

    /// Audio + video muxed into one WebM, decoded back to prove both streams are
    /// valid and the shared-epoch PTS keeps the muxer happy.
    #[test]
    fn writes_and_reopens_a_valid_av_webm() {
        ensure_gst().expect("gst init");
        let codec = select_codec(Container::WebM).expect("webm available");
        let path = std::env::temp_dir().join("mw_linux_av.webm");
        let _ = std::fs::remove_file(&path);

        let pipe = build_pipeline(&codec, &path, true, true, 30, None, None)
            .expect("build av webm pipeline");
        pipe.pipeline
            .set_state(gst::State::Playing)
            .expect("pipeline to PLAYING");

        let mut enc = Encoder::new(pipe, path.clone());
        let (w, h) = (64u32, 48u32);
        let frame = rgba_frame(w, h);
        let rate = 48_000u32;
        let channels = 2u16;
        // ~one video frame of audio per tick (480 frames = 10ms @ 48k... use one
        // video-frame worth ≈ 1600 frames @ 30fps for simplicity).
        let audio_frames = (rate / 30) as usize;
        let chunk = vec![0.0f32; audio_frames * channels as usize];
        for i in 0..12u64 {
            let pts = i * (1_000_000 / 30);
            enc.on_video(w, h, pts, frame.clone());
            enc.on_audio(rate, channels, pts, chunk.clone());
        }
        enc.finish().expect("finalize av webm");

        let meta = std::fs::metadata(&path).expect("output file exists");
        assert!(meta.len() > 0, "output av webm is empty");
        assert_decodable(&path);
        let _ = std::fs::remove_file(&path);
    }

    /// The error-domain labeling stays honest.
    #[test]
    fn error_mapping_is_backend() {
        let stream = glib::Error::new(gst::StreamError::Encode, "encode failed");
        assert!(matches!(
            map_gst_error(&stream, None),
            MediaWriterError::Backend(_)
        ));
        let resource = glib::Error::new(gst::ResourceError::OpenWrite, "cannot write");
        assert!(matches!(
            map_gst_error(&resource, None),
            MediaWriterError::Backend(_)
        ));
    }
}
