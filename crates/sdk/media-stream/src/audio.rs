//! A platform-agnostic handle to a live *audio* source — the peer of
//! [`MediaStream`](crate::MediaStream).
//!
//! Why a dedicated abstraction rather than raw PCM callbacks? For video the
//! motive is zero-copy economics (a 1080p frame is megabytes; copying every
//! one melts the device), so a `MediaStream` wraps an opaque `native_source`
//! a same-platform consumer binds to without copying. Audio buffers are tiny
//! by comparison — copying is free — so that's *not* why `AudioStream`
//! exists. Its motive is **playback / transport delegation**: handing the
//! platform's native audio pipeline (a web `MediaStream` feeding an
//! `<audio>`/Web Audio graph, an iOS `AVAudioEngine`, an Android
//! `AudioTrack`) the buffering, decode, device-output routing, and clock-sync
//! it already does well, instead of reconstructing all of it from raw PCM.
//!
//! The *shape* mirrors [`MediaStream`](crate::MediaStream) exactly:
//!
//! - **CPU sample tap** — [`AudioStream::subscribe`] delivers normalized
//!   interleaved-`f32` [`AudioFrame`]s on a callback; [`AudioStream::latest`]
//!   polls the most recent chunk.
//! - **Native source** — [`AudioStream::native_source`] returns an opaque
//!   `Rc<dyn Any>` a same-platform playback layer downcasts (web
//!   `MediaStream` with an audio track, an Apple `AVAudioNode`, an Android
//!   `AudioTrack` feed). Authors never touch it.
//!
//! A capture backend builds both halves with [`AudioStream::new`], hands the
//! `Send` [`AudioWriter`] to its audio thread, and attaches a stopper. The
//! file-writer SDK consumes an `AudioStream` (CPU tap) alongside a
//! `MediaStream`, aligning the two by their shared-[`clock`](crate::clock)
//! `pts_micros` to mux lip-synced audio + video to a file.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::clock;

// ---------------------------------------------------------------------------
// Frame types — the normalized currency every audio producer emits.
// ---------------------------------------------------------------------------

/// The sample rate + channel layout of an audio stream. `Copy`, so a consumer
/// can stash it cheaply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    /// Sample rate in Hz (e.g. `44_100`, `48_000`).
    pub sample_rate: u32,
    /// Channel count (`1` = mono, `2` = stereo, …).
    pub channels: u16,
}

/// One chunk of captured PCM, borrowed for the duration of a
/// [`subscribe`](AudioStream::subscribe) callback (or copied out of
/// [`latest`](AudioStream::latest)).
///
/// Samples are **normalized `f32` in `[-1.0, 1.0]`**, **interleaved** by
/// channel (`L R L R …` for stereo) — the same shape `microphone`'s
/// `AudioBuffer` uses, so every producer converts its native format into this
/// one layout and consumer code is identical everywhere.
///
/// [`pts_micros`](Self::pts_micros) places the chunk on the shared
/// [`clock`](crate::clock) timeline so a muxer can lip-sync it against video.
pub struct AudioFrame<'a> {
    /// Interleaved, normalized `f32` PCM for this chunk.
    pub samples: &'a [f32],
    /// Sample rate of these samples, in Hz (authoritative — what the device
    /// produced).
    pub sample_rate: u32,
    /// Channel count of these samples. `samples.len() == frame_count * channels`.
    pub channels: u16,
    /// Capture timestamp on the shared [`clock`](crate::clock) timeline, in
    /// microseconds — the start of this chunk.
    pub pts_micros: u64,
}

impl AudioFrame<'_> {
    /// Number of sample frames (one frame = one sample per channel).
    pub fn frame_count(&self) -> usize {
        let ch = self.channels.max(1) as usize;
        self.samples.len() / ch
    }

    /// Wall-clock duration this chunk represents, in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / self.sample_rate as f64
    }

    /// The chunk's [`AudioFormat`].
    pub fn format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }
}

// ---------------------------------------------------------------------------
// The callback bound. Mirrors `MediaStream`'s `FrameCallback`: producers
// deliver chunks on an audio thread on native/Android (so `Send`) and on the
// main thread on web (so not `Send`).
// ---------------------------------------------------------------------------

