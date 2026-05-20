//! Audio subsystem for the wgpu desktop preview.
//!
//! One process-wide [`AudioSubsystem`] owns a single `cpal`
//! output stream and an in-process mixer that sums any number of
//! [`AudioSource`]s into the device's frames. Sources are heap-
//! allocated trait objects shared via `Arc<Mutex<…>>` so worker
//! threads (the per-Video AAC decoder, the per-file Symphonia
//! decoder, …) can push samples into them while the audio
//! callback drains.
//!
//! Pipeline:
//!
//! ```text
//!   per-source worker thread  →  bounded ring buffer  →  mixer  →  cpal callback  →  speakers
//!         (decode AAC / decode mp3 / push PCM)           (sum + clamp)
//! ```
//!
//! Master clock: [`AudioSubsystem::position_seconds`] returns
//! `frames_written / output_sample_rate`. The video decoder uses
//! this as its sync reference so audio drives A/V sync; if the
//! mixer falls behind for any reason, video pacing follows along
//! instead of running off to wall-clock time and tearing the lip-
//! sync apart.
//!
//! Out of scope for Phase 1: capture (microphone), per-source
//! resampling for non-48k device formats (we coerce to f32 stereo
//! at 48 kHz internally and let `cpal` resample if the device
//! disagrees; for AAC/MP3 sources that already match this is a
//! no-op).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Internal mixer sample format. Interleaved stereo f32. All
/// sources produce samples in this layout; the cpal callback
/// converts to the device's native format if needed.
pub(crate) const MIXER_CHANNELS: u16 = 2;
/// Target mixer rate. Most consumer hardware natively runs at 48
/// kHz; if the device picks something else, cpal will surface
/// that and we ask the source to render at the device rate
/// directly (no internal resampler — symphonia decodes to the
/// source's native rate, which we resample to device rate in
/// the source's render path via a simple linear step).
pub(crate) const DEFAULT_MIXER_RATE: u32 = 48_000;

// ---------------------------------------------------------------------------
// AudioSource trait
// ---------------------------------------------------------------------------

/// One stream of audio being mixed into the output. Implementors
/// are typically backed by a producer thread (a decode loop, a
/// network reader) that pushes samples into a shared buffer,
/// while `render` is invoked from the cpal audio callback to
/// drain into the device's output frame.
///
/// Contract: `render` MUST NOT block, allocate, or take heavy
/// locks — it's called on the audio thread and stalls there
/// cause audible underruns.
pub trait AudioSource: Send {
    /// Pull up to `out.len()` interleaved samples into `out` at
    /// the given device `channels` / `sample_rate`. Returns the
    /// number of samples written (0 ≤ n ≤ out.len()). Any samples
    /// the source can't fill stay as the caller left them — the
    /// mixer zero-fills before calling render, so unwritten
    /// positions correspond to silence.
    fn render(&mut self, out: &mut [f32], channels: u16, sample_rate: u32) -> usize;

