//! The deterministic Rust mixer every non-web backend renders with, and its
//! handle side ([`MixerEngine`]).
//!
//! # Threads and the render path
//!
//! The **handle side** (`AudioEngine` / `Voice`, main thread) never touches
//! mixer state. It sends [`Command`]s through a bounded lock-free queue
//! (`crossbeam_queue::ArrayQueue`, allocated once) that the **render side**
//! ([`Mixer::render`], the device's audio callback) drains at the start of
//! every render quantum. The render path takes no locks and, in steady state,
//! allocates nothing: the voice table is a fixed `MAX_VOICES` array, the
//! quantum scratch is inline, and commands move by value.
//!
//! # Who frees what (no deallocation on the audio thread)
//!
//! Commands carry `Arc`s into the render thread (a voice's PCM, a stream's
//! frame ring, its status flag). If the render thread dropped the LAST
//! reference to one of those it would free a possibly multi-megabyte buffer
//! inside the audio callback. It never does: for every voice the handle side
//! keeps its own clones (a `LiveVoice`) plus a per-voice `claim` token that
//! only the render side also holds. The render side drops a voice's source
//! and flag BEFORE its claim (field order in `VoiceState`); the handle side
//! drops its `LiveVoice` only once it observes the claim's count at 1 — i.e.
//! after the render side let go of everything — so the handle side's drop is
//! always the last one, on the main thread. The render thread only ever
//! performs reference-count decrements. Pruning runs on every handle call.
//!
//! # Signal chain and timing (the convergence contract with the web graph)
//!
//! Per voice: source (linear-interpolated read at `rate`, k-rate) →
//! gain (a-rate) → biquad low-pass (a-rate cutoff) → stereo panner (a-rate)
//! → sum → master gain (a-rate) → output channel mapping → clamp to
//! `[-1, 1]`. The web backend builds the same chain from
//! `AudioBufferSourceNode → GainNode → BiquadFilterNode → StereoPannerNode
//! → GainNode(master) → destination`. Work happens in 128-frame render
//! quanta like Web Audio's; commands take effect at the next quantum boundary.

