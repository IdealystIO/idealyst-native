//! Linux (desktop) file/URL video decode via **GStreamer** — the native Linux
//! media stack, and the same engine GTK's `GtkMediaFile` / WebKitGTK sit on.
//!
//! Where `apple.rs` uses `AVPlayer` + `AVPlayerItemVideoOutput` and `android.rs`
//! uses `MediaExtractor`/`MediaCodec`, this drives a hand-built pipeline that
//! decodes a clip into the SAME currency the SDK contract requires — a
//! [`MediaStream`](media_stream::MediaStream) of tightly-packed `RGBA8` frames —
//! plus a [`Transport`](crate::Transport) over the pipeline for play / pause /
//! seek / rate / position / duration.
//!
//! ```text
//!   <source> ! decodebin ! videoconvert ! [videoscale !] video/x-raw,RGBA ! appsink
//!                       \-> (audio) audioconvert ! audioresample ! volume ! autoaudiosink
//! ```
//!
//! ## Why `RGBA` at the appsink
//! [`FrameWriter::write_rgba8`](media_stream::FrameWriter::write_rgba8) wants
//! **tightly-packed top-down `RGBA8`**. Constraining the appsink's caps to
//! `video/x-raw,format=RGBA` makes `videoconvert` hand us exactly that, so the
//! frame map is a straight memcpy (or a per-row copy when a buffer carries
//! stride padding) — no swizzle, unlike the Apple `BGRA` path.
//!
//! ## Dynamic pad linking (a GStreamer invariant)
//! `decodebin` only knows a stream's type after typefinding, so it exposes its
//! decoded output on a **dynamic** pad. We link it in a `pad-added` callback,
//! gated on the pad's caps: `video/*` → the `videoconvert` chain feeding the
//! appsink; `audio/*` → an audio branch built on the fly and brought up with
//! `sync_state_with_parent` (the standard "add elements to a running pipeline"
//! dance). The audio branch is best-effort and never blocks video: if no output
//! device / `autoaudiosink` is available it falls back to `fakesink` so the
//! decoded audio pad is still drained (an unconsumed decodebin pad would stall
//! the whole pipeline).
//!
//! ## Async state + the bus/pull threads (no GLib main loop assumed)
//! GStreamer state changes are asynchronous and their outcomes (preroll, EOS,
//! errors) arrive as bus messages. This SDK may run outside GTK, so we assume NO
//! GLib main loop. Two dedicated threads own the live work (the pattern set by
//! `audio/src/linux.rs`):
//! - **bus thread** — `Bus::timed_pop(100ms)` + a `quit` flag; on EOS it either
//!   flush-seeks to 0 (looping) or marks the clip ended; on Error it logs + marks
//!   ended (never panics).
//! - **pull thread** — pulls decoded `RGBA` samples from the appsink via the
//!   `try-pull-sample` action signal (a 100 ms timeout so `quit` is seen
//!   promptly) and pushes each through the [`FrameWriter`].
//!
//! We drive the appsink through its **action signals** (`try-pull-sample`,
//! `pull-preroll`) rather than the typed `gstreamer_app::AppSink`, because only
//! the base `gstreamer` crate is a dependency here (matching `audio`'s dep set —
//! no new crate pulled in).
//!
//! ## Audio for the recorder
//! The clip's audio is played audibly (parity with `AVPlayer` / the web
//! `<video>`), but its PCM is NOT yet tapped into the [`AudioWriter`] for the
//! recording mux, so [`Opened::has_audio`](crate::Opened) is `false` and the
//! audio stream is dropped — the same posture `apple.rs` ships (its
//! `MTAudioProcessingTap` is gated OFF). Wiring a `tee`-off `appsink` PCM tap is
//! the follow-on that flips `has_audio` to `true`.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use gio::prelude::*;
use gstreamer as gst;
use gstreamer::prelude::*;

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

/// One-time, thread-safe, idempotent `gst::init()` — reused verbatim from
/// `audio/src/linux.rs`. A failed init (missing core libs) is reported honestly
/// on every call rather than silently skipped.
fn ensure_gst() -> Result<(), VideoDecodeError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(VideoDecodeError::Backend)
}

/// A cheap, cloneable, `'static` view over shared encoded bytes so a
/// [`gio::MemoryInputStream`] can wrap them without a copy. (Reused from the
/// audio backend — kept for a future raw-`Bytes` `DecodeSource`; today lib.rs
/// materializes `Bytes` to a temp `file://` before we're called.)
#[derive(Clone)]
struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// The resolved source. One pipeline shape; only the source element differs —
/// mirrors `audio/src/linux.rs`.
#[derive(Clone)]
enum SourceKind {
    /// Encoded clip in memory → `giostreamsrc` over a `MemoryInputStream`.
    Bytes(Arc<Vec<u8>>),
    /// A local filesystem path → `filesrc`.
    File(String),
    /// A remote `http(s)` URL → `souphttpsrc`, progressively streamed.
    Http(String),
}

