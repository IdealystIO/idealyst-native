//! [`Mixer`] — a live, real-time-paced [`AudioStream`] producer that mixes
//! [`Pcm`] buffers in on demand.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use media_stream::{AudioFormat, AudioStream, AudioWriter};
use runtime_core::scheduling;

use crate::pcm::{convert_samples, Pcm};

/// How often the real-time driver pumps, in milliseconds.
///
/// 20 ms is a timer the host scheduler can honour on every platform
/// (browsers clamp `setTimeout` to ≥4 ms; 20 ms stays well clear) while
/// keeping each chunk small (~880–960 frames). A `setTimeout` chain is used
/// rather than `requestAnimationFrame` because rAF stops entirely in a hidden
/// tab / backgrounded view, and a recording must keep its audio timeline
/// running when nothing is animating.
pub const PUMP_TICK_MS: i32 = 20;

/// How far ahead of "now" each pump renders, in microseconds (60 ms).
///
/// Consumers (the web `MediaStreamTrack` bridge, an encoder) drain audio at
/// the hardware rate; the producer must stay ahead of them or they starve and
/// click. Timer callbacks arrive late by up to a frame or two when the main
/// thread is busy (16–33 ms on a 60 Hz display), so the lead must cover one
/// [`PUMP_TICK_MS`] plus that jitter: 20 + ~33 ≈ 55 ms, rounded to 60. The
/// cost is latency: a voice started with [`Mixer::play`] begins at the
/// write head, which is at most this far ahead of real time.
pub const PUMP_LEAD_MICROS: u64 = 60_000;

/// The most audio a single pump will render, in microseconds (250 ms).
///
/// After a stall — a tab hidden for 10 s throttles timers to ≤1 Hz, a
/// debugger pause, a long synchronous task — the frames "owed" can be
/// seconds' worth. Rendering them all at once would dump seconds of audio
/// into consumers that play it back in real time: the web bridge caps its
/// backlog at ~1 s and DROPS the oldest audio beyond that, and a recorder
/// would stamp a burst of stale audio. Instead the pump renders at most this
/// much (ending at `now + lead`) and skips the rest: the stream's timestamps
/// jump forward over the gap (they stay monotonic and on the shared clock, so
/// a muxer keeps lip-sync), and voices resume where they left off rather than
/// having the stall's worth of sound silently consumed.
pub const MAX_PUMP_MICROS: u64 = 250_000;

struct Slot {
    id: u64,
    /// Samples already converted to the mixer's format.
    samples: Rc<Vec<f32>>,
    /// Next sample index (not frame) to mix.
    pos: usize,
    gain: f32,
}

struct State {
    /// Shared-clock time of frame 0 of the timeline; set by the first pump.
    anchor_micros: Option<u64>,
    /// Frames accounted for since the anchor (rendered, or skipped by the
    /// stall cap). The next chunk starts at this frame.
    frames: u64,
    voices: Vec<Slot>,
    next_id: u64,
    gain: f32,
}

struct Inner {
    format: AudioFormat,
    writer: AudioWriter,
    state: RefCell<State>,
}

impl Inner {
    fn pump_to(&self, now_micros: u64) {
        let rate = self.format.sample_rate as u128;
        let ch = self.format.channels as usize;
        // Render under the borrow, write after releasing it: the write fans
        // out to subscribers synchronously, and a (web, main-thread)
        // subscriber is free to call back into the mixer.
        let (chunk, pts) = {
            let mut st = self.state.borrow_mut();
            let anchor = *st.anchor_micros.get_or_insert(now_micros);
            let ahead = now_micros.saturating_sub(anchor) as u128 + PUMP_LEAD_MICROS as u128;
            let target = (ahead * rate / 1_000_000) as u64;
            let mut owed = target.saturating_sub(st.frames);
            let max = (MAX_PUMP_MICROS as u128 * rate / 1_000_000) as u64;
            if owed > max {
                // Stall: drop the oldest owed audio (see MAX_PUMP_MICROS).
                st.frames += owed - max;
                owed = max;
            }
            if owed == 0 {
                return;
            }
            let pts = anchor + (st.frames as u128 * 1_000_000 / rate) as u64;
            let n = owed as usize * ch;
            let mut chunk = vec![0.0f32; n];
            st.voices.retain_mut(|v| {
                let take = (v.samples.len() - v.pos).min(n);
                for (dst, src) in chunk[..take].iter_mut().zip(&v.samples[v.pos..v.pos + take]) {
                    *dst += src * v.gain;
                }
                v.pos += take;
                v.pos < v.samples.len()
            });
            let g = st.gain;
            for s in &mut chunk {
                *s = (*s * g).clamp(-1.0, 1.0);
            }
            st.frames += owed;
            (chunk, pts)
        };
        self.writer
            .write_pcm_f32_at(self.format.sample_rate, self.format.channels, &chunk, pts);
    }
}

