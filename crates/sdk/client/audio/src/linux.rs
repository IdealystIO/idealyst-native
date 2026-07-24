//! Linux (and other native ALSA/PipeWire desktop) playback via **GStreamer**.
//!
//! GStreamer is the native Linux media stack (the same engine GTK's
//! `GtkMediaFile` / WebKitGTK sit on). We drive a small hand-built pipeline
//!
//! ```text
//!   <source> ! decodebin ! audioconvert ! audioresample ! volume ! autoaudiosink
//! ```
//!
//! which gives exactly the control surface this SDK exposes: dynamic `volume`
//! via the `volume` element, pause/resume via the pipeline state, EOS-driven
//! looping via a flushing seek, and RAII stop via a transition to `Null`.
//!
//! ## Why GStreamer and not `rodio`
//!
//! The previous backend used `rodio`, which decodes from a **seekable byte
//! source** and has no network stack — so a remote `http(s)` URL could not be
//! streamed and surfaced an [`AudioError::Backend`]. That broke the framework's
//! uniform-behavior rule (CLAUDE.md §7): Android (`MediaPlayer`), Apple
//! (`AVPlayer`), and web (browser fetch) all stream remote URLs. GStreamer's
//! `souphttpsrc` **progressively streams** an `http(s)` URI exactly like the
//! mobile players, restoring parity.
//!
//! ## The three source kinds (all no-temp-file)
//!
//! One pipeline shape; only the *source element* differs:
//!
//! - **`Bytes`** (in memory) → `giostreamsrc` fed a [`gio::MemoryInputStream`]
//!   built over the shared bytes. This avoids a temp file entirely — the
//!   decoder reads straight from RAM. (`appsrc` would also work but needs a
//!   push loop; a temp file is what we explicitly avoid.)
//! - **`Path`** (local file) → `filesrc location=<path>`.
//! - **`Url`** (`http(s)`) → `souphttpsrc location=<url>`, **progressively
//!   streamed** — the whole reason this backend exists. A `file://` URL is
//!   treated as a local path.
//!
//! `decodebin` auto-plugs the right demuxer/decoder for the container
//! (wav / mp3 / flac / ogg / aac / …) in every case, so the format handling is
//! identical across the three.
//!
//! ## Async state changes + the bus thread (no GLib main loop assumed)
//!
//! GStreamer state changes are **asynchronous** and their outcomes
//! (preroll-done, EOS, errors) arrive as **bus messages**. This SDK may run
//! outside GTK, so we must NOT assume a GLib main loop is spinning to dispatch
//! them. Instead each live [`PlaybackHandle`] owns a **dedicated bus thread**
//! that blocks on `Bus::timed_pop` and handles messages itself:
//!
//! - **EOS** → if the looping flag is set, seek to 0 with `FLUSH` (restart);
//!   otherwise mark the playback ended.
//! - **Error** → log and mark ended (never panic).
//!
//! The thread wakes every 100 ms to observe the `quit` flag so `Drop` can join
//! it promptly. Messages arrive with near-zero latency (a queued message
//! returns from `timed_pop` immediately); the 100 ms timeout only bounds how
//! fast the thread notices shutdown.
//!
//! ## Reusable pieces for the GStreamer "media cluster"
//!
//! The idempotent [`ensure_gst`] init (`OnceLock` around `gst::init`), the
//! [`bus_loop`] pattern (own the bus on a thread, no main loop), the honest
//! [`map_gst_error`] domain split (`StreamError` → `Decode`, everything else →
//! `Backend`), and the `giostreamsrc` + `MemoryInputStream` in-memory source
//! are all intended to be copied by the upcoming camera / media-writer /
//! video-decode / screen-recorder backends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use gio::prelude::*;
use gstreamer as gst;
use gstreamer::prelude::*;

use crate::{AudioError, AudioSource};

/// One-time, thread-safe, idempotent `gst::init()`. Stores the outcome so a
/// failed init (rare — missing core libs) is reported honestly on every call
/// rather than silently skipped. Reusable verbatim by the media cluster.
fn ensure_gst() -> Result<(), AudioError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(AudioError::Backend)
}