/// Resolve a [`DecodeSource`] to a [`SourceKind`]. Native always receives a
/// `Url` (lib.rs materializes `Bytes` to a temp `file://` first), but we still
/// accept `Bytes` for completeness / a future direct path.
fn resolve_source(source: DecodeSource) -> Result<SourceKind, VideoDecodeError> {
    match source {
        DecodeSource::Bytes(data) => Ok(SourceKind::Bytes(Arc::new(data))),
        DecodeSource::Url(url) => {
            if let Some(path) = url.strip_prefix("file://") {
                Ok(SourceKind::File(path.to_string()))
            } else if url.starts_with("http://") || url.starts_with("https://") {
                Ok(SourceKind::Http(url))
            } else if url.contains("://") {
                // Any other scheme (data:, blob:, …) has no `souphttpsrc`/`filesrc`
                // element on Linux — report it honestly rather than mis-decode.
                Err(VideoDecodeError::BadSource(format!("unsupported URL scheme: {url}")))
            } else {
                // A bare path with no scheme → treat as a local file.
                Ok(SourceKind::File(url))
            }
        }
    }
}

/// Build the source element for a [`SourceKind`]. A missing element (plugin not
/// installed) is a `Backend` (environment) failure — we can't even reach the
/// decoder; genuine *decode* failures surface later as `StreamError`.
fn build_source(kind: &SourceKind) -> Result<gst::Element, VideoDecodeError> {
    match kind {
        SourceKind::Bytes(bytes) => {
            let gbytes = glib::Bytes::from_owned(SharedBytes(bytes.clone()));
            let stream = gio::MemoryInputStream::from_bytes(&gbytes);
            gst::ElementFactory::make("giostreamsrc")
                .property("stream", stream.upcast::<gio::InputStream>())
                .build()
                .map_err(|e| VideoDecodeError::Backend(format!("giostreamsrc unavailable: {e}")))
        }
        SourceKind::File(path) => gst::ElementFactory::make("filesrc")
            .property("location", path)
            .build()
            .map_err(|e| VideoDecodeError::Backend(format!("filesrc unavailable: {e}"))),
        SourceKind::Http(url) => gst::ElementFactory::make("souphttpsrc")
            .property("location", url)
            .build()
            .map_err(|e| VideoDecodeError::Backend(format!("souphttpsrc unavailable: {e}"))),
    }
}

/// Make a plain element by factory name; a missing element is `Backend`.
fn make(name: &str) -> Result<gst::Element, VideoDecodeError> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| VideoDecodeError::Backend(format!("missing GStreamer element `{name}`: {e}")))
}

/// Split a GStreamer error into an honest [`VideoDecodeError`]. GStreamer groups
/// errors by domain: **`StreamError`** ("can't decode this" — unknown type, no
/// decoder, corrupt) and **`ResourceError`** (missing file / unreachable URL /
/// unreadable) both mean the *source* is bad → [`VideoDecodeError::BadSource`];
/// everything else (`CoreError` negotiation/pipeline, `LibraryError`) is an
/// environment/config failure → [`VideoDecodeError::Backend`]. Adapts the audio
/// backend's split to this crate's error surface (which has no `Decode` variant,
/// but does distinguish a bad source from a backend fault).
fn map_gst_error(err: &glib::Error, debug: Option<&str>) -> VideoDecodeError {
    let detail = match debug {
        Some(d) if !d.is_empty() => format!("{err} ({d})"),
        _ => err.to_string(),
    };
    if err.kind::<gst::StreamError>().is_some() || err.kind::<gst::ResourceError>().is_some() {
        VideoDecodeError::BadSource(format!("GStreamer could not open/decode the source: {detail}"))
    } else {
        VideoDecodeError::Backend(format!("GStreamer pipeline error: {detail}"))
    }
}

/// Pop the first `Error` message currently on a pipeline's bus and map it; falls
/// back to a generic `Backend` if the failure left no detail. Reused shape from
/// the audio backend.
fn drain_error(pipeline: &gst::Pipeline) -> VideoDecodeError {
    if let Some(bus) = pipeline.bus() {
        while let Some(msg) =
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(200), &[gst::MessageType::Error])
        {
            if let gst::MessageView::Error(e) = msg.view() {
                return map_gst_error(&e.error(), e.debug().as_deref());
            }
        }
    }
    VideoDecodeError::Backend("GStreamer pipeline failed to preroll (no error detail)".into())
}