/// The bound a [`subscribe`](AudioStream::subscribe) callback must satisfy.
/// Implemented automatically for any matching closure. `Send` on native /
/// Android (the callback runs on an audio thread), not on web (main thread).
#[cfg(not(target_arch = "wasm32"))]
pub trait AudioFrameCallback: FnMut(&AudioFrame) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: FnMut(&AudioFrame) + Send + 'static> AudioFrameCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait AudioFrameCallback: FnMut(&AudioFrame) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: FnMut(&AudioFrame) + 'static> AudioFrameCallback for T {}

#[cfg(not(target_arch = "wasm32"))]
type BoxedAudioCallback = Box<dyn FnMut(&AudioFrame) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
type BoxedAudioCallback = Box<dyn FnMut(&AudioFrame) + 'static>;

// ---------------------------------------------------------------------------
// AudioChannel — the Send half. The audio thread writes; consumers read
// (latest-chunk pull) or are fanned out to (subscribe push).
// ---------------------------------------------------------------------------

struct OwnedAudio {
    samples: Vec<f32>,
    format: AudioFormat,
    pts_micros: u64,
}

#[derive(Default)]
struct AudioChannel {
    latest: Mutex<Option<OwnedAudio>>,
    generation: AtomicU64,
    subscribers: Mutex<Vec<(u64, BoxedAudioCallback)>>,
    next_sub_id: AtomicU64,
}

impl AudioChannel {
    // Single-producer, mirroring `FrameChannel::write`: reuse the previous
    // chunk's allocation, fill it, fan out to push subscribers WITHOUT holding
    // the `latest` lock (so a subscriber may call `latest()`), then store it.
    fn write(&self, format: AudioFormat, pts_micros: u64, samples: &[f32]) {
        if format.sample_rate == 0 || format.channels == 0 || samples.is_empty() {
            return;
        }
        let mut buf = self
            .latest
            .lock()
            .unwrap()
            .take()
            .map(|a| a.samples)
            .unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(samples);

        {
            let view = AudioFrame {
                samples: &buf,
                sample_rate: format.sample_rate,
                channels: format.channels,
                pts_micros,
            };
            let mut subs = self.subscribers.lock().unwrap();
            for (_, cb) in subs.iter_mut() {
                cb(&view);
            }
        }

        *self.latest.lock().unwrap() = Some(OwnedAudio {
            samples: buf,
            format,
            pts_micros,
        });
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn add_subscriber(&self, cb: BoxedAudioCallback) -> u64 {
        let id = self.next_sub_id.fetch_add(1, Ordering::Relaxed);
        self.subscribers.lock().unwrap().push((id, cb));
        id
    }

    fn remove_subscriber(&self, id: u64) {
        self.subscribers.lock().unwrap().retain(|(i, _)| *i != id);
    }
}

// ---------------------------------------------------------------------------
// AudioWriter — the producer's `Send` push handle (held by the audio thread).
// ---------------------------------------------------------------------------

/// The producer side of an [`AudioStream`]: the audio thread pushes PCM
/// chunks through it. `Send` + cheap to clone.
#[derive(Clone)]
pub struct AudioWriter {
    channel: Arc<AudioChannel>,
}

impl AudioWriter {
    /// Push a chunk of interleaved, normalized `f32` PCM, stamped with the
    /// current [`clock::now_micros`]. Empty chunks and zero formats are
    /// ignored.
    pub fn write_pcm_f32(&self, sample_rate: u32, channels: u16, samples: &[f32]) {
        self.write_pcm_f32_at(sample_rate, channels, samples, clock::now_micros());
    }