/// A cheap, cloneable, `'static` view over shared encoded bytes so a
/// [`gio::MemoryInputStream`] can wrap them without copying the buffer for
/// every `play()`.
#[derive(Clone)]
struct SharedBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// The resolved source, kept in the [`PreparedSound`] so each `play()` can
/// rebuild an independent pipeline (independent overlapping voice, matching
/// the web/Apple mixing behavior).
#[derive(Clone)]
enum SourceKind {
    /// Encoded audio in memory → `giostreamsrc` over a `MemoryInputStream`.
    Bytes(Arc<Vec<u8>>),
    /// A local filesystem path → `filesrc`.
    File(String),
    /// A remote `http(s)` URL → `souphttpsrc`, progressively streamed.
    Http(String),
}

/// Build the source element for a [`SourceKind`]. A missing element (plugin
/// not installed) is a `Backend` (environment) failure — we can't even reach
/// the decoder; genuine *decode* failures surface later as `StreamError`.
fn build_source(kind: &SourceKind) -> Result<gst::Element, AudioError> {
    match kind {
        SourceKind::Bytes(bytes) => {
            // `from_owned` takes ownership of the (cheap, ref-counted)
            // `SharedBytes`, so the decoder reads straight out of RAM — no
            // temp file, no copy per play().
            let gbytes = glib::Bytes::from_owned(SharedBytes(bytes.clone()));
            let stream = gio::MemoryInputStream::from_bytes(&gbytes);
            gst::ElementFactory::make("giostreamsrc")
                .property("stream", stream.upcast::<gio::InputStream>())
                .build()
                .map_err(|e| AudioError::Backend(format!("giostreamsrc unavailable: {e}")))
        }
        SourceKind::File(path) => gst::ElementFactory::make("filesrc")
            .property("location", path)
            .build()
            .map_err(|e| AudioError::Backend(format!("filesrc unavailable: {e}"))),
        SourceKind::Http(url) => gst::ElementFactory::make("souphttpsrc")
            .property("location", url)
            .build()
            .map_err(|e| AudioError::Backend(format!("souphttpsrc unavailable: {e}"))),
    }
}

/// Make a plain element by factory name; a missing element is `Backend`.
fn make(name: &str) -> Result<gst::Element, AudioError> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| AudioError::Backend(format!("missing GStreamer element `{name}`: {e}")))
}

/// A constructed (not-yet-started) pipeline plus the handles the control
/// surface needs.
struct Built {
    pipeline: gst::Pipeline,
    volume: gst::Element,
}

/// Assemble `<source> ! decodebin ! audioconvert ! audioresample ! volume !
/// <sink>`. `sink_name` is `"autoaudiosink"` in production and `"fakesink"` in
/// headless tests (no output device needed to prove decode).
///
/// `decodebin` exposes its decoded output on a **dynamic** pad (it only knows
/// the stream type after typefinding), so we link it to the converter chain in
/// a `pad-added` callback rather than statically.
fn build_pipeline(kind: &SourceKind, sink_name: &str) -> Result<Built, AudioError> {
    let pipeline = gst::Pipeline::default();

    let source = build_source(kind)?;
    let decodebin = make("decodebin")?;
    let audioconvert = make("audioconvert")?;
    let audioresample = make("audioresample")?;
    let volume = make("volume")?;
    let sink = make(sink_name)?;

    pipeline
        .add_many([
            &source,
            &decodebin,
            &audioconvert,
            &audioresample,
            &volume,
            &sink,
        ])
        .map_err(|e| AudioError::Backend(format!("pipeline add failed: {e}")))?;

    // Static half: source → decodebin, and convert → resample → volume → sink.
    source
        .link(&decodebin)
        .map_err(|e| AudioError::Backend(format!("source→decodebin link failed: {e}")))?;
    gst::Element::link_many([&audioconvert, &audioresample, &volume, &sink])
        .map_err(|e| AudioError::Backend(format!("audio chain link failed: {e}")))?;

    // Dynamic half: decodebin's decoded pad → audioconvert. Only link audio
    // pads (a container could expose other stream types); ignore anything
    // else so we don't try to feed video into the audio chain.
    let convert_weak = audioconvert.downgrade();
    decodebin.connect_pad_added(move |_dbin, src_pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let sink_pad = convert
            .static_pad("sink")
            .expect("audioconvert always has a static sink pad");
        if sink_pad.is_linked() {
            return; // already linked (first audio pad wins)
        }
        // Gate on caps: only link audio/* pads.
        let is_audio = src_pad
            .current_caps()
            .or_else(|| Some(src_pad.query_caps(None)))
            .and_then(|caps| caps.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(false);
        if is_audio {
            let _ = src_pad.link(&sink_pad);
        }
    });

    Ok(Built { pipeline, volume })
}