/// Target decode size honoring `max_dimension` (preserving aspect). `(0,0)`
/// natural size → no constraint. Copied from `apple.rs::target_size`.
fn target_size(nat_w: u32, nat_h: u32, max_dim: Option<u32>) -> (u32, u32) {
    match (max_dim, nat_w, nat_h) {
        (Some(max), w, h) if w > 0 && h > 0 && w.max(h) > max => {
            let scale = max as f32 / w.max(h) as f32;
            (
                ((w as f32 * scale) as u32).max(1),
                ((h as f32 * scale) as u32).max(1),
            )
        }
        _ => (nat_w, nat_h),
    }
}

/// The appsink filter caps: always `RGBA`, plus a fixed `width`/`height` (and
/// `1/1` pixel-aspect-ratio) when we're downscaling to honor `max_dimension`.
fn build_video_caps(scale_to: Option<(u32, u32)>) -> gst::Caps {
    let builder = gst::Caps::builder("video/x-raw").field("format", "RGBA");
    match scale_to {
        Some((w, h)) => builder
            .field("width", w as i32)
            .field("height", h as i32)
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .build(),
        None => builder.build(),
    }
}

/// Map one decoded `RGBA` [`gst::Sample`] to a frame and push it through the
/// writer, returning the frame's `(width, height)` on success. Shared by the
/// live pull thread AND the headless test, so the test exercises the exact
/// pull → map → write path the decoder uses.
///
/// The appsink caps pin the format to `RGBA`, so the buffer is normally tightly
/// packed (`width * height * 4`) and copied straight through. GStreamer may pad
/// rows to an alignment; if the buffer is larger than the tight size and evenly
/// divisible by the height, we treat `len / height` as the stride and copy row
/// by row into a tight buffer. The buffer PTS is carried as the frame timestamp
/// (nanoseconds → microseconds) so a downstream muxer sees the real cadence.
fn push_sample(sample: &gst::Sample, frames: &FrameWriter) -> Option<(u32, u32)> {
    let caps = sample.caps()?;
    let structure = caps.structure(0)?;
    let width = structure.get::<i32>("width").ok()?;
    let height = structure.get::<i32>("height").ok()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let (w, h) = (width as u32, height as u32);

    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    let data = map.as_slice();
    let tight = w as usize * h as usize * 4;
    // GStreamer PTS is on the pipeline timeline in ns; the frame timestamp is µs.
    let pts_us = buffer.pts().map(|t| t.nseconds() / 1000);

    let write = |bytes: &[u8]| match pts_us {
        Some(us) => frames.write_rgba8_at(w, h, bytes, us),
        None => frames.write_rgba8(w, h, bytes),
    };

    if data.len() == tight {
        write(data);
    } else if data.len() > tight && data.len() % h as usize == 0 {
        let stride = data.len() / h as usize;
        let row = w as usize * 4;
        if stride < row {
            return None;
        }
        let mut packed = Vec::with_capacity(tight);
        for y in 0..h as usize {
            packed.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        write(&packed);
    } else {
        return None;
    }
    Some((w, h))
}

/// Probe the source to validate it decodes, learn its natural video size, and
/// note whether it carries an audio track — the video analogue of the audio
/// backend's `preroll`. Building `<source> ! decodebin` and driving it to
/// `PAUSED` prerolls every stream (mapping a bad container to `BadSource` and a
/// missing file / unreachable URL to `BadSource`, a pipeline fault to
/// `Backend`), then we read the decoded pads' caps and tear the probe down.
fn probe(kind: &SourceKind) -> Result<(Option<(u32, u32)>, bool), VideoDecodeError> {
    let pipeline = gst::Pipeline::default();
    let source = build_source(kind)?;
    let decodebin = make("decodebin")?;
    pipeline
        .add_many([&source, &decodebin])
        .map_err(|e| VideoDecodeError::Backend(format!("probe add failed: {e}")))?;
    source
        .link(&decodebin)
        .map_err(|e| VideoDecodeError::Backend(format!("probe source→decodebin link failed: {e}")))?;

    // Drain each decoded pad into its own `fakesink` so preroll can complete —
    // an unconsumed decodebin pad blocks the state change.
    let pipeline_weak = pipeline.downgrade();
    decodebin.connect_pad_added(move |_dbin, src_pad| {
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let Ok(fakesink) = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .build()
        else {
            return;
        };
        if pipeline.add(&fakesink).is_err() {
            return;
        }
        let _ = fakesink.sync_state_with_parent();
        if let Some(sink_pad) = fakesink.static_pad("sink") {
            let _ = src_pad.link(&sink_pad);
        }
    });

    if pipeline.set_state(gst::State::Paused).is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }
    // Block up to 10 s for preroll (enough for a remote stream to connect +
    // buffer its first data); guards against a hang.
    let (result, _current, _pending) = pipeline.state(gst::ClockTime::from_seconds(10));
    if result.is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    // Preroll done → the decoded pads now carry negotiated caps. Read them.
    let mut natural: Option<(u32, u32)> = None;
    let mut has_audio = false;
    let mut pads = decodebin.iterate_src_pads();
    while let Ok(Some(pad)) = pads.next() {
        let Some(caps) = pad.current_caps().or_else(|| Some(pad.query_caps(None))) else {
            continue;
        };
        let Some(structure) = caps.structure(0) else {
            continue;
        };
        let name = structure.name();
        if name.starts_with("video/") {
            let w = structure.get::<i32>("width").ok();
            let h = structure.get::<i32>("height").ok();
            if let (Some(w), Some(h)) = (w, h) {
                if w > 0 && h > 0 {
                    natural = Some((w as u32, h as u32));
                }
            }
        } else if name.starts_with("audio/") {
            has_audio = true;
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
    Ok((natural, has_audio))
}

/// A constructed (not-yet-started) live pipeline plus the handles the decoder +
/// transport need.
struct Built {
    pipeline: gst::Pipeline,
    appsink: gst::Element,
    /// The audio branch's `volume` element once (and if) its pad is linked, for
    /// the transport's mute. Filled on `pad-added` from the streaming thread.
    volume: Arc<Mutex<Option<gst::Element>>>,
}

/// Assemble `<source> ! decodebin ! videoconvert ! [videoscale !] appsink`
/// (RGBA), wiring decodebin's dynamic pads: `video/*` → the converter chain,
/// `audio/*` → a best-effort audible branch.
fn build_pipeline(
    kind: &SourceKind,
    scale_to: Option<(u32, u32)>,
) -> Result<Built, VideoDecodeError> {
    let pipeline = gst::Pipeline::default();

    let source = build_source(kind)?;
    let decodebin = make("decodebin")?;
    let videoconvert = make("videoconvert")?;
    let appsink = make("appsink")?;

    appsink.set_property("caps", build_video_caps(scale_to));
    // Real-time delivery (frames land at playback cadence, for compositing) and
    // a bounded, drop-old queue so a slow consumer never back-pressures decode
    // (which would also stall audio).
    appsink.set_property("sync", true);
    appsink.set_property("max-buffers", 4u32);
    appsink.set_property("drop", true);

    let videoscale = match scale_to {
        Some(_) => Some(make("videoscale")?),
        None => None,
    };

    pipeline
        .add_many([&source, &decodebin, &videoconvert, &appsink])
        .map_err(|e| VideoDecodeError::Backend(format!("pipeline add failed: {e}")))?;
    if let Some(scale) = &videoscale {
        pipeline
            .add(scale)
            .map_err(|e| VideoDecodeError::Backend(format!("videoscale add failed: {e}")))?;
    }

    source
        .link(&decodebin)
        .map_err(|e| VideoDecodeError::Backend(format!("source→decodebin link failed: {e}")))?;
    // Static video tail: videoconvert ! [videoscale !] appsink.
    match &videoscale {
        Some(scale) => gst::Element::link_many([&videoconvert, scale, &appsink])
            .map_err(|e| VideoDecodeError::Backend(format!("video chain link failed: {e}")))?,
        None => videoconvert
            .link(&appsink)
            .map_err(|e| VideoDecodeError::Backend(format!("videoconvert→appsink link failed: {e}")))?,
    }

    let volume: Arc<Mutex<Option<gst::Element>>> = Arc::new(Mutex::new(None));

    // Dynamic half: link decodebin's decoded pad by media type.
    let videoconvert_weak = videoconvert.downgrade();
    let pipeline_weak = pipeline.downgrade();
    let volume_slot = volume.clone();
    decodebin.connect_pad_added(move |_dbin, src_pad| {
        let media = src_pad
            .current_caps()
            .or_else(|| Some(src_pad.query_caps(None)))
            .and_then(|caps| caps.structure(0).map(|s| s.name().to_string()));
        let Some(media) = media else {
            return;
        };

        if media.starts_with("video/") {
            let Some(videoconvert) = videoconvert_weak.upgrade() else {
                return;
            };
            let Some(sink_pad) = videoconvert.static_pad("sink") else {
                return;
            };
            if !sink_pad.is_linked() {
                let _ = src_pad.link(&sink_pad); // first video pad wins
            }
        } else if media.starts_with("audio/") {
            let Some(pipeline) = pipeline_weak.upgrade() else {
                return;
            };
            // Best-effort: a failure here leaves audio unlinked but never
            // disturbs the video path.
            let _ = link_audio_branch(&pipeline, src_pad, &volume_slot);
        }
    });

    Ok(Built {
        pipeline,
        appsink,
        volume,
    })
}

/// Build + attach an audible audio branch (`audioconvert ! audioresample !
/// volume ! autoaudiosink`) to the running pipeline and link `src_pad` into it.
/// Falls back to `fakesink` when no output device / `autoaudiosink` is
/// available, so the decoded audio pad is always drained (an unconsumed pad
/// stalls decodebin). Records the `volume` element for the transport's mute.
///
/// Adding elements to an already-live pipeline requires `sync_state_with_parent`
/// on each so they catch up to the pipeline's current state — the standard
/// GStreamer dynamic-pipeline pattern.
fn link_audio_branch(
    pipeline: &gst::Pipeline,
    src_pad: &gst::Pad,
    volume_slot: &Arc<Mutex<Option<gst::Element>>>,
) -> Result<(), VideoDecodeError> {
    let convert = make("audioconvert")?;
    let resample = make("audioresample")?;
    let volume = make("volume")?;
    let sink = gst::ElementFactory::make("autoaudiosink")
        .build()
        .or_else(|_| gst::ElementFactory::make("fakesink").property("sync", true).build())
        .map_err(|e| VideoDecodeError::Backend(format!("audio sink unavailable: {e}")))?;

    pipeline
        .add_many([&convert, &resample, &volume, &sink])
        .map_err(|e| VideoDecodeError::Backend(format!("audio branch add failed: {e}")))?;
    gst::Element::link_many([&convert, &resample, &volume, &sink])
        .map_err(|e| VideoDecodeError::Backend(format!("audio branch link failed: {e}")))?;
    for element in [&convert, &resample, &volume, &sink] {
        element
            .sync_state_with_parent()
            .map_err(|e| VideoDecodeError::Backend(format!("audio branch sync failed: {e}")))?;
    }

    let Some(sink_pad) = convert.static_pad("sink") else {
        return Err(VideoDecodeError::Backend("audioconvert has no sink pad".into()));
    };
    src_pad
        .link(&sink_pad)
        .map_err(|e| VideoDecodeError::Backend(format!("audio pad link failed: {e}")))?;

    *volume_slot.lock().unwrap() = Some(volume);
    Ok(())
}

/// The bus-message loop (dedicated thread; no GLib main loop). On EOS it
/// flush-seeks to 0 when looping, else marks the clip ended; on Error it logs +
/// marks ended. Exits when `quit` is set. Mirrors `audio/src/linux.rs::bus_loop`.
fn bus_loop(
    bus: gst::Bus,
    pipeline: gst::Pipeline,
    looping: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
) {
    loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        match msg.view() {
            gst::MessageView::Eos(_) => {
                if looping.load(Ordering::Relaxed) {
                    let _ = pipeline.seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                        gst::ClockTime::ZERO,
                    );
                } else {
                    ended.store(true, Ordering::Relaxed);
                }
            }
            gst::MessageView::Error(e) => {
                eprintln!(
                    "video-decode(linux): playback error: {} ({:?})",
                    e.error(),
                    e.debug()
                );
                ended.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// The frame pull loop (dedicated thread). Pulls decoded `RGBA` samples from the
/// appsink via the `try-pull-sample` action signal (100 ms timeout so `quit` is
/// observed promptly) and pushes each through the [`FrameWriter`]. `sync=true`
/// on the appsink means samples arrive at playback cadence; while paused none
/// arrive and the loop simply idles.
fn pull_loop(appsink: gst::Element, frames: FrameWriter, quit: Arc<AtomicBool>) {
    // 100 ms in ns for the `try-pull-sample` timeout arg (a `GstClockTime`).
    let timeout_ns: u64 = 100_000_000;
    loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        let sample: Option<gst::Sample> =
            appsink.emit_by_name("try-pull-sample", &[&timeout_ns]);
        if let Some(sample) = sample {
            push_sample(&sample, &frames);
        }
        // `None` = timeout or EOS; loop and re-check `quit`.
    }
}

/// The GStreamer-backed transport. Used only on the main thread (Rc-wrapped,
/// matching `MediaStream`'s `!Send` contract), so `Cell` state is fine.
struct LinuxTransport {
    pipeline: gst::Pipeline,
    /// The audio branch's `volume` element, once linked (see `link_audio_branch`).
    volume: Arc<Mutex<Option<gst::Element>>>,
    muted: Cell<bool>,
    /// Last requested playback rate; `play()` resumes at it, seeks preserve it.
    rate: Cell<f32>,
    /// Set by `bus_loop` at a non-looping EOS (or error) — makes `is_playing`
    /// report false once the clip finishes.
    ended: Arc<AtomicBool>,
}

impl LinuxTransport {
    fn seek_at(&self, seconds: f32, flags: gst::SeekFlags, rate: f64) {
        let pos = gst::ClockTime::from_nseconds((seconds.max(0.0) as f64 * 1e9) as u64);
        let _ = self.pipeline.seek(
            rate,
            flags,
            gst::SeekType::Set,
            pos,
            gst::SeekType::None,
            gst::ClockTime::NONE,
        );
    }
}

impl TransportControl for LinuxTransport {
    fn play(&self) {
        let r = self.rate.get();
        let r = if r <= 0.0 { 1.0 } else { r };
        self.rate.set(r);
        self.ended.store(false, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Playing);
    }

    fn pause(&self) {
        let _ = self.pipeline.set_state(gst::State::Paused);
    }

    fn seek(&self, seconds: f32) {
        // EXACT (ACCURATE) — decode the precise frame for a scrub's landing.
        let rate = self.rate.get();
        let rate = if rate <= 0.0 { 1.0 } else { rate as f64 };
        self.seek_at(seconds, gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, rate);
    }

    fn seek_preview(&self, seconds: f32) {
        // FAST — land on the nearest keyframe for smooth live scrubbing.
        let rate = self.rate.get();
        let rate = if rate <= 0.0 { 1.0 } else { rate as f64 };
        self.seek_at(seconds, gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, rate);
    }

    fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        if let Some(volume) = self.volume.lock().unwrap().as_ref() {
            volume.set_property("mute", muted);
        }
    }

    fn set_rate(&self, rate: f32) {
        let rate = rate.max(0.0);
        self.rate.set(rate);
        if rate == 0.0 {
            // Rate 0 == pause, per the contract.
            let _ = self.pipeline.set_state(gst::State::Paused);
            return;
        }
        // Instant rate change: re-seek from the current position with the new
        // rate (a flushing seek is how GStreamer changes playback rate).
        let pos = self
            .pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f32 / 1e9)
            .unwrap_or(0.0);
        self.seek_at(pos, gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, rate as f64);
    }

    fn position(&self) -> f32 {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f32 / 1e9)
            .unwrap_or(0.0)
    }

    fn duration(&self) -> f32 {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f32 / 1e9)
            .unwrap_or(0.0)
    }

    fn is_playing(&self) -> bool {
        !self.ended.load(Ordering::Relaxed)
            && self.rate.get() > 0.0
            && self.pipeline.current_state() == gst::State::Playing
    }

    fn is_muted(&self) -> bool {
        self.muted.get()
    }
}