    /// Like [`write_pcm_f32`](Self::write_pcm_f32) but with an explicit
    /// capture timestamp (microseconds on the shared [`clock`] timeline) for
    /// the *start* of the chunk — use it when the producer has a real
    /// hardware audio timestamp.
    pub fn write_pcm_f32_at(
        &self,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
        pts_micros: u64,
    ) {
        let format = AudioFormat {
            sample_rate,
            channels,
        };
        self.channel.write(format, pts_micros, samples);
    }
}

// ---------------------------------------------------------------------------
// AudioStream — the !Send, main-thread consumer handle.
// ---------------------------------------------------------------------------

struct AudioStreamInner {
    channel: Arc<AudioChannel>,
    native: RefCell<Option<Rc<dyn Any>>>,
    stopper: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Drop for AudioStreamInner {
    fn drop(&mut self) {
        if let Some(stop) = self.stopper.borrow_mut().take() {
            stop();
        }
    }
}

/// A cloneable, platform-agnostic handle to a live audio source — the peer of
/// [`MediaStream`](crate::MediaStream). Capture runs while any clone is alive;
/// the last drop stops it.
///
/// Not `Send` — it holds the main-thread native source and capture lifecycle.
/// The `Send` push side is [`AudioWriter`], handed to the audio thread.
#[derive(Clone)]
pub struct AudioStream {
    inner: Rc<AudioStreamInner>,
}

impl AudioStream {
    /// Create a stream and its producer [`AudioWriter`].
    #[allow(clippy::new_without_default)]
    pub fn new() -> (AudioStream, AudioWriter) {
        let channel = Arc::new(AudioChannel::default());
        let stream = AudioStream {
            inner: Rc::new(AudioStreamInner {
                channel: channel.clone(),
                native: RefCell::new(None),
                stopper: RefCell::new(None),
            }),
        };
        (stream, AudioWriter { channel })
    }

    /// Record the platform's native audio source (e.g. a web `MediaStream`
    /// with an audio track) for a same-platform playback layer to downcast
    /// via [`native_source`](Self::native_source).
    pub fn set_native_source(&self, src: Rc<dyn Any>) {
        *self.inner.native.borrow_mut() = Some(src);
    }

    /// The platform's native audio source, if the producer set one.
    pub fn native_source(&self) -> Option<Rc<dyn Any>> {
        self.inner.native.borrow().clone()
    }

    /// Attach the capture teardown closure, run when the last clone drops.
    /// Replaces any previously attached stopper.
    pub fn attach_stopper(&self, stop: impl FnOnce() + 'static) {
        *self.inner.stopper.borrow_mut() = Some(Box::new(stop));
    }

    /// Subscribe to PCM chunks (push). `callback` fires with each new
    /// [`AudioFrame`] until the returned [`AudioSubscription`] is dropped.
    /// Runs on the producer's audio thread on native/Android, the main thread
    /// on web — keep it fast; copy samples out rather than processing in place.
    pub fn subscribe<C: AudioFrameCallback>(&self, callback: C) -> AudioSubscription {
        let id = self.inner.channel.add_subscriber(Box::new(callback));
        AudioSubscription {
            channel: self.inner.channel.clone(),
            id,
        }
    }

    /// Copy the most recent chunk's samples into `buf` (pull). Returns its
    /// [`AudioFormat`], or `None` if no chunk has arrived. Pair with
    /// [`generation`](Self::generation) to skip re-reading an unchanged chunk.
    pub fn latest(&self, buf: &mut Vec<f32>) -> Option<AudioFormat> {
        let slot = self.inner.channel.latest.lock().unwrap();
        let chunk = slot.as_ref()?;
        buf.clear();
        buf.extend_from_slice(&chunk.samples);
        Some(chunk.format)
    }

    /// The most recent chunk's [`AudioFormat`], or `None` if none has arrived.
    pub fn format(&self) -> Option<AudioFormat> {
        self.inner
            .channel
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| a.format)
    }

    /// The capture timestamp of the most recent chunk (microseconds on the
    /// shared [`clock`] timeline) — the start of the chunk — or `None` if none
    /// has arrived.
    pub fn latest_pts(&self) -> Option<u64> {
        self.inner
            .channel
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| a.pts_micros)
    }

    /// A counter bumped on every chunk. Compare across calls to detect a new
    /// chunk without copying.
    pub fn generation(&self) -> u64 {
        self.inner.channel.generation.load(Ordering::Acquire)
    }
}

/// Cancels a [`subscribe`](AudioStream::subscribe) when dropped. Hold it for
/// as long as you want the callback to fire.
///
/// Do not drop an `AudioSubscription` from inside its own callback (the
/// channel's subscriber list is locked during fan-out).
pub struct AudioSubscription {
    channel: Arc<AudioChannel>,
    id: u64,
}