/// Split a GStreamer error into an honest [`AudioError`]. GStreamer groups
/// errors by domain: **`StreamError`** covers "can't decode this" (unknown
/// type, no decoder, corrupt/format) → [`AudioError::Decode`]; everything else
/// (`ResourceError` for missing files / unreachable URLs / no device,
/// `CoreError` for pipeline/negotiation) → [`AudioError::Backend`]. Reusable by
/// the media cluster.
fn map_gst_error(err: &glib::Error, debug: Option<&str>) -> AudioError {
    let detail = match debug {
        Some(d) if !d.is_empty() => format!("{err} ({d})"),
        _ => err.to_string(),
    };
    if err.kind::<gst::StreamError>().is_some() {
        AudioError::Decode(format!("GStreamer could not decode the source: {detail}"))
    } else {
        AudioError::Backend(format!("GStreamer pipeline error: {detail}"))
    }
}

/// Pop the first `Error` message currently on the pipeline's bus and map it;
/// falls back to a generic `Backend` if the failure left no detail.
fn drain_error(pipeline: &gst::Pipeline) -> AudioError {
    if let Some(bus) = pipeline.bus() {
        // The failing state change posts its ERROR before returning, so a
        // short poll is enough to retrieve it.
        while let Some(msg) =
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(200), &[gst::MessageType::Error])
        {
            if let gst::MessageView::Error(e) = msg.view() {
                return map_gst_error(&e.error(), e.debug().as_deref());
            }
        }
    }
    AudioError::Backend("GStreamer pipeline failed to reach PAUSED (no error detail)".into())
}

/// Build the pipeline and drive it to `PAUSED`, blocking until it either
/// prerolls (decodes the first buffers) or fails. This validates the source
/// **decodes** at `load` time — mapping a bad container to `Decode` and a
/// missing file / unreachable URL / absent device to `Backend` — then tears
/// the probe pipeline down. Each `play()` builds its own fresh pipeline.
fn preroll(kind: &SourceKind, sink_name: &str) -> Result<(), AudioError> {
    let built = build_pipeline(kind, sink_name)?;
    let pipeline = built.pipeline;

    if pipeline.set_state(gst::State::Paused).is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    // Async: block up to 10 s for preroll to complete. Enough for a remote
    // stream to connect + buffer its first data; guards against a hang.
    let (result, _current, _pending) = pipeline.state(gst::ClockTime::from_seconds(10));
    let outcome = match result {
        Ok(_) => Ok(()),
        Err(_) => Err(drain_error(&pipeline)),
    };
    let _ = pipeline.set_state(gst::State::Null);
    outcome
}