/// Keeps decode alive; `Drop` stops the pipeline (state → `Null`) and joins both
/// worker threads — RAII teardown, matching the audio backend's `PlaybackHandle`.
struct StreamHandle {
    pipeline: Option<gst::Pipeline>,
    quit: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
    pull_thread: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Signal the threads first, then tear the pipeline down, then join.
        self.quit.store(true, Ordering::Relaxed);
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        if let Some(t) = self.bus_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.pull_thread.take() {
            let _ = t.join();
        }
    }
}

/// Open + begin decoding `source`. Native always receives a `Url` (lib.rs
/// materializes `Bytes` to a temp `file://` before calling us).
///
/// We probe the source first (validating decode, learning natural size + audio
/// presence — honest `BadSource`/`Backend` at open, parity with the audio
/// backend's eager validation), then build the live pipeline, push the first
/// frame from the preroll so a paused clip still shows a frame, and spawn the
/// bus + pull threads. `audio` (the recorder PCM writer) is intentionally
/// dropped for now — see the module note; the clip still plays its sound.
pub(crate) async fn open(
    source: DecodeSource,
    config: DecodeConfig,
    frames: FrameWriter,
    _audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    ensure_gst()?;

    let kind = resolve_source(source)?;

    // Validate decode + learn natural size / audio presence.
    let (natural, _has_audio_track) = probe(&kind)?;
    let scale_to = match (config.max_dimension, natural) {
        (Some(_), Some((nw, nh))) => {
            let (tw, th) = target_size(nw, nh, config.max_dimension);
            if (tw, th) != (nw, nh) {
                Some((tw, th))
            } else {
                None
            }
        }
        _ => None,
    };

    let built = build_pipeline(&kind, scale_to)?;
    let pipeline = built.pipeline;
    let appsink = built.appsink;
    let volume = built.volume;

    let bus = pipeline
        .bus()
        .ok_or_else(|| VideoDecodeError::Backend("pipeline has no bus".into()))?;

    // Preroll to PAUSED so the first frame is decoded and available even if we
    // don't autoplay; a failure here maps to an honest error.
    if pipeline.set_state(gst::State::Paused).is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }
    let (result, _current, _pending) = pipeline.state(gst::ClockTime::from_seconds(10));
    if result.is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    // Push the preroll frame so a paused clip shows its first frame immediately.
    let preroll: Option<gst::Sample> = appsink.emit_by_name("pull-preroll", &[]);
    if let Some(sample) = preroll {
        push_sample(&sample, &frames);
    }

    let looping = Arc::new(AtomicBool::new(config.loop_playback));
    let ended = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));

    if config.muted {
        if let Some(volume) = volume.lock().unwrap().as_ref() {
            volume.set_property("mute", true);
        }
    }

    if config.autoplay {
        let _ = pipeline.set_state(gst::State::Playing);
    }

    let bus_thread = {
        let pipeline = pipeline.clone();
        let looping = looping.clone();
        let ended = ended.clone();
        let quit = quit.clone();
        std::thread::spawn(move || bus_loop(bus, pipeline, looping, ended, quit))
    };
    let pull_thread = {
        let appsink = appsink.clone();
        let frames = frames.clone();
        let quit = quit.clone();
        std::thread::spawn(move || pull_loop(appsink, frames, quit))
    };

    let control: std::rc::Rc<dyn TransportControl> = std::rc::Rc::new(LinuxTransport {
        pipeline: pipeline.clone(),
        volume,
        muted: Cell::new(config.muted),
        rate: Cell::new(if config.autoplay { 1.0 } else { 0.0 }),
        ended,
    });

    let handle = StreamHandle {
        pipeline: Some(pipeline),
        quit,
        bus_thread: Some(bus_thread),
        pull_thread: Some(pull_thread),
    };

    Ok(Opened {
        handle: Box::new(handle),
        control,
        // PCM tap for the recorder is not yet wired (parity with apple.rs, whose
        // audio tap is gated OFF); the clip still plays its sound audibly.
        has_audio: false,
        natural_size: natural,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `videotestsrc num-buffers=N ! videoconvert ! <RGBA caps> ! appsink`,
    /// pull one sample through the SAME action-signal + `push_sample` path the
    /// live decoder uses, and return the frame it wrote into a `MediaStream`.
    /// No file, no device — the headless proof that the appsink pull + RGBA map
    /// works, and the core regression guard (against the old stub this whole
    /// module — and therefore this test — did not exist, so the backend returned
    /// `Unsupported` for everything).
    fn pull_one_videotestsrc_frame(
        w: u32,
        h: u32,
    ) -> (Option<(u32, u32)>, media_stream::MediaStream) {
        ensure_gst().expect("gst init");
        let pipeline = gst::Pipeline::default();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 1i32)
            .build()
            .expect("videotestsrc");
        let convert = make("videoconvert").expect("videoconvert");
        let appsink = make("appsink").expect("appsink");
        appsink.set_property("caps", build_video_caps(Some((w, h))));
        appsink.set_property("sync", false);
        let scale = make("videoscale").expect("videoscale");

        pipeline
            .add_many([&src, &convert, &scale, &appsink])
            .expect("add");
        gst::Element::link_many([&src, &convert, &scale, &appsink]).expect("link");

        pipeline.set_state(gst::State::Playing).expect("play");

        let (stream, writer) = media_stream::MediaStream::new();
        // Pull with a generous timeout (2 s) for the single buffer to arrive.
        let timeout_ns: u64 = 2_000_000_000;
        let sample: Option<gst::Sample> =
            appsink.emit_by_name("try-pull-sample", &[&timeout_ns]);
        let dims = sample.and_then(|s| push_sample(&s, &writer));

        let _ = pipeline.set_state(gst::State::Null);
        (dims, stream)
    }

    /// The load-bearing regression test: the real appsink pull + RGBA frame map
    /// yields a tightly-packed `width*height*4` `RGBA8` frame of the requested
    /// size — exercised headless with `videotestsrc` (no file, no device).
    #[test]
    fn pulls_rgba_frame_from_videotestsrc() {
        let (dims, stream) = pull_one_videotestsrc_frame(320, 240);
        assert_eq!(dims, Some((320, 240)), "appsink must yield a 320x240 frame");

        let mut buf = Vec::new();
        let got = stream.latest(&mut buf).expect("a frame was written");
        assert_eq!(got, (320, 240));
        assert_eq!(
            buf.len(),
            320 * 240 * 4,
            "frame must be tightly-packed RGBA8 (w*h*4)"
        );
    }

    /// A different size proves width/height are read from the sample caps, not
    /// hard-coded, and that the odd-ish dimension still packs tightly.
    #[test]
    fn frame_dimensions_come_from_caps() {
        let (dims, stream) = pull_one_videotestsrc_frame(64, 48);
        assert_eq!(dims, Some((64, 48)));
        let mut buf = Vec::new();
        stream.latest(&mut buf).expect("frame");
        assert_eq!(buf.len(), 64 * 48 * 4);
    }

    /// The error-domain split must be honest: a `StreamError` (can't decode) and
    /// a `ResourceError` (missing/unreadable) both mean a bad *source* →
    /// `BadSource`; anything else (`CoreError`) is a backend fault → `Backend`.
    #[test]
    fn error_mapping_splits_source_vs_backend() {
        let stream = glib::Error::new(gst::StreamError::CodecNotFound, "no codec");
        assert!(
            matches!(map_gst_error(&stream, None), VideoDecodeError::BadSource(_)),
            "StreamError must map to BadSource"
        );

        let resource = glib::Error::new(gst::ResourceError::NotFound, "missing");
        assert!(
            matches!(map_gst_error(&resource, None), VideoDecodeError::BadSource(_)),
            "ResourceError must map to BadSource"
        );

        let core = glib::Error::new(gst::CoreError::Negotiation, "no format");
        assert!(
            matches!(map_gst_error(&core, None), VideoDecodeError::Backend(_)),
            "non-source errors must map to Backend"
        );
    }

    /// Garbage bytes must not preroll into a playable clip — `probe` must reject
    /// them with a typed `BadSource` (never panic, never `Unsupported`).
    /// `decodebin` fails typefinding on random bytes → `StreamError` →
    /// `BadSource`. Exercised headless (no device, no real file: the bytes go
    /// through `giostreamsrc` from RAM).
    #[test]
    fn garbage_bytes_probe_to_bad_source() {
        ensure_gst().expect("gst init");
        let kind = SourceKind::Bytes(Arc::new(vec![0u8; 512]));
        match probe(&kind) {
            Err(VideoDecodeError::BadSource(_)) => {}
            Err(other) => panic!("expected BadSource for garbage, got {other:?}"),
            Ok(_) => panic!("garbage bytes must not probe as a decodable clip"),
        }
    }

    /// End-to-end via the PUBLIC API: opening garbage bytes must fail with a
    /// typed error and, crucially, must NOT return `Unsupported` — proving the
    /// Linux backend (not the stub) is the one the cfg cascade selected. This is
    /// the test that FAILS against the old `stub.rs` (which returned
    /// `Unsupported` for every input).
    #[tokio::test]
    async fn public_open_reaches_linux_backend_not_stub() {
        let dec = crate::VideoDecoder::new();
        let result = dec
            .open(
                crate::DecodeSource::bytes(vec![0u8; 512]),
                crate::DecodeConfig::default(),
            )
            .await;
        match result {
            Err(VideoDecodeError::Unsupported) => {
                panic!("Linux must not report Unsupported — the stub is still selected")
            }
            Err(VideoDecodeError::BadSource(_)) => {} // expected: garbage rejected
            Err(VideoDecodeError::Backend(_)) => {}   // acceptable environment fault
            Ok(_) => panic!("garbage bytes must not open into a decodable clip"),
        }
    }

    /// Real-file decode (needs an actual clip). Ignored by default; run with
    /// `--ignored` and set `IDEALYST_TEST_VIDEO=/path/to/clip.mp4` to decode a
    /// real file end-to-end and assert a frame flows. Exercises the full
    /// `open` → probe → pipeline → pull path.
    #[tokio::test]
    #[ignore = "requires a real video file via IDEALYST_TEST_VIDEO=/path/to/clip.mp4"]
    async fn decodes_a_real_file_end_to_end() {
        let Ok(path) = std::env::var("IDEALYST_TEST_VIDEO") else {
            return;
        };
        let dec = crate::VideoDecoder::new();
        let mut config = crate::DecodeConfig::default();
        config.autoplay = true;
        let clip = dec
            .open(crate::DecodeSource::url(format!("file://{path}")), config)
            .await
            .expect("real file must open");
        // Poll for a frame for up to ~3 s.
        let mut buf = Vec::new();
        let mut got = None;
        for _ in 0..300 {
            if let Some(dims) = clip.frames().latest(&mut buf) {
                got = Some(dims);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(got.is_some(), "a real clip must produce at least one frame");
    }
}