use std::collections::VecDeque;
use std::sync::atomic::{fence, AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use media_stream::{AudioFrame, AudioStream, AudioSubscription};

use crate::buffer::BufferData;
use crate::dsp::{self, BiquadState, Param, LOWPASS_Q_DB};
use crate::params::VoiceParams;
use crate::SoundBuffer;

/// The most voices that sound at once. Past it, [`play`](crate::AudioEngine::play)
/// **steals the oldest non-looping buffer voice** (cut immediately); looping
/// and stream voices are never stolen. If every voice is looping or a
/// stream, the new voice is dropped (its [`Voice::is_playing`](crate::Voice::is_playing)
/// turns `false`).
pub const MAX_VOICES: usize = 128;

/// Frames per render quantum — Web Audio's fixed 128.
pub const QUANTUM: usize = 128;

/// Commands the queue holds before the handle side buffers the overflow
/// locally (only reachable while the output is suspended or stalled).
pub(crate) const COMMAND_CAPACITY: usize = 4096;

/// The most audio a stream voice buffers ahead of the playhead: past this
/// the oldest frames are dropped, so a producer running fast (or a stalled
/// output) never builds latency — the same policy as the `media-stream` web
/// bridge.
pub const STREAM_BACKLOG_SECS: f32 = 0.1;

/// A voice's play-state flag, shared by its `Voice` handles, the handle
/// side's bookkeeping and the render side.
#[derive(Debug)]
pub(crate) struct VoiceShared {
    pub(crate) playing: AtomicBool,
}

impl VoiceShared {
    pub(crate) fn new() -> Arc<VoiceShared> {
        Arc::new(VoiceShared {
            playing: AtomicBool::new(true),
        })
    }
}

/// A live stream voice's input: output-rate frames (`[l, r]`; `r` unused
/// when `channels == 1`), filled by the stream's producer thread.
pub(crate) struct StreamInput {
    ring: ArrayQueue<[f32; 2]>,
    /// 1 (mono) or 2 (stereo) — how the panner treats the frames.
    channels: AtomicU8,
}

pub(crate) enum Source {
    Buffer(Arc<BufferData>),
    Stream(Arc<StreamInput>),
}

/// A change to one voice. Values are already sanitized by the public layer.
#[derive(Clone, Copy, Debug)]
pub(crate) enum VoiceOp {
    Stop { fade_secs: f32 },
    Gain { gain: f32, ramp_secs: f32 },
    Pan { pan: f32, ramp_secs: f32 },
    Rate { rate: f32, ramp_secs: f32 },
    Lowpass { hz: Option<f32>, ramp_secs: f32 },
    Looping(bool),
}

pub(crate) enum Command {
    Play {
        id: u64,
        source: Source,
        params: VoiceParams,
        shared: Arc<VoiceShared>,
        claim: Arc<()>,
    },
    Voice {
        id: u64,
        op: VoiceOp,
    },
    MasterGain {
        gain: f32,
        ramp_secs: f32,
    },
}

pub(crate) type CommandQueue = ArrayQueue<Command>;

pub(crate) fn new_queue() -> Arc<CommandQueue> {
    Arc::new(ArrayQueue::new(COMMAND_CAPACITY))
}

// ---------------------------------------------------------------------------
// Render side.
// ---------------------------------------------------------------------------

struct VoiceState {
    id: u64,
    /// Start order, for "steal the oldest".
    seq: u64,
    // Drop order matters: `source` and `shared` drop BEFORE `claim` (see the
    // module docs).
    source: Source,
    shared: Arc<VoiceShared>,
    /// Playhead in source frames.
    pos: f64,
    looping: bool,
    stopping: bool,
    gain: Param,
    pan: Param,
    rate: Param,
    /// Cutoff in Hz; at Nyquist the filter is the identity.
    cutoff: Param,
    filter_on: bool,
    /// Turning the filter off: ramping the cutoff to Nyquist, then bypass.
    filter_off_pending: bool,
    coeffs: [f64; 5],
    coeff_hz: f32,
    biquad: [BiquadState; 2],
    /// Never read: its reference count IS the "render side is done" signal.
    _claim: Arc<()>,
}

/// The deterministic mixer. Owned by the render thread (the device callback,
/// or an [`OfflineRenderer`](crate::OfflineRenderer)).
pub(crate) struct Mixer {
    queue: Arc<CommandQueue>,
    sample_rate: u32,
    voices: Vec<Option<VoiceState>>,
    master: Param,
    /// The current quantum, post-master, pre-clamp, stereo.
    quantum: [[f32; 2]; QUANTUM],
    /// Next frame of `quantum` to hand out; `QUANTUM` = render a new one.
    q_pos: usize,
    next_seq: u64,
}

impl Mixer {
    pub(crate) fn new(queue: Arc<CommandQueue>, sample_rate: u32) -> Mixer {
        let mut voices = Vec::with_capacity(MAX_VOICES);
        voices.resize_with(MAX_VOICES, || None);
        Mixer {
            queue,
            sample_rate: sample_rate.max(1),
            voices,
            master: Param::new(1.0),
            quantum: [[0.0; 2]; QUANTUM],
            q_pos: QUANTUM,
            next_seq: 0,
        }
    }

    fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    /// Fill `out` (interleaved, `channels` wide) with the next frames.
    ///
    /// The mixer is stereo inside; the output mapping is Web Audio's
    /// "speakers" rule: mono = `(L + R)/2`, stereo as is, wider layouts get
    /// L/R in the first two channels and silence elsewhere. Every sample is
    /// clamped to `[-1, 1]` last, as a browser destination does. A changed
    /// `sample_rate` (a device reconfiguration) applies from now on.
    pub(crate) fn render(&mut self, out: &mut [f32], channels: u16, sample_rate: u32) {
        if sample_rate.max(1) != self.sample_rate {
            self.sample_rate = sample_rate.max(1);
            for v in self.voices.iter_mut().flatten() {
                v.coeff_hz = f32::NAN; // force a coefficient recompute
            }
        }
        let ch = channels.max(1) as usize;
        let frames = out.len() / ch;
        for f in 0..frames {
            if self.q_pos == QUANTUM {
                self.render_quantum();
                self.q_pos = 0;
            }
            let [l, r] = self.quantum[self.q_pos];
            self.q_pos += 1;
            let dst = &mut out[f * ch..(f + 1) * ch];
            match ch {
                1 => dst[0] = clamp_out(0.5 * (l + r)),
                _ => {
                    dst[0] = clamp_out(l);
                    dst[1] = clamp_out(r);
                    for s in &mut dst[2..] {
                        *s = 0.0;
                    }
                }
            }
        }
        for s in &mut out[frames * ch..] {
            *s = 0.0;
        }
    }

    fn render_quantum(&mut self) {
        while let Some(cmd) = self.queue.pop() {
            self.apply(cmd);
        }
        self.quantum = [[0.0; 2]; QUANTUM];
        let out_rate = self.sample_rate;
        let nyquist = self.nyquist();
        for slot in self.voices.iter_mut() {
            let Some(v) = slot else { continue };
            if !render_voice(v, &mut self.quantum, out_rate, nyquist) {
                v.shared.playing.store(false, Ordering::Release);
                *slot = None;
            }
        }
        for frame in self.quantum.iter_mut() {
            let g = self.master.tick();
            frame[0] *= g;
            frame[1] *= g;
        }
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play {
                id,
                source,
                params,
                shared,
                claim,
            } => self.start_voice(id, source, params, shared, claim),
            Command::Voice { id, op } => self.voice_op(id, op),
            Command::MasterGain { gain, ramp_secs } => {
                let frames = dsp::secs_to_frames(ramp_secs, self.sample_rate);
                self.master.ramp_to(gain, frames);
            }
        }
    }

    fn start_voice(
        &mut self,
        id: u64,
        source: Source,
        params: VoiceParams,
        shared: Arc<VoiceShared>,
        claim: Arc<()>,
    ) {
        let nyquist = self.nyquist();
        let mut pos = 0.0;
        if let Source::Buffer(data) = &source {
            let frames = data.frames as f64;
            pos = params.start_offset_secs as f64 * data.sample_rate as f64;
            if params.looping && frames > 0.0 {
                pos %= frames;
            } else if pos >= frames {
                // Nothing left to play.
                shared.playing.store(false, Ordering::Release);
                return;
            }
        }
        let Some(slot) = self.free_slot() else {
            shared.playing.store(false, Ordering::Release);
            return;
        };
        let (filter_on, cutoff) = match params.lowpass_hz {
            Some(hz) => (true, hz.min(nyquist)),
            None => (false, nyquist),
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        self.voices[slot] = Some(VoiceState {
            id,
            seq,
            source,
            shared,
            pos,
            looping: params.looping,
            stopping: false,
            gain: Param::new(params.gain),
            pan: Param::new(params.pan),
            rate: Param::new(params.rate),
            cutoff: Param::new(cutoff),
            filter_on,
            filter_off_pending: false,
            coeffs: [1.0, 0.0, 0.0, 0.0, 0.0],
            coeff_hz: f32::NAN,
            biquad: [BiquadState::default(); 2],
            _claim: claim,
        });
    }

    /// A free slot, stealing the oldest non-looping buffer voice when full.
    fn free_slot(&mut self) -> Option<usize> {
        if let Some(i) = self.voices.iter().position(Option::is_none) {
            return Some(i);
        }
        let victim = self
            .voices
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i, v)))
            .filter(|(_, v)| !v.looping && matches!(v.source, Source::Buffer(_)))
            .min_by_key(|(_, v)| v.seq)
            .map(|(i, _)| i)?;
        if let Some(v) = self.voices[victim].take() {
            v.shared.playing.store(false, Ordering::Release);
        }
        Some(victim)
    }

    fn voice_op(&mut self, id: u64, op: VoiceOp) {
        let rate = self.sample_rate;
        let nyquist = self.nyquist();
        let Some(idx) = self
            .voices
            .iter()
            .position(|v| v.as_ref().is_some_and(|v| v.id == id))
        else {
            return;
        };
        let v = self.voices[idx].as_mut().expect("position found it");
        match op {
            VoiceOp::Stop { fade_secs } => {
                let frames = dsp::secs_to_frames(fade_secs, rate);
                if frames == 0 {
                    v.shared.playing.store(false, Ordering::Release);
                    self.voices[idx] = None;
                } else {
                    v.stopping = true;
                    v.gain.ramp_to(0.0, frames);
                }
            }
            // A stopping voice's gain belongs to its fade.
            VoiceOp::Gain { gain, ramp_secs } => {
                if !v.stopping {
                    v.gain.ramp_to(gain, dsp::secs_to_frames(ramp_secs, rate));
                }
            }
            VoiceOp::Pan { pan, ramp_secs } => {
                v.pan.ramp_to(pan, dsp::secs_to_frames(ramp_secs, rate))
            }
            VoiceOp::Rate { rate: r, ramp_secs } => {
                v.rate.ramp_to(r, dsp::secs_to_frames(ramp_secs, rate))
            }
            VoiceOp::Lowpass { hz, ramp_secs } => {
                let frames = dsp::secs_to_frames(ramp_secs, rate);
                match hz {
                    Some(hz) => {
                        if !v.filter_on {
                            // Enter at Nyquist — the identity, continuous
                            // with the bypass history — then ramp down.
                            v.filter_on = true;
                            v.cutoff = Param::new(nyquist);
                        }
                        v.filter_off_pending = false;
                        v.cutoff.ramp_to(hz.min(nyquist), frames);
                    }
                    None => {
                        if v.filter_on {
                            v.cutoff.ramp_to(nyquist, frames);
                            v.filter_off_pending = true;
                            if frames == 0 {
                                v.filter_on = false;
                                v.filter_off_pending = false;
                            }
                        }
                    }
                }
            }
            VoiceOp::Looping(looping) => v.looping = looping,
        }
    }
}