/// A live audio bus: a continuous, real-time-paced [`AudioStream`] that mixes
/// [`Pcm`] buffers in as you [`play`](Self::play) them — the bridge from
/// synthesized sound into the media pipeline (record a game's audio with
/// `media-writer`, route it into a web `MediaStreamTrack`, feed any
/// [`AudioStream`] consumer).
///
/// ```ignore
/// let bus = synth::Mixer::new(synth::AudioFormat { sample_rate: 48_000, channels: 2 });
/// let stream = bus.stream();          // hand to media-writer, etc.
/// bus.play(&synth::presets::coin());  // fire-and-forget
/// ```
///
/// # Pacing
///
/// The mixer emits audio continuously — **silence when no voice is playing**
/// — so the stream's timeline never has holes a recorder would have to
/// paper over. Each pump renders exactly the frames owed up to
/// `now + `[`PUMP_LEAD_MICROS`], stamped with sample-accurate timestamps on
/// the shared [`media_stream::clock`], so a muxer lines it up against video
/// from `camera` / `screen-recorder`.
///
/// The real-time driver is a [`PUMP_TICK_MS`] timer chain on the host
/// scheduler, started by [`new`](Self::new). It needs an installed scheduler
/// (every backend installs one at boot); with none — a plain native unit
/// test — nothing pumps automatically and you drive it with
/// [`pump_to`](Self::pump_to) / [`pump`](Self::pump). A stall is capped at
/// [`MAX_PUMP_MICROS`] per pump.
///
/// # Lifetime
///
/// `Mixer` is a cheap `Clone` handle. The mixer — and its timer — stays alive
/// while **any** `Mixer` clone **or any clone of its [`stream`](Self::stream)**
/// is alive (a recorder holding just the stream keeps it running). When the
/// last one drops, the timer chain ends on its next tick.
#[derive(Clone)]
pub struct Mixer {
    inner: Rc<Inner>,
    stream: AudioStream,
}

impl Mixer {
    /// A mixer producing `format` audio (a zero rate or channel count is
    /// raised to 1). Starts the real-time driver if a scheduler is installed.
    pub fn new(format: AudioFormat) -> Mixer {
        let format = AudioFormat {
            sample_rate: format.sample_rate.max(1),
            channels: format.channels.max(1),
        };
        let (stream, writer) = AudioStream::new();
        let inner = Rc::new(Inner {
            format,
            writer,
            state: RefCell::new(State {
                anchor_micros: None,
                frames: 0,
                voices: Vec::new(),
                next_id: 0,
                gain: 1.0,
            }),
        });
        // The stream's stopper owns a strong ref, so the mixer lives as long
        // as any stream clone does. No cycle: `Inner` never holds the stream.
        let keep = inner.clone();
        stream.attach_stopper(move || drop(keep));
        schedule_tick(Rc::downgrade(&inner));
        Mixer { inner, stream }
    }

    /// The live stream this mixer produces (a clone of the handle).
    pub fn stream(&self) -> AudioStream {
        self.stream.clone()
    }

    /// The format the stream carries.
    pub fn format(&self) -> AudioFormat {
        self.inner.format
    }