    /// `true` once this source has finished and may be removed
    /// from the mixer. Returning `true` is permanent.
    fn is_finished(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// AudioSubsystem
// ---------------------------------------------------------------------------

/// Per-process audio runtime. The `AudioSubsystem` is reference-
/// counted so source handles (video, file, pcm) can keep it alive
/// for as long as they need to play. The cpal `Stream` is `!Send`
/// on macOS (and others), so it can't sit inside an `Arc` that
/// crosses threads — we pin it to a dedicated owner thread that
/// builds it, calls `play`, and parks forever. The mixer state
/// (`AudioInner`) is `Send + Sync` and shared with the audio
/// callback through an `Arc` clone.
pub struct AudioSubsystem {
    inner: Arc<AudioInner>,
}

struct AudioInner {
    sources: Mutex<Vec<SourceSlot>>,
    /// Monotonic frame count written into the device output.
    /// Updated by the cpal callback. Read by sync logic to derive
    /// playback position.
    frames_written: AtomicU64,
    /// Device-reported sample rate. Sources are told this each
    /// `render` call so they resample / step correctly.
    sample_rate: u32,
    /// Device-reported channel count (1 = mono, 2 = stereo).
    channels: u16,
}

struct SourceSlot {
    id: u64,
    source: Box<dyn AudioSource>,
}

/// Opaque handle to a source registered with the subsystem.
/// Used to remove or to query whether the slot still exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub(crate) u64);

impl AudioSubsystem {
    /// Build a subsystem against the default output device. The
    /// cpal stream lives on a dedicated owner thread (because
    /// `cpal::Stream` is `!Send`); this call blocks until the
    /// thread has either opened the stream successfully (returns
    /// `Ok`) or failed (returns `Err`).
    pub fn new() -> Result<Arc<Self>, String> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<Arc<AudioInner>, String>>();
        std::thread::Builder::new()
            .name("audio-owner".into())
            .spawn(move || {
                match build_stream() {
                    Ok((inner, stream)) => {
                        // Send the mixer state back, then park
                        // forever holding the stream. The stream
                        // can't move off this thread, so the
                        // thread must outlive it (which is the
                        // process lifetime since we never tear
                        // down audio in Phase 1).
                        let _ = tx.send(Ok(inner));
                        std::thread::park();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| format!("spawn audio-owner thread: {e}"))?;
        let inner = rx
            .recv()
            .map_err(|e| format!("audio-owner thread died: {e}"))??;
        Ok(Arc::new(Self { inner }))
    }

    /// Mixer-side sample rate. Audio sources can use this to
    /// pre-allocate their resampler step / ring buffers.
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate
    }

    /// Mixer-side channel count (1 or 2).
    pub fn channels(&self) -> u16 {
        self.inner.channels
    }

    /// Master clock in seconds since the subsystem started. The
    /// video decoder uses this as its pacing reference (audio
    /// master clock sync).
    pub fn position_seconds(&self) -> f64 {
        self.inner.frames_written.load(Ordering::Acquire) as f64
            / self.inner.sample_rate.max(1) as f64
    }

    /// Register a source. The mixer will call its `render` on the
    /// audio thread until it reports `is_finished`.
    pub fn add_source(&self, source: Box<dyn AudioSource>) -> SourceId {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = SourceId(NEXT.fetch_add(1, Ordering::Relaxed));
        if let Ok(mut sources) = self.inner.sources.lock() {
            sources.push(SourceSlot { id: id.0, source });
        }
        id
    }

    /// Drop a previously-added source. No-op if the id is unknown
    /// (already finished and reaped, or never registered).
    pub fn remove_source(&self, id: SourceId) {
        if let Ok(mut sources) = self.inner.sources.lock() {
            sources.retain(|s| s.id != id.0);
        }
    }
}

/// Open the default output device and build the stream. Lives in
/// its own fn so the audio-owner thread can drive it without
/// dragging the surrounding subsystem types into a `!Send`
/// closure.
fn build_stream() -> Result<(Arc<AudioInner>, cpal::Stream), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e:?}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let inner = Arc::new(AudioInner {
        sources: Mutex::new(Vec::new()),
        frames_written: AtomicU64::new(0),
        sample_rate,
        channels,
    });
    let err_fn = |e| eprintln!("[audio] stream error: {e:?}");
    let stream_inner = inner.clone();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _| audio_callback_f32(&stream_inner, data),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let inner2 = stream_inner.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    thread_local! {
                        static SCRATCH: std::cell::RefCell<Vec<f32>> =
                            std::cell::RefCell::new(Vec::new());
                    }
                    SCRATCH.with(|s| {
                        let mut s = s.borrow_mut();
                        s.resize(data.len(), 0.0);
                        audio_callback_f32(&inner2, &mut s);
                        for (d, src) in data.iter_mut().zip(s.iter()) {
                            *d = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    });
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let inner2 = stream_inner.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    thread_local! {
                        static SCRATCH: std::cell::RefCell<Vec<f32>> =
                            std::cell::RefCell::new(Vec::new());
                    }
                    SCRATCH.with(|s| {
                        let mut s = s.borrow_mut();
                        s.resize(data.len(), 0.0);
                        audio_callback_f32(&inner2, &mut s);
                        for (d, src) in data.iter_mut().zip(s.iter()) {
                            let f = src.clamp(-1.0, 1.0);
                            *d = ((f * 0.5 + 0.5) * u16::MAX as f32) as u16;
                        }
                    });
                },
                err_fn,
                None,
            )
        }
        fmt => return Err(format!("unsupported sample format: {fmt:?}")),
    }
    .map_err(|e| format!("build_output_stream: {e:?}"))?;
    stream
        .play()
        .map_err(|e| format!("stream.play(): {e:?}"))?;
    eprintln!(
        "[audio] opened output: {} ch, {} Hz, {:?}",
        channels, sample_rate, sample_format
    );
    Ok((inner, stream))
}