impl Drop for Mixer {
    /// The output is gone: nothing will play again, so every voice — live or
    /// still queued — reports not playing.
    fn drop(&mut self) {
        for v in self.voices.iter().flatten() {
            v.shared.playing.store(false, Ordering::Release);
        }
        while let Some(cmd) = self.queue.pop() {
            if let Command::Play { shared, .. } = cmd {
                shared.playing.store(false, Ordering::Release);
            }
        }
    }
}

#[inline]
fn clamp_out(s: f32) -> f32 {
    if s.is_nan() {
        0.0
    } else {
        s.clamp(-1.0, 1.0)
    }
}

/// Render one quantum of `v` into `acc`. Returns `false` when the voice is
/// finished (played out, or its stop fade completed).
fn render_voice(
    v: &mut VoiceState,
    acc: &mut [[f32; 2]; QUANTUM],
    out_rate: u32,
    nyquist: f32,
) -> bool {
    // `playbackRate` is k-rate in Web Audio: one value per quantum.
    let rate = v.rate.current() as f64;
    v.rate.advance(QUANTUM as u32);

    let (buffer, stream) = match &v.source {
        Source::Buffer(b) => (Some(&**b), None),
        Source::Stream(s) => (None, Some(&**s)),
    };
    let channels = match (&buffer, stream) {
        (Some(b), _) => b.channels,
        (None, Some(s)) => s.channels.load(Ordering::Relaxed).clamp(1, 2) as usize,
        _ => 1,
    };
    let step = buffer
        .map(|b| b.sample_rate as f64 / out_rate as f64 * rate)
        .unwrap_or(0.0);

    for frame in acc.iter_mut() {
        // --- source ---
        let (mut l, mut r) = if let Some(b) = buffer {
            if !v.looping && v.pos >= b.frames as f64 {
                return false;
            }
            let (l, r) = read_interp(b, v.pos, v.looping);
            v.pos += step;
            if v.looping && b.frames > 0 && v.pos >= b.frames as f64 {
                v.pos %= b.frames as f64;
            }
            (l, r)
        } else if let Some(s) = stream {
            let [l, r] = s.ring.pop().unwrap_or([0.0, 0.0]);
            (l, r)
        } else {
            (0.0, 0.0)
        };

        // --- gain ---
        let g = v.gain.tick();
        l *= g;
        r *= g;

        // --- low-pass ---
        if v.filter_on {
            let hz = v.cutoff.tick();
            if hz != v.coeff_hz {
                v.coeffs = dsp::lowpass_coefficients(
                    out_rate as f64,
                    hz.min(nyquist) as f64,
                    LOWPASS_Q_DB,
                );
                v.coeff_hz = hz;
            }
            l = v.biquad[0].process(&v.coeffs, l);
            if channels == 2 {
                r = v.biquad[1].process(&v.coeffs, r);
            }
            if v.filter_off_pending && !v.cutoff.is_ramping() {
                v.filter_on = false;
                v.filter_off_pending = false;
            }
        } else {
            v.cutoff.tick();
            l = v.biquad[0].bypass(l);
            if channels == 2 {
                r = v.biquad[1].bypass(r);
            }
        }

        // --- pan ---
        let pan = v.pan.tick();
        let (ol, or) = if channels == 2 {
            dsp::pan_stereo(pan, l, r)
        } else {
            dsp::pan_mono(pan, l)
        };
        frame[0] += ol;
        frame[1] += or;

        if v.stopping && !v.gain.is_ramping() {
            return false;
        }
    }
    true
}