/// The bus-message loop that runs on each playback's dedicated thread. Handles
/// EOS (loop-seek or mark ended) and errors without any GLib main loop. Exits
/// when `quit` is set. See the module note on async state changes.
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
        // 100 ms timeout so shutdown is observed promptly; a real message
        // returns immediately, so EOS/loop latency is effectively zero.
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        match msg.view() {
            gst::MessageView::Eos(_) => {
                if looping.load(Ordering::Relaxed) {
                    // Next-boundary loop: flush-seek back to the start and keep
                    // playing. FLUSH discards buffered data so playback resumes
                    // cleanly from 0.
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
                    "audio(linux): playback error: {} ({:?})",
                    e.error(),
                    e.debug()
                );
                ended.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// A prepared sound: just the resolved [`SourceKind`], shared so each `play()`
/// rebuilds an independent pipeline. No live device or pipeline is held here —
/// the `Sound` is cheap, and dropping it never stops an already-started
/// playback (each owns its own pipeline).
pub(crate) struct PreparedSound {
    kind: SourceKind,
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        match PlaybackHandle::start(&self.kind, "autoaudiosink") {
            Ok(handle) => handle,
            Err(e) => {
                // The source prerolled at `prepare` time; a failure here is a
                // rare race (device lost between load and play). Return an
                // inert handle rather than panic, mirroring the other backends.
                eprintln!("audio(linux): play() failed: {e}");
                PlaybackHandle::inert()
            }
        }
    }
}

/// A running playback owning its GStreamer pipeline and bus thread. `Drop`
/// stops the pipeline (state → `Null`) and joins the bus thread — RAII stop.
pub(crate) struct PlaybackHandle {
    /// `None` for an inert handle (device lost at play time).
    pipeline: Option<gst::Pipeline>,
    /// The `volume` element, for dynamic volume; `None` when inert.
    volume: Option<gst::Element>,
    /// Consulted by [`bus_loop`] on EOS — loop-seek iff set. Toggled live.
    looping: Arc<AtomicBool>,
    /// Set by [`bus_loop`] when a non-looping playback reaches EOS (or errors);
    /// makes `is_playing` report `false` after the sound finishes.
    ended: Arc<AtomicBool>,
    /// Signals the bus thread to exit; set on `Drop`.
    quit: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
}

impl PlaybackHandle {
    /// Build a fresh pipeline for one voice, start the bus thread, and begin
    /// playback (`Playing`). The bus thread is spawned *after* the state change
    /// so a synchronous start failure doesn't leak a thread; any messages
    /// posted meanwhile stay queued on the bus for the thread to drain.
    fn start(kind: &SourceKind, sink_name: &str) -> Result<Self, AudioError> {
        let built = build_pipeline(kind, sink_name)?;
        let pipeline = built.pipeline;
        let volume = built.volume;

        let looping = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));

        let bus = pipeline
            .bus()
            .ok_or_else(|| AudioError::Backend("pipeline has no bus".into()))?;

        if pipeline.set_state(gst::State::Playing).is_err() {
            let err = drain_error(&pipeline);
            let _ = pipeline.set_state(gst::State::Null);
            return Err(err);
        }

        let bus_thread = {
            let pipeline = pipeline.clone();
            let looping = looping.clone();
            let ended = ended.clone();
            let quit = quit.clone();
            std::thread::spawn(move || bus_loop(bus, pipeline, looping, ended, quit))
        };

        Ok(Self {
            pipeline: Some(pipeline),
            volume: Some(volume),
            looping,
            ended,
            quit,
            bus_thread: Some(bus_thread),
        })
    }

    /// An inert handle whose controls are no-ops — used when a pipeline
    /// couldn't be built at `play()` time. Never panics, matching the
    /// Apple/Android inert-handle behavior.
    fn inert() -> Self {
        Self {
            pipeline: None,
            volume: None,
            looping: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(true)),
            quit: Arc::new(AtomicBool::new(true)),
            bus_thread: None,
        }
    }
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        // Signal the bus thread first, then tear the pipeline down, then join.
        self.quit.store(true, Ordering::Relaxed);
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        if let Some(thread) = self.bus_thread.take() {
            let _ = thread.join();
        }
    }
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        if let Some(ref pipeline) = self.pipeline {
            let _ = pipeline.set_state(gst::State::Paused);
        }
    }

    pub(crate) fn resume(&self) {
        if let Some(ref pipeline) = self.pipeline {
            let _ = pipeline.set_state(gst::State::Playing);
        }
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        if let Some(ref vol) = self.volume {
            // The `volume` element's `volume` property is a linear gdouble
            // (1.0 = unity); the public wrapper already clamped to [0.0, 1.0].
            vol.set_property("volume", volume as f64);
        }
    }

    pub(crate) fn set_looping(&self, looping: bool) {
        // Consulted by `bus_loop` at the next EOS boundary; the current pass is
        // never interrupted, per the SDK contract.
        self.looping.store(looping, Ordering::Relaxed);
    }

    pub(crate) fn is_playing(&self) -> bool {
        match self.pipeline {
            // Playing == the pipeline's current state is Playing and it hasn't
            // reached a non-looping EOS. While looping, `ended` stays false.
            Some(ref pipeline) => {
                !self.ended.load(Ordering::Relaxed)
                    && pipeline.current_state() == gst::State::Playing
            }
            None => false,
        }
    }
}