// ---------------------------------------------------------------------------
// Audio callback
// ---------------------------------------------------------------------------

fn audio_callback_f32(inner: &AudioInner, data: &mut [f32]) {
    // Zero-fill first; each source adds into the buffer. Sources
    // that don't fill all the way leave silence in the tail.
    for x in data.iter_mut() {
        *x = 0.0;
    }
    if let Ok(mut sources) = inner.sources.lock() {
        // Scratch buffer per render so we can clamp the sum at
        // the end. Same trick as the i16/u16 conversion path.
        thread_local! {
            static SCRATCH: std::cell::RefCell<Vec<f32>> =
                std::cell::RefCell::new(Vec::new());
        }
        SCRATCH.with(|s| {
            let mut s = s.borrow_mut();
            s.resize(data.len(), 0.0);
            sources.retain_mut(|slot| {
                for x in s.iter_mut() {
                    *x = 0.0;
                }
                slot.source.render(&mut s, inner.channels, inner.sample_rate);
                for (d, x) in data.iter_mut().zip(s.iter()) {
                    *d += *x;
                }
                !slot.source.is_finished()
            });
        });
        // Final clamp so summed sources don't clip the device.
        // Soft-limit would be nicer, but for Phase 1 hard clip is
        // fine; with typical content one source dominates.
        for x in data.iter_mut() {
            *x = x.clamp(-1.0, 1.0);
        }
    }
    let frames = (data.len() / inner.channels.max(1) as usize) as u64;
    inner.frames_written.fetch_add(frames, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Process-wide singleton
// ---------------------------------------------------------------------------

/// Global subsystem accessor. Initialized lazily on first
/// `audio_subsystem()` call so apps that don't use audio don't
/// pay the cpal startup cost. The `OnceLock<Result<…>>` shape
/// lets us cache failure (no device, no permissions) without
/// retrying every frame.
static SUBSYS: OnceLock<Result<Arc<AudioSubsystem>, String>> = OnceLock::new();

/// Get (or initialize) the process-wide audio subsystem.
/// Returns `Err` if the OS audio device couldn't be opened —
/// callers that need audio should surface this; callers that
/// just want sound-if-available can `.ok()` it.
pub fn subsystem() -> Result<Arc<AudioSubsystem>, String> {
    SUBSYS.get_or_init(AudioSubsystem::new).clone()
}

// ---------------------------------------------------------------------------
// Shared ring buffer used by all decode-into-mixer sources
// ---------------------------------------------------------------------------

/// Bounded FIFO of interleaved f32 samples. Decoder thread pushes;
/// mixer callback pops. Capacity is in samples (not frames); use
/// `frames * channels` when sizing.
pub(crate) struct SampleRing {
    inner: Mutex<RingInner>,
    /// Set by the producer when the stream has ended and no more
    /// samples will be pushed. The mixer drains the remainder and
    /// then reports the source finished.
    pub(crate) producer_done: AtomicBool,
    /// Set by the holder to mute output without dropping the
    /// source. Sample data still flows so the master clock keeps
    /// advancing; the mixer just zeroes the output.
    pub(crate) muted: AtomicBool,
    /// Set when the holder wants playback suspended. Producer
    /// thread should check this and park; mixer outputs silence.
    pub(crate) paused: AtomicBool,
}

struct RingInner {
    samples: VecDeque<f32>,
    capacity: usize,
}

impl SampleRing {
    pub(crate) fn new(capacity_samples: usize) -> Self {
        Self {
            inner: Mutex::new(RingInner {
                samples: VecDeque::with_capacity(capacity_samples),
                capacity: capacity_samples,
            }),
            producer_done: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        }
    }

    /// Push samples. Drops oldest samples if buffer is full —
    /// "always favor the latest" matches video's slot behavior
    /// (a fallen-behind consumer would otherwise drift forever).
    pub(crate) fn push(&self, samples: &[f32]) {
        if let Ok(mut g) = self.inner.lock() {
            let cap = g.capacity;
            if samples.len() >= cap {
                g.samples.clear();
                g.samples.extend(samples[samples.len() - cap..].iter().copied());
            } else {
                let needed = samples.len();
                let have = g.samples.len();
                if have + needed > cap {
                    let drop = (have + needed) - cap;
                    for _ in 0..drop {
                        g.samples.pop_front();
                    }
                }
                g.samples.extend(samples.iter().copied());
            }
        }
    }

    /// Drain up to `out.len()` samples into `out`. Returns the
    /// number written. Unwritten positions are left as-is —
    /// caller pre-zeroes if it wants silence.
    pub(crate) fn pop_into(&self, out: &mut [f32]) -> usize {
        let Ok(mut g) = self.inner.lock() else { return 0 };
        let n = g.samples.len().min(out.len());
        for slot in out.iter_mut().take(n) {
            *slot = g.samples.pop_front().unwrap_or(0.0);
        }
        n
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.lock().map(|g| g.samples.len()).unwrap_or(0)
    }

    /// Drop every buffered sample. Used on seek so the mixer
    /// stops playing pre-seek audio while the producer thread
    /// catches up to the new position.
    pub(crate) fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.samples.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Standard ring-backed source
// ---------------------------------------------------------------------------

/// Audio source backed by a producer-fed [`SampleRing`]. All three
/// of {VideoAudioSource, FileAudioSource, PcmSource} share this
/// shape — they only differ in what feeds the ring.
pub(crate) struct RingSource {
    pub(crate) ring: Arc<SampleRing>,
    pub(crate) volume: Arc<Mutex<f32>>,
    /// Sample rate of the data the producer pushes. The render
    /// path linearly resamples to the device rate.
    pub(crate) source_rate: u32,
    /// Channel count the producer pushes (interleaved). Mono is
    /// upmixed to stereo by duplicating; stereo with a mono
    /// device is downmixed by averaging.
    pub(crate) source_channels: u16,
    /// Resampler phase: fractional source-sample index. Persists
    /// across `render` calls so the resampler stays continuous.
    phase: f64,
}

impl RingSource {
    pub(crate) fn new(
        ring: Arc<SampleRing>,
        source_rate: u32,
        source_channels: u16,
        volume: Arc<Mutex<f32>>,
    ) -> Self {
        Self {
            ring,
            volume,
            source_rate,
            source_channels,
            phase: 0.0,
        }
    }
}

impl AudioSource for RingSource {
    fn render(&mut self, out: &mut [f32], device_channels: u16, device_rate: u32) -> usize {
        if self.ring.paused.load(Ordering::Acquire) {
            return 0;
        }
        let muted = self.ring.muted.load(Ordering::Acquire);
        let vol = *self.volume.lock().unwrap_or_else(|p| p.into_inner());
        // Resample ratio: source samples consumed per device sample.
        let step = self.source_rate as f64 / device_rate.max(1) as f64;
        let dch = device_channels.max(1) as usize;
        let sch = self.source_channels.max(1) as usize;
        let device_frames = out.len() / dch;
        // We need at most `phase.ceil() + device_frames * step`
        // source frames available to fill the full output buffer.
        // Pull that worth of samples into a scratch buffer; if
        // the ring is short, fill only what we can.
        let source_frames_needed =
            (self.phase + device_frames as f64 * step).ceil() as usize + 2;
        let source_samples_needed = source_frames_needed * sch;
        thread_local! {
            static SCRATCH: std::cell::RefCell<Vec<f32>> = std::cell::RefCell::new(Vec::new());
        }
        SCRATCH.with(|s| {
            let mut s = s.borrow_mut();
            s.resize(source_samples_needed, 0.0);
            let got = self.ring.pop_into(&mut s);
            let source_frames_got = got / sch;
            // How many device frames can we actually fill given
            // what's in `s`? Last device frame f maps to source
            // index phase + f * step; we need that to be < got_frames.
            let max_device_frames = if step > 0.0 {
                ((source_frames_got as f64 - self.phase - 1.0) / step).floor() as isize
            } else {
                device_frames as isize
            };
            let device_frames_filled = max_device_frames
                .max(0)
                .min(device_frames as isize) as usize;
            for f in 0..device_frames_filled {
                let src_idx = self.phase + f as f64 * step;
                let i0 = src_idx.floor() as usize;
                let i1 = (i0 + 1).min(source_frames_got.saturating_sub(1));
                let frac = (src_idx - src_idx.floor()) as f32;
                // Linear interp + channel mixdown. Cheap and
                // good-enough for typical content; if we ever
                // need pitch-correct, swap in a windowed-sinc
                // resampler here.
                let (l_s, r_s) = sample_frame_lr(&s, i0, sch);
                let (l_n, r_n) = sample_frame_lr(&s, i1, sch);
                let l = (l_s * (1.0 - frac) + l_n * frac) * vol;
                let r = (r_s * (1.0 - frac) + r_n * frac) * vol;
                let out_base = f * dch;
                if muted {
                    // Skip writing — `out` is already zero from
                    // the mixer's pre-fill.
                    continue;
                }
                match device_channels {
                    1 => out[out_base] += (l + r) * 0.5,
                    2 => {
                        out[out_base] += l;
                        out[out_base + 1] += r;
                    }
                    n => {
                        // Multichannel device — duplicate L into
                        // even indices and R into odd. Surround
                        // expansion can wait.
                        for c in 0..n as usize {
                            out[out_base + c] += if c % 2 == 0 { l } else { r };
                        }
                    }
                }
            }
            // Advance phase by what we *consumed*; the resampler
            // produced `device_frames_filled` device frames, each
            // worth `step` source frames.
            self.phase += device_frames_filled as f64 * step;
            // Reset phase to start of next "batch" so we don't
            // keep growing it across calls — drop everything we
            // fully consumed.
            let consumed_full = self.phase.floor() as usize;
            self.phase -= consumed_full as f64;
            // The next call needs to re-fetch from where we left
            // off. Anything in `s` we didn't use is discarded —
            // it was popped from the ring already, so it's lost.
            // To avoid losing samples, we re-push the unused tail
            // back into the ring's front via a small unconsume.
            let to_unconsume = source_frames_got.saturating_sub(consumed_full);
            if to_unconsume > 0 {
                let tail_start = consumed_full * sch;
                let tail_end = tail_start + to_unconsume * sch;
                if tail_end <= s.len() {
                    self.ring.unconsume_front(&s[tail_start..tail_end]);
                }
            }
            device_frames_filled * dch
        })
    }

    fn is_finished(&self) -> bool {
        self.ring.producer_done.load(Ordering::Acquire) && self.ring.len() == 0
    }
}

impl SampleRing {
    /// Push samples to the FRONT of the ring. Used by the
    /// resampler to return the un-consumed tail it pulled but
    /// didn't end up using this callback. Cheap because typical
    /// `tail.len()` is < 4 samples.
    pub(crate) fn unconsume_front(&self, tail: &[f32]) {
        if let Ok(mut g) = self.inner.lock() {
            // Push in reverse so order is preserved.
            for &s in tail.iter().rev() {
                g.samples.push_front(s);
            }
            // Drop overflow from the back if we exceeded capacity.
            while g.samples.len() > g.capacity {
                g.samples.pop_back();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Symphonia-backed source: video AAC track + standalone files
// ---------------------------------------------------------------------------

/// Spawn a decoder thread that reads an audio track from an mp4
/// or any symphonia-supported container/codec, decodes it to f32
/// PCM, and pushes into a [`SampleRing`]. The ring is registered
/// with `subsystem` as a [`RingSource`].
///
/// Returns `None` if the bytes don't contain a decodable audio
/// track (no audio, codec not enabled, parse failure). The
/// decoder thread is detached — when the returned handle's
/// `ring.producer_done` is set the source self-removes from the
/// mixer.
pub(crate) struct DecodedAudioHandle {
    pub(crate) ring: Arc<SampleRing>,
    pub(crate) volume: Arc<Mutex<f32>>,
    pub(crate) source_id: SourceId,
    pub(crate) subsystem: Arc<AudioSubsystem>,
    /// Set true to ask the decoder thread to exit promptly.
    pub(crate) shutdown: Arc<AtomicBool>,
    /// `Some(secs)` requests the decoder thread jump to that
    /// playback time and flush the ring. Cleared after the
    /// seek lands. Mirrors the video decoder's `seek_request`
    /// so a `VideoDecoder::seek` keeps A/V in sync.
    pub(crate) seek_request: Arc<Mutex<Option<f64>>>,
}

impl DecodedAudioHandle {
    /// Ask the decode thread to seek to `target_secs` from the
    /// start of the clip. The thread flushes any buffered samples
    /// before resuming so the user hears the new position
    /// promptly (~one ring's worth of latency, ≤ 500 ms).
    pub(crate) fn request_seek(&self, target_secs: f64) {
        if let Ok(mut g) = self.seek_request.lock() {
            *g = Some(target_secs.max(0.0));
        }
        // Drain the ring up-front so the mixer plays silence
        // immediately rather than a couple of stale frames while
        // the decode thread catches up.
        self.ring.clear();
    }
}

impl Drop for DecodedAudioHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.ring.paused.store(false, Ordering::Release);
        self.subsystem.remove_source(self.source_id);
    }
}

/// Decode any symphonia-supported byte source into a ring-fed
/// audio source registered on the subsystem. `initial_volume` is
/// `0.0..=1.0` (1.0 = unity). `looping = true` rewinds at EOF.
pub(crate) fn spawn_decoded_source(
    bytes: Arc<Vec<u8>>,
    hint_ext: Option<&str>,
    subsystem: Arc<AudioSubsystem>,
    initial_volume: f32,
    looping: bool,
) -> Option<DecodedAudioHandle> {
    use symphonia::core::probe::Hint;

    let mut hint = Hint::new();
    if let Some(ext) = hint_ext {
        hint.with_extension(ext);
    }
    // Quick probe just to verify the bytes are something
    // symphonia can read AND that it has at least one audio
    // track in a codec we enabled. If the probe fails we
    // return None up-front so the caller can omit audio
    // without spinning up a thread.
    let probe_bytes = bytes.clone();
    let cursor = std::io::Cursor::new(probe_bytes.as_ref().clone());
    let mss = symphonia::core::io::MediaSourceStream::new(
        Box::new(cursor),
        Default::default(),
    );
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .ok()?;
    // `default_track()` defers to whatever the container marks as
    // default — for a video+audio mp4 that's typically the video
    // track, which has no `sample_rate`. Pick the first track
    // that actually looks like audio.
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some())?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate?;
    let channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);
    eprintln!(
        "[audio] decoded source: track #{} {} Hz, {} ch, codec {:?}",
        track_id, sample_rate, channels, track.codec_params.codec
    );

    // Ring sized to ~500ms of audio at the source rate; decoder
    // blocks (via sleep) when it gets ahead of that.
    let ring_capacity = (sample_rate as usize * channels as usize) / 2;
    let ring = Arc::new(SampleRing::new(ring_capacity));
    let volume = Arc::new(Mutex::new(initial_volume.clamp(0.0, 1.0)));
    let shutdown = Arc::new(AtomicBool::new(false));

    let source = Box::new(RingSource::new(
        ring.clone(),
        sample_rate,
        channels,
        volume.clone(),
    ));
    let source_id = subsystem.add_source(source);

    let ring_for_thread = ring.clone();
    let shutdown_for_thread = shutdown.clone();
    let hint_ext = hint_ext.map(|s| s.to_string());
    std::thread::Builder::new()
        .name("audio-decode".into())
        .spawn(move || {
            decoded_decode_loop(
                bytes,
                hint_ext,
                ring_for_thread,
                shutdown_for_thread,
                looping,
            );
        })
        .ok()?;

    Some(DecodedAudioHandle {
        ring,
        volume,
        source_id,
        subsystem,
        shutdown,
    })
}

fn decoded_decode_loop(
    bytes: Arc<Vec<u8>>,
    hint_ext: Option<String>,
    ring: Arc<SampleRing>,
    shutdown: Arc<AtomicBool>,
    looping: bool,
) {
    use symphonia::core::audio::{AudioBufferRef, Signal};
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    'outer: loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let mut hint = Hint::new();
        if let Some(ext) = hint_ext.as_deref() {
            hint.with_extension(ext);
        }
        let cursor = std::io::Cursor::new(bytes.as_ref().clone());
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
        let probed = match symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[audio] probe failed: {e:?}");
                return;
            }
        };
        let mut format = probed.format;
        let track = match format
            .tracks()
            .iter()
            .find(|t| t.codec_params.sample_rate.is_some())
        {
            Some(t) => t,
            None => return,
        };
        let track_id = track.id;
        let mut decoder = match symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[audio] make decoder: {e:?}");
                return;
            }
        };
        let source_channels = track
            .codec_params
            .channels
            .map(|c| c.count())
            .unwrap_or(2);

        loop {
            if shutdown.load(Ordering::Acquire) {
                break 'outer;
            }
            // Back-pressure: if the ring is more than ~70% full,
            // the mixer is behind. Sleep briefly so the decoder
            // doesn't blow past it and force the "drop oldest"
            // behavior — that would tear A/V sync apart on
            // catch-up.
            while ring.len() > ring_capacity_hint(&ring) * 7 / 10 {
                if shutdown.load(Ordering::Acquire) {
                    break 'outer;
                }
                if ring.paused.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // End of stream.
                    if looping {
                        continue 'outer;
                    }
                    ring.producer_done.store(true, Ordering::Release);
                    break 'outer;
                }
                Err(SymError::ResetRequired) => {
                    // The stream changed config; loop back and
                    // re-probe.
                    continue 'outer;
                }
                Err(e) => {
                    eprintln!("[audio] next_packet: {e:?}");
                    ring.producer_done.store(true, Ordering::Release);
                    break 'outer;
                }
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(e)) => {
                    eprintln!("[audio] decode error: {e:?}");
                    continue;
                }
                Err(e) => {
                    eprintln!("[audio] decode fatal: {e:?}");
                    ring.producer_done.store(true, Ordering::Release);
                    break 'outer;
                }
            };
            // Convert to interleaved f32, regardless of the
            // decoder's native sample format.
            let interleaved = audio_buffer_to_interleaved_f32(decoded, source_channels);
            ring.push(&interleaved);
        }
    }
}