/// Linear interpolation between frame `⌊pos⌋` and the next one (the first
/// frame when looping across the end, silence past the end otherwise) — the
/// `AudioBufferSourceNode` resampling the spec describes.
#[inline]
fn read_interp(b: &BufferData, pos: f64, looping: bool) -> (f32, f32) {
    let i = pos as usize;
    let frac = (pos - i as f64) as f32;
    let at = |k: usize| -> (f32, f32) {
        let k = if k >= b.frames {
            if looping && b.frames > 0 {
                k % b.frames
            } else {
                return (0.0, 0.0);
            }
        } else {
            k
        };
        if b.channels == 2 {
            (b.samples[2 * k], b.samples[2 * k + 1])
        } else {
            (b.samples[k], 0.0)
        }
    };
    let (l0, r0) = at(i);
    if frac == 0.0 {
        return (l0, r0);
    }
    let (l1, r1) = at(i + 1);
    (l0 + (l1 - l0) * frac, r0 + (r1 - r0) * frac)
}

// ---------------------------------------------------------------------------
// Stream input conversion (producer thread).
// ---------------------------------------------------------------------------

/// Converts a stream's chunks to the output rate and to mono/stereo frames,
/// on the stream PRODUCER's thread (allocation is fine there; the scratch is
/// reused). A continuous-phase linear resampler, gapless across chunks; a
/// change of source rate or layout restarts it.
struct StreamConverter {
    input: Arc<StreamInput>,
    out_rate: u32,
    src_rate: u32,
    src_channels: u16,
    /// Read position (source frames) relative to the current chunk's first
    /// frame; in `[-1, 0)` it interpolates from `prev`.
    t: f64,
    prev: [f32; 2],
    scratch: Vec<[f32; 2]>,
}