    /// Start mixing `pcm` in at the write head (the next frame the mixer
    /// renders — at most [`PUMP_LEAD_MICROS`] ahead of real time). The buffer
    /// is converted to the mixer's format once, up front.
    ///
    /// **Fire-and-forget**: the returned [`Voice`] is only a remote control.
    /// Dropping it does *not* stop the sound — a sound effect plays to its
    /// end without the caller keeping anything alive. Call
    /// [`Voice::stop`] to cut it short. (This is deliberately the opposite of
    /// `audio::Playback`, whose drop stops the sound: a mixer voice is a few
    /// hundred ms of samples, finite by construction, and owning a handle per
    /// gunshot would just be noise in game code.)
    pub fn play(&self, pcm: &Pcm) -> Voice {
        let samples = convert_samples(&pcm.samples, pcm.format, self.inner.format);
        let mut st = self.inner.state.borrow_mut();
        let id = st.next_id;
        st.next_id += 1;
        if !samples.is_empty() {
            st.voices.push(Slot {
                id,
                samples: Rc::new(samples),
                pos: 0,
                gain: 1.0,
            });
        }
        Voice {
            id,
            inner: Rc::downgrade(&self.inner),
        }
    }

    /// Set the master gain applied to the mix (before clamping to `[-1, 1]`).
    /// Default `1.0`. Takes effect from the next rendered chunk.
    pub fn set_gain(&self, gain: f32) {
        self.inner.state.borrow_mut().gain = gain.max(0.0);
    }

    /// The master gain.
    pub fn gain(&self) -> f32 {
        self.inner.state.borrow().gain
    }

    /// Voices still playing.
    pub fn active_voices(&self) -> usize {
        self.inner.state.borrow().voices.len()
    }

    /// Stop every playing voice.
    pub fn stop_all(&self) {
        self.inner.state.borrow_mut().voices.clear();
    }

    /// Pump against the shared [`media_stream::clock`]. The real-time driver
    /// calls this every [`PUMP_TICK_MS`]; call it yourself only when no
    /// scheduler is installed.
    pub fn pump(&self) {
        self.inner.pump_to(media_stream::clock::now_micros());
    }

    /// Render and write exactly the frames owed up to
    /// `now_micros + `[`PUMP_LEAD_MICROS`] (at most [`MAX_PUMP_MICROS`]' worth),
    /// as one chunk. The first call anchors the timeline at `now_micros`.
    /// Deterministic — the testing seam for pacing; `now_micros` should be on
    /// the [`media_stream::clock`] timeline.
    pub fn pump_to(&self, now_micros: u64) {
        self.inner.pump_to(now_micros);
    }
}

/// Arm the next driver tick. Uses `after_ms_detached` (the runtime owns and
/// sweeps the one-shot task) holding only a `Weak` to the mixer, so the chain
/// ends by itself on the first tick after the mixer is gone — no handle to
/// cancel, no `mem::forget`, and no cancelling a task from inside its own
/// callback.
fn schedule_tick(weak: Weak<Inner>) {
    // Without a scheduler, `after_ms*` runs its body synchronously off web —
    // a self-rearming chain would recurse forever. Stay manual instead.
    if !scheduling::is_scheduler_installed() {
        return;
    }
    scheduling::after_ms_detached(PUMP_TICK_MS, move || {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        inner.pump_to(media_stream::clock::now_micros());
        drop(inner);
        if weak.strong_count() > 0 {
            schedule_tick(weak);
        }
    });
}

/// A remote control for one sound started with [`Mixer::play`]. Dropping it
/// does **not** stop the sound (see [`Mixer::play`]).
pub struct Voice {
    id: u64,
    inner: Weak<Inner>,
}

impl Voice {
    /// Stop the sound; it contributes nothing from the next rendered chunk.
    /// No-op if it already finished.
    pub fn stop(&self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.state.borrow_mut().voices.retain(|v| v.id != self.id);
        }
    }

    /// Whether the sound is still playing (not finished, stopped, or its
    /// mixer gone).
    pub fn is_playing(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.state.borrow().voices.iter().any(|v| v.id == self.id))
    }

    /// Set this voice's gain (default `1.0`), from the next rendered chunk.
    pub fn set_gain(&self, gain: f32) {
        if let Some(inner) = self.inner.upgrade() {
            if let Some(v) = inner.state.borrow_mut().voices.iter_mut().find(|v| v.id == self.id) {
                v.gain = gain.max(0.0);
            }
        }
    }
}