fn audio_buffer_to_interleaved_f32(
    buf: symphonia::core::audio::AudioBufferRef<'_>,
    channels: usize,
) -> Vec<f32> {
    use symphonia::core::audio::AudioBufferRef::*;
    use symphonia::core::audio::Signal;
    use symphonia::core::conv::IntoSample;

    let frames = buf.frames();
    let mut out = vec![0.0_f32; frames * channels];
    macro_rules! pack {
        ($b:expr, $convert:ident) => {{
            let b = $b;
            let chans = b.spec().channels.count().min(channels);
            for ch in 0..chans {
                let plane = b.chan(ch);
                for (i, s) in plane.iter().enumerate() {
                    out[i * channels + ch] = (*s).$convert();
                }
            }
            // If decoder produced fewer channels than declared,
            // duplicate the last channel into the rest. Common
            // case is mono → both stereo channels.
            if chans < channels {
                for i in 0..frames {
                    let last = out[i * channels + chans - 1];
                    for ch in chans..channels {
                        out[i * channels + ch] = last;
                    }
                }
            }
        }};
    }
    match buf {
        F32(b) => pack!(b, into_sample),
        F64(b) => pack!(b, into_sample),
        S8(b) => pack!(b, into_sample),
        S16(b) => pack!(b, into_sample),
        S24(b) => pack!(b, into_sample),
        S32(b) => pack!(b, into_sample),
        U8(b) => pack!(b, into_sample),
        U16(b) => pack!(b, into_sample),
        U24(b) => pack!(b, into_sample),
        U32(b) => pack!(b, into_sample),
    }
    out
}