/// Prepare a sound: resolve the source, init GStreamer, and validate it
/// **decodes** by prerolling a probe pipeline (eager [`AudioError::Decode`] on
/// bad audio, matching the cross-platform contract that `load` validates the
/// audio; [`AudioError::Backend`] for a missing file / unreachable URL / no
/// output device).
///
/// Remote `http(s)` URLs are **progressively streamed** by `souphttpsrc`
/// (parity with Android `MediaPlayer`, Apple `AVPlayer`, and the browser) —
/// they are no longer read as a local file and no longer return a `Backend`
/// error for being remote. A genuinely unreachable URL still fails honestly
/// as `Backend`.
pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    ensure_gst()?;

    let kind = match source {
        AudioSource::Bytes(bytes) => SourceKind::Bytes(Arc::new(bytes)),
        AudioSource::Path(path) => SourceKind::File(path.to_string_lossy().into_owned()),
        AudioSource::Url(url) => {
            // `file://` → local path; any other scheme (http/https/…) streams.
            if let Some(path) = url.strip_prefix("file://") {
                SourceKind::File(path.to_string())
            } else {
                SourceKind::Http(url)
            }
        }
    };

    // Validate decode now (against the real audio sink, so an absent device is
    // an honest Backend error at load time — same as the prior backend and the
    // mobile players, which also prepare against real output).
    preroll(&kind, "autoaudiosink")?;

    Ok(PreparedSound { kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, valid 16-bit PCM mono WAV (8 kHz, a handful of samples).
    /// Built in-memory so the decode tests need no asset file — they exercise
    /// the real GStreamer decodebin path this backend uses.
    fn tiny_wav() -> Vec<u8> {
        let sample_rate: u32 = 8000;
        let bits_per_sample: u16 = 16;
        let channels: u16 = 1;
        // A short but non-trivial buffer so preroll has real audio to chew on.
        let samples: [i16; 16] = [
            0, 8000, 16000, 8000, 0, -8000, -16000, -8000, 0, 8000, 16000, 8000, 0, -8000, -16000,
            -8000,
        ];

        let block_align = channels * bits_per_sample / 8;
        let byte_rate = sample_rate * block_align as u32;
        let data_len = (samples.len() * 2) as u32;
        let riff_len = 36 + data_len;

        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_len.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        wav
    }

    /// A valid WAV must preroll (decode) to PAUSED against a `fakesink`, with
    /// no audio output device required. This is the headless proof that the
    /// GStreamer decode path works — the core regression guard.
    #[test]
    fn prerolls_valid_wav_without_device() {
        ensure_gst().expect("gst init");
        let kind = SourceKind::Bytes(Arc::new(tiny_wav()));
        preroll(&kind, "fakesink").expect("valid wav must preroll to PAUSED via fakesink");
    }

    /// Garbage bytes must map to a typed `Decode` error and never panic — the
    /// `load_bad_bytes_is_typed_error_not_panic` invariant, exercised headless
    /// (fakesink, no device) so it holds on CI. `decodebin` fails typefinding
    /// on random bytes → `StreamError` → `Decode`.
    #[test]
    fn garbage_bytes_map_to_decode_error() {
        ensure_gst().expect("gst init");
        let kind = SourceKind::Bytes(Arc::new(vec![0u8; 64]));
        match preroll(&kind, "fakesink") {
            Err(AudioError::Decode(_)) => {}
            Err(other) => panic!("expected Decode error for garbage, got {other:?}"),
            Ok(()) => panic!("garbage bytes must not preroll into a playable pipeline"),
        }
    }

    /// The error-domain split must be honest: a `StreamError` (can't decode) →
    /// `Decode`; anything else (`ResourceError` = I/O, missing file, bad URL,
    /// no device) → `Backend`.
    #[test]
    fn error_mapping_splits_stream_vs_resource() {
        let stream = glib::Error::new(gst::StreamError::CodecNotFound, "no codec");
        assert!(
            matches!(map_gst_error(&stream, None), AudioError::Decode(_)),
            "StreamError must map to Decode"
        );

        let resource = glib::Error::new(gst::ResourceError::NotFound, "missing");
        assert!(
            matches!(map_gst_error(&resource, None), AudioError::Backend(_)),
            "ResourceError must map to Backend"
        );

        let core = glib::Error::new(gst::CoreError::Negotiation, "no format");
        assert!(
            matches!(map_gst_error(&core, None), AudioError::Backend(_)),
            "non-stream errors must map to Backend"
        );
    }

    /// End-to-end looping-flag logic, headless via `fakesink` (no device). With
    /// looping ON, the tiny WAV replays forever — the bus thread flush-seeks on
    /// every EOS and `ended` stays false. With looping OFF, EOS marks the
    /// playback ended. This exercises the real `bus_loop` decision.
    #[test]
    fn looping_flag_drives_eos_handling() {
        ensure_gst().expect("gst init");
        let kind = SourceKind::Bytes(Arc::new(tiny_wav()));

        // Non-looping: must reach EOS and mark ended within a short window.
        let once = PlaybackHandle::start(&kind, "fakesink").expect("start non-looping");
        once.set_looping(false);
        let ended = wait_until(&once.ended, true, 3000);
        assert!(ended, "non-looping playback must end at EOS");
        drop(once);

        // Looping: must NOT be ended after several EOS cycles (it keeps
        // seeking back to 0).
        let repeat = PlaybackHandle::start(&kind, "fakesink").expect("start looping");
        repeat.set_looping(true);
        // Give it well past several passes of the tiny buffer.
        let ended = wait_until(&repeat.ended, true, 500);
        assert!(!ended, "looping playback must not end while the flag is set");
        drop(repeat); // RAII stop must not hang or panic.
    }

    /// Spin until `flag` reaches `target` or `timeout_ms` elapses; returns the
    /// final observed value. Polls rather than sleeping a fixed time so the
    /// fast (EOS) case returns promptly.
    fn wait_until(flag: &AtomicBool, target: bool, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();
        loop {
            let v = flag.load(Ordering::Relaxed);
            if v == target || start.elapsed().as_millis() as u64 >= timeout_ms {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Real audible playback needs an output device, which CI hosts lack.
    /// Ignored by default; run with `--ignored` on a machine with ALSA /
    /// PipeWire to hear it. Exercises the full play() → pipeline → Drop path.
    #[test]
    #[ignore = "requires a real audio output device (ALSA/PipeWire)"]
    fn plays_a_wav_end_to_end() {
        ensure_gst().expect("gst init");
        let prepared = PreparedSound {
            kind: SourceKind::Bytes(Arc::new(tiny_wav())),
        };
        let playback = prepared.play();
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(playback); // RAII stop; must not panic.
    }

    /// **Parity proof**: a remote `http(s)` URL must progressively STREAM and
    /// decode. Ignored (needs network); uses `fakesink` so it proves streaming
    /// + decode WITHOUT an audio device. Run with `--ignored` on a networked
    /// host. If this prerolls, remote-URL streaming parity is achieved.
    #[test]
    #[ignore = "requires network access to a public audio URL"]
    fn streams_remote_url() {
        ensure_gst().expect("gst init");
        let kind = SourceKind::Http(
            "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3".to_string(),
        );
        preroll(&kind, "fakesink").expect("remote URL must stream + decode (preroll to PAUSED)");
    }
}