impl Drop for AudioSubscription {
    fn drop(&mut self) {
        self.channel.remove_subscriber(self.id);
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const FMT: AudioFormat = AudioFormat {
        sample_rate: 48_000,
        channels: 2,
    };

    #[test]
    fn latest_returns_none_before_any_write() {
        let (stream, _writer) = AudioStream::new();
        let mut buf = Vec::new();
        assert!(stream.latest(&mut buf).is_none());
        assert!(stream.format().is_none());
        assert_eq!(stream.generation(), 0);
    }

    #[test]
    fn latest_copies_most_recent_chunk_and_format() {
        let (stream, writer) = AudioStream::new();
        writer.write_pcm_f32(FMT.sample_rate, FMT.channels, &[0.1, 0.2, 0.3, 0.4]);
        writer.write_pcm_f32(FMT.sample_rate, FMT.channels, &[0.5, 0.6]);

        let mut buf = Vec::new();
        let fmt = stream.latest(&mut buf).expect("a chunk");
        assert_eq!(fmt, FMT);
        assert_eq!(buf, vec![0.5, 0.6]);
        assert_eq!(stream.generation(), 2);
    }

    #[test]
    fn subscribe_receives_pushed_chunks_with_pts() {
        let (stream, writer) = AudioStream::new();
        let count = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(Mutex::new(Vec::<(usize, u64)>::new()));

        let sub = {
            let count = count.clone();
            let frames = frames.clone();
            stream.subscribe(move |f: &AudioFrame| {
                count.fetch_add(1, Ordering::Relaxed);
                frames
                    .lock()
                    .unwrap()
                    .push((f.samples.len(), f.pts_micros));
            })
        };

        writer.write_pcm_f32_at(FMT.sample_rate, FMT.channels, &[0.0, 0.0, 0.0, 0.0], 1_000);
        writer.write_pcm_f32_at(FMT.sample_rate, FMT.channels, &[0.0, 0.0], 2_000);
        assert_eq!(count.load(Ordering::Relaxed), 2);
        assert_eq!(*frames.lock().unwrap(), vec![(4, 1_000), (2, 2_000)]);

        drop(sub);
        writer.write_pcm_f32(FMT.sample_rate, FMT.channels, &[0.0, 0.0]);
        assert_eq!(count.load(Ordering::Relaxed), 2, "dropped sub stops firing");
    }

    #[test]
    fn empty_or_zero_format_chunks_are_ignored() {
        let (stream, writer) = AudioStream::new();
        writer.write_pcm_f32(FMT.sample_rate, FMT.channels, &[]); // empty
        writer.write_pcm_f32(0, FMT.channels, &[0.1, 0.2]); // zero rate
        writer.write_pcm_f32(FMT.sample_rate, 0, &[0.1, 0.2]); // zero channels
        assert_eq!(stream.generation(), 0);
        let mut buf = Vec::new();
        assert!(stream.latest(&mut buf).is_none());
    }

    #[test]
    fn frame_count_and_duration() {
        let f = AudioFrame {
            samples: &[0.0; 480 * 2],
            sample_rate: 48_000,
            channels: 2,
            pts_micros: 0,
        };
        assert_eq!(f.frame_count(), 480);
        assert!((f.duration_secs() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn dropping_last_stream_runs_stopper() {
        let (stream, _writer) = AudioStream::new();
        let stopped = Arc::new(AtomicUsize::new(0));
        {
            let stopped = stopped.clone();
            stream.attach_stopper(move || {
                stopped.fetch_add(1, Ordering::Relaxed);
            });
        }
        let clone = stream.clone();
        drop(stream);
        assert_eq!(stopped.load(Ordering::Relaxed), 0, "clone keeps it alive");
        drop(clone);
        assert_eq!(stopped.load(Ordering::Relaxed), 1, "last drop stops");
    }

    #[test]
    fn native_source_roundtrips() {
        let (stream, _writer) = AudioStream::new();
        assert!(stream.native_source().is_none());
        let src: Rc<dyn Any> = Rc::new(String::from("native-handle"));
        stream.set_native_source(src);
        let got = stream.native_source().expect("source");
        assert_eq!(got.downcast_ref::<String>().map(String::as_str), Some("native-handle"));
    }
}