fn ring_capacity_hint(ring: &SampleRing) -> usize {
    ring.inner.lock().map(|g| g.capacity).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Programmatic PCM source — app code pushes samples
// ---------------------------------------------------------------------------

/// Handle for a programmatic audio stream where the app code
/// pushes raw f32 PCM via [`Self::push_samples`]. Useful for
/// generated audio, TTS streaming, tone generators.
pub struct PcmStream {
    ring: Arc<SampleRing>,
    volume: Arc<Mutex<f32>>,
    source_id: SourceId,
    subsystem: Arc<AudioSubsystem>,
}

impl PcmStream {
    /// Open a programmatic source. `sample_rate` and `channels`
    /// describe the layout of samples the app will push. The
    /// mixer resamples / channel-maps automatically.
    pub fn new(
        subsystem: Arc<AudioSubsystem>,
        sample_rate: u32,
        channels: u16,
        initial_volume: f32,
    ) -> Self {
        // ~500ms at the source rate. Larger buffer = more latency
        // but more tolerance for bursty pushes.
        let ring_capacity = (sample_rate as usize * channels as usize) / 2;
        let ring = Arc::new(SampleRing::new(ring_capacity));
        let volume = Arc::new(Mutex::new(initial_volume.clamp(0.0, 1.0)));
        let source = Box::new(RingSource::new(
            ring.clone(),
            sample_rate,
            channels,
            volume.clone(),
        ));
        let source_id = subsystem.add_source(source);
        Self { ring, volume, source_id, subsystem }
    }

    /// Push interleaved f32 samples. If the ring is at capacity
    /// the oldest samples are dropped — generated audio is
    /// usually fine with this since it's produced in lockstep
    /// with the consumer.
    pub fn push_samples(&self, samples: &[f32]) {
        self.ring.push(samples);
    }

    pub fn set_volume(&self, vol: f32) {
        if let Ok(mut v) = self.volume.lock() {
            *v = vol.clamp(0.0, 1.0);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.ring.muted.store(muted, Ordering::Release);
    }

    pub fn set_paused(&self, paused: bool) {
        self.ring.paused.store(paused, Ordering::Release);
    }
}

impl Drop for PcmStream {
    fn drop(&mut self) {
        self.subsystem.remove_source(self.source_id);
    }
}

fn sample_frame_lr(samples: &[f32], frame_idx: usize, channels: usize) -> (f32, f32) {
    let base = frame_idx * channels;
    if base >= samples.len() {
        return (0.0, 0.0);
    }
    match channels {
        1 => {
            let s = samples[base];
            (s, s)
        }
        2 => (samples[base], samples[base + 1]),
        n => {
            // 5.1, 7.1: take front L/R; everything else is dropped
            // for now. Proper downmix matrix is a follow-up.
            let l = samples[base];
            let r = if n >= 2 { samples[base + 1] } else { l };
            (l, r)
        }
    }
}