impl StreamConverter {
    fn push(&mut self, frame: &AudioFrame) {
        let in_ch = frame.channels.max(1) as usize;
        if frame.sample_rate == 0 {
            return;
        }
        if frame.sample_rate != self.src_rate || frame.channels != self.src_channels {
            self.src_rate = frame.sample_rate;
            self.src_channels = frame.channels;
            self.t = 0.0;
            self.prev = [0.0; 2];
            self.input
                .channels
                .store(if in_ch == 1 { 1 } else { 2 }, Ordering::Relaxed);
        }
        self.scratch.clear();
        self.scratch
            .extend(frame.samples.chunks_exact(in_ch).map(|f| match in_ch {
                1 => [f[0], 0.0],
                2 => [f[0], f[1]],
                _ => {
                    let s = downmix_frame(f);
                    [s.0, s.1]
                }
            }));
        let n = self.scratch.len();
        if n == 0 {
            return;
        }
        if self.src_rate == self.out_rate {
            for f in &self.scratch {
                let _ = self.input.ring.force_push(*f);
            }
            return;
        }
        let step = self.src_rate as f64 / self.out_rate as f64;
        let mut t = self.t;
        // Strictly below the last frame: the interval [n-1, n) needs the
        // NEXT chunk's first frame, so it is produced on the next call (with
        // `t - n` in [-1, 0), interpolating from `prev`). `<=` here indexes
        // one past the chunk when `t` lands exactly on n-1.
        while t < (n - 1) as f64 {
            let i0 = t.floor() as isize;
            let frac = (t - i0 as f64) as f32;
            let a = if i0 < 0 {
                self.prev
            } else {
                self.scratch[i0 as usize]
            };
            let b = self.scratch[(i0 + 1) as usize];
            let _ = self
                .input
                .ring
                .force_push([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
            t += step;
        }
        self.t = t - n as f64;
        self.prev = self.scratch[n - 1];
    }
}

/// The Web Audio "speakers" stereo down-mix for one frame (quad, 5.1, else
/// discrete) — the same rule `SoundBuffer` applies.
fn downmix_frame(f: &[f32]) -> (f32, f32) {
    const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match f.len() {
        4 => (0.5 * (f[0] + f[2]), 0.5 * (f[1] + f[3])),
        6 => (
            f[0] + SQRT_HALF * (f[2] + f[4]),
            f[1] + SQRT_HALF * (f[2] + f[5]),
        ),
        _ => (f[0], f[1]),
    }
}

// ---------------------------------------------------------------------------
// Handle side.
// ---------------------------------------------------------------------------

/// The handle side's record of a voice it started: keeps every `Arc` the
/// render side holds alive until the render side is done (see the module
/// docs), plus a stream voice's subscription.
struct LiveVoice {
    shared: Arc<VoiceShared>,
    claim: Arc<()>,
    _source: Option<Arc<BufferData>>,
    _stream: Option<Arc<StreamInput>>,
    _sub: Option<AudioSubscription>,
}

#[derive(Default)]
pub(crate) struct HandleState {
    overflow: VecDeque<Command>,
    next_id: u64,
    live: Vec<LiveVoice>,
}

/// The [`EngineBackend`](crate::backend::EngineBackend) half that every
/// mixer-driven backend (cpal, Android, offline) shares: command
/// production, bookkeeping, deferred frees. The device-specific half is an
/// [`OutputDevice`](crate::backend::OutputDevice).
pub(crate) struct MixerEngine {
    // Field order: the device (which owns the render callback, and with it
    // the `Mixer`) closes before the bookkeeping that keeps its `Arc`s alive.
    device: Box<dyn crate::backend::OutputDevice>,
    queue: Arc<CommandQueue>,
    sample_rate: u32,
    state: std::cell::RefCell<HandleState>,
}

impl MixerEngine {
    pub(crate) fn new(
        queue: Arc<CommandQueue>,
        sample_rate: u32,
        device: Box<dyn crate::backend::OutputDevice>,
    ) -> MixerEngine {
        MixerEngine {
            device,
            queue,
            sample_rate: sample_rate.max(1),
            state: Default::default(),
        }
    }

    fn send(&self, st: &mut HandleState, cmd: Command) {
        Self::flush(&self.queue, st);
        if st.overflow.is_empty() {
            if let Err(cmd) = self.queue.push(cmd) {
                st.overflow.push_back(cmd);
            }
        } else {
            st.overflow.push_back(cmd);
        }
    }

    fn flush(queue: &CommandQueue, st: &mut HandleState) {
        while let Some(cmd) = st.overflow.pop_front() {
            if let Err(cmd) = queue.push(cmd) {
                st.overflow.push_front(cmd);
                break;
            }
        }
    }

    /// Flush overflow and free what the render side has released.
    fn maintain(&self, st: &mut HandleState) {
        Self::flush(&self.queue, st);
        st.live.retain(|v| {
            if Arc::strong_count(&v.claim) == 1 {
                // Pairs with the render side's Release decrement of the
                // claim: everything it dropped before that is visible, so
                // the drops below are the last references.
                fence(Ordering::Acquire);
                false
            } else {
                true
            }
        });
    }

    fn start(
        &self,
        source: Source,
        params: VoiceParams,
        sub: Option<AudioSubscription>,
    ) -> (u64, Arc<VoiceShared>) {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        let id = st.next_id;
        st.next_id += 1;
        let shared = VoiceShared::new();
        let claim = Arc::new(());
        let (src_buf, src_stream) = match &source {
            Source::Buffer(b) => (Some(b.clone()), None),
            Source::Stream(s) => (None, Some(s.clone())),
        };
        st.live.push(LiveVoice {
            shared: shared.clone(),
            claim: claim.clone(),
            _source: src_buf,
            _stream: src_stream,
            _sub: sub,
        });
        self.send(
            &mut st,
            Command::Play {
                id,
                source,
                params,
                shared: shared.clone(),
                claim,
            },
        );
        (id, shared)
    }
}

impl crate::backend::EngineBackend for MixerEngine {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn output_latency(&self) -> Option<std::time::Duration> {
        self.device.output_latency()
    }

    fn play(&self, buf: &SoundBuffer, params: VoiceParams) -> (u64, Arc<VoiceShared>) {
        self.start(Source::Buffer(buf.data.clone()), params, None)
    }

    fn play_stream(&self, stream: &AudioStream, params: VoiceParams) -> (u64, Arc<VoiceShared>) {
        let capacity = (STREAM_BACKLOG_SECS as f64 * self.sample_rate as f64)
            .round()
            .max(1.0) as usize;
        let input = Arc::new(StreamInput {
            ring: ArrayQueue::new(capacity),
            channels: AtomicU8::new(2),
        });
        let mut conv = StreamConverter {
            input: input.clone(),
            out_rate: self.sample_rate,
            src_rate: 0,
            src_channels: 0,
            t: 0.0,
            prev: [0.0; 2],
            scratch: Vec::new(),
        };
        let sub = stream.subscribe(move |frame: &AudioFrame| conv.push(frame));
        self.start(Source::Stream(input), params, Some(sub))
    }

    fn voice(&self, id: u64, op: VoiceOp) {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        self.send(&mut st, Command::Voice { id, op });
    }

    fn set_master_gain(&self, gain: f32, ramp_secs: f32) {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        self.send(&mut st, Command::MasterGain { gain, ramp_secs });
    }

    fn resume(&self) {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        drop(st);
        self.device.resume();
    }

    fn suspend(&self) {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        drop(st);
        self.device.suspend();
    }

    fn active_voices(&self) -> usize {
        let mut st = self.state.borrow_mut();
        self.maintain(&mut st);
        st.live
            .iter()
            .filter(|v| v.shared.playing.load(Ordering::Acquire))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioEngine, VoiceParams};
    use media_stream::AudioFormat;

    /// Regression guard for the "no deallocation on the audio thread" rule:
    /// after a voice plays out, the render side must have released its
    /// reference WITHOUT being the last holder — the handle side frees the
    /// PCM on its next call.
    #[test]
    fn render_side_never_drops_the_last_buffer_reference() {
        let (engine, mut r) = AudioEngine::offline(48_000, 2);
        let buf = SoundBuffer::from_samples(
            AudioFormat {
                sample_rate: 48_000,
                channels: 1,
            },
            vec![0.5; 256],
        );
        let weak = Arc::downgrade(&buf.data);
        let voice = engine.play(&buf, VoiceParams::new());
        drop(buf);
        drop(voice);
        let _ = r.render_frames(QUANTUM * 4);
        assert!(
            weak.upgrade().is_some(),
            "the handle side still owns the PCM"
        );
        assert_eq!(engine.active_voices(), 0); // maintenance pass
        assert!(weak.upgrade().is_none(), "freed on the handle side");
    }

    /// A stolen / rejected voice's `Arc`s are also freed handle-side.
    #[test]
    fn rejected_voice_is_released_on_the_handle_side() {
        let (engine, mut r) = AudioEngine::offline(48_000, 2);
        let buf = SoundBuffer::from_samples(
            AudioFormat {
                sample_rate: 48_000,
                channels: 1,
            },
            vec![0.5; 4],
        );
        let weak = Arc::downgrade(&buf.data);
        // Starts past the end: rejected at drain time.
        engine.play(&buf, VoiceParams::new().start_offset(1.0));
        drop(buf);
        let _ = r.render_frames(QUANTUM);
        assert!(weak.upgrade().is_some());
        engine.active_voices();
        assert!(weak.upgrade().is_none());
    }

    /// Commands queued past the queue's capacity (output stalled) are kept
    /// in order and delivered once there is room.
    #[test]
    fn overflowing_commands_are_delivered_in_order() {
        let (engine, mut r) = AudioEngine::offline(48_000, 2);
        let buf = SoundBuffer::from_samples(
            AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            vec![1.0; 96_000],
        );
        let v = engine.play(&buf, VoiceParams::new().looping(true));
        for i in 0..COMMAND_CAPACITY + 50 {
            v.set_gain(i as f32 / 10_000.0, 0.0);
        }
        v.set_gain(0.25, 0.0);
        // First render drains one queue's worth; later calls flush the rest.
        let _ = r.render_frames(QUANTUM);
        engine.active_voices();
        let out = r.render_frames(QUANTUM);
        assert!(
            out.iter().all(|&s| s == 0.25),
            "last command wins: {}",
            out[0]
        );
    }
}
