//! Web-only: turn a *synthetic* [`AudioStream`] (one fed by an
//! [`AudioWriter`](crate::AudioWriter) rather than backed by a real
//! `getUserMedia`/`<audio>` source) into a live `MediaStream`
//! (`web_glue::dom::MediaStream`) with a real audio track.
//!
//! # Why this exists
//!
//! On native, an `AudioStream` is fully interchangeable everywhere: every
//! consumer (the file recorder, playback) pulls PCM frames via
//! [`subscribe`](crate::AudioStream::subscribe), so a mic-backed stream and a
//! stream synthesized from PCM (a denoiser's output, a mixer's output) are the
//! same thing.
//!
//! On web they are *not* the same thing. Browser sinks — `MediaRecorder`,
//! `<audio srcObject>`, WebRTC — consume a `MediaStreamTrack` *object*, not PCM
//! callbacks. A capture stream (mic/camera) publishes its live
//! `MediaStream` (`web_glue::dom::MediaStream`) as the stream's
//! [`native_source`](crate::AudioStream::native_source), so those sinks bind to
//! it directly. A *synthetic* stream has no such object, so before this bridge
//! `native_source()` returned `None` and e.g. recording a denoised or mixed mic
//! failed with `Unsupported`. That broke the `AudioStream` promise of "plug into
//! inputs/outputs on every platform".
//!
//! # What it does
//!
//! [`AudioStream::native_source`](crate::AudioStream::native_source), on web,
//! lazily builds one of these bridges the first time a consumer asks for a
//! native handle on a synthetic stream, then caches it. The bridge:
//!
//! 1. opens an `AudioContext`,
//! 2. creates a `MediaStreamAudioDestinationNode` — its `.stream` is a real
//!    `MediaStream` with a live audio track (this is what `native_source()`
//!    returns),
//! 3. drives a `ScriptProcessorNode` into that destination, filling each
//!    render quantum from a ring buffer that a [`subscribe`] callback feeds with
//!    the stream's PCM frames (remixed / resampled to the graph's format).
//!
//! Because the destination's track is live immediately, a consumer gets a valid
//! (initially silent) track synchronously; audio starts flowing the moment the
//! first PCM frame arrives.
//!
//! # Why `ScriptProcessorNode` (deprecated) and not `AudioWorklet`
//!
//! `native_source()` is a **synchronous** accessor, and every consumer relies on
//! that. `AudioWorklet` requires `audioWorklet.addModule(url)` — asynchronous,
//! and needing a separately-served JS module — so adopting it would force
//! `native_source()` (and thus every caller) to become async. `ScriptProcessor`
//! is constructed and connected synchronously with no external asset, so the
//! accessor stays sync. It's deprecated but universally supported; the upgrade
//! path (an inline-Blob AudioWorklet) is a self-contained follow-up that doesn't
//! change this module's callers.
//!
//! # Clock caveat
//!
//! The bridged track runs on WebAudio's own clock, independent of the source
//! stream's `pts_micros`. Fine for capture and one-way playback; note it if you
//! ever need tight lip-sync against the *original* source timeline.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use web_glue::dom::MediaStream;
use web_glue::{Closure, JsCast, JsValue};

use crate::audio::{AudioFrame, AudioStream, AudioSubscription};

// The WebAudio graph, as web-glue bindings. Every node is a plain handle;
// the graph shape (context → script processor → stream destination) lives in
// the snippets.
web_glue::import! {
    // `new AudioContext({ sampleRate })`, resumed in case the autoplay policy
    // started it suspended (recording is triggered by a user gesture, so the
    // context is allowed to run). Throws when WebAudio is unavailable or the
    // rate is refused.
    #[catch]
    fn js_ctx_new(rate: f64) -> u32 =
        "(r) => { const c = new AudioContext({ sampleRate: r }); \
           const p = c.resume(); if (p) p.catch(() => {}); return G.add(c); }";
    // `ctx.createMediaStreamDestination()`.
    #[catch]
    fn js_ctx_dest(c: u32) -> u32 = "(c) => G.add(G.get(c).createMediaStreamDestination())";
    // `dest.stream` — the live `MediaStream` whose audio track sinks bind to.
    fn js_dest_stream(d: u32) -> u32 = "(d) => G.add(G.get(d).stream)";
    // `ctx.createScriptProcessor(buffer, 1, outChannels)`.
    #[catch]
    fn js_proc_new(c: u32, buffer: u32, out: u32) -> u32 =
        "(c, b, o) => G.add(G.get(c).createScriptProcessor(b, 1, o))";
    fn js_proc_channels(p: u32) -> u32 = "(p) => G.get(p).channelCount >>> 0";
    // `onaudioprocess = f`, then connect into the destination so its track
    // carries the processor's output.
    fn js_proc_attach(p: u32, f: u32, d: u32) =
        "(p, f, d) => { const n = G.get(p); n.onaudioprocess = G.get(f); n.connect(G.get(d)); }";
    // Teardown: handler FIRST (see `Drop for WebAudioBridge`), then the node.
    fn js_proc_detach(p: u32) =
        "(p) => { const n = G.get(p); n.onaudioprocess = null; try { n.disconnect(); } catch (_) {} }";
    // `close()` returns a Promise; teardown is fire-and-forget.
    fn js_ctx_close(c: u32) = "(c) => { const p = G.get(c).close(); if (p) p.catch(() => {}); }";
    // An `AudioProcessingEvent`'s output buffer shape: channels * 2^20 +
    // frames (frames < 2^20, channels ≤ 32 per the Web Audio spec).
    fn js_out_shape(e: u32) -> f64 =
        "(e) => { const b = G.get(e).outputBuffer; return b == null ? 0 : \
           b.numberOfChannels * 1048576 + b.length; }";
    // Copy `ch` planar channels of `n` f32s at `p` (channel c at p + c*n*4)
    // into the event's output buffer. The views are made and used inside
    // this one call (memory may grow between calls).
    fn js_out_fill(e: u32, p: usize, ch: u32, n: u32) =
        "(e, p, ch, n) => { const b = G.get(e).outputBuffer; const base = p >>> 0; \
           for (let c = 0; c < ch; c++) \
             b.copyToChannel(new Float32Array(G.u8().buffer, base + c * n * 4, n), c); }";
}

/// ScriptProcessor render-quantum size (power of two, 256..=16384). ~43 ms at
/// 48 kHz — a balance between callback overhead and latency.
const BUFFER_SIZE: u32 = 2048;

/// Don't drain the ring until it holds at least this many frames, so brief
/// producer jitter doesn't underrun into audible gaps. One buffer of cushion.
const PRIME_FRAMES: usize = BUFFER_SIZE as usize;

/// Hard cap on buffered frames per channel (~1 s at 48 kHz). If a sink ever
/// consumes slower than the producer, drop the oldest rather than grow without
/// bound. In practice the graph is rate-matched, so this never trips.
const MAX_BACKLOG_FRAMES: usize = 48_000;

/// Bit layout of [`js_out_shape`]: channels above, frames below.
const SHAPE_CHANNEL_SHIFT: f64 = 1_048_576.0;

/// Per-output-channel PCM waiting to be rendered, plus the primed latch.
struct Ring {
    /// De-interleaved samples, one queue per graph output channel.
    chans: Vec<VecDeque<f32>>,
    /// Set once the ring has filled past [`PRIME_FRAMES`]; before that the
    /// render callback emits silence to build a cushion.
    primed: bool,
}

impl Ring {
    fn new(channels: usize) -> Ring {
        Ring {
            chans: (0..channels.max(1)).map(|_| VecDeque::new()).collect(),
            primed: false,
        }
    }
}

/// Mutable graph state shared between `native_source`'s setup and the per-frame
/// fill callback. The `ScriptProcessorNode` is built lazily on the first frame
/// (that's when the channel count is known), so it starts as `None`.
struct Graph {
    /// The `AudioContext`.
    ctx: JsValue,
    /// The `MediaStreamAudioDestinationNode`.
    dest: JsValue,
    /// The `ScriptProcessorNode`, once the first frame has fixed the channel
    /// count.
    proc: Option<JsValue>,
    /// Kept alive alongside `proc`: its `onaudioprocess` handler.
    on_audio: Option<Closure>,
    /// The graph's sample rate (the `AudioContext`'s rate); PCM at a different
    /// rate is resampled to it before entering the ring.
    ctx_rate: u32,
}

/// Live WebAudio graph backing a synthetic [`AudioStream`]'s native track.
/// Held inside the `AudioStream` so it lives exactly as long as the stream; the
/// last drop tears the graph down.
pub(crate) struct WebAudioBridge {
    /// The `MediaStream` whose audio track carries the rendered PCM — the value
    /// `native_source()` hands out (downcast to `web_glue::dom::MediaStream`).
    stream: MediaStream,
    /// Feeds the ring; dropping it detaches from the source stream.
    _sub: AudioSubscription,
    /// Shared with the render callback; kept here so the graph can be closed.
    graph: Rc<RefCell<Graph>>,
}

impl WebAudioBridge {
    /// The bridged `MediaStream` (its audio track is what sinks record/play).
    /// A handle clone: the same JS stream.
    pub(crate) fn media_stream(&self) -> MediaStream {
        self.stream.clone()
    }
}

impl Drop for WebAudioBridge {
    fn drop(&mut self) {
        // Release the audio device. ORDER MATTERS: detach the `onaudioprocess`
        // handler FIRST. The node can still fire one queued render callback
        // during teardown, and if the Rust `Closure` (dropped right after this,
        // when `Graph` drops) were gone by then, that late call would throw
        // "called after its Rust owner dropped it". Clearing the handler drops
        // JS's reference to the closure up front, so no late call can land.
        // Then disconnect the node and close the context (both fire-and-forget).
        let g = self.graph.borrow();
        if let Some(proc) = &g.proc {
            // SAFETY: a live node handle.
            unsafe { js_proc_detach(proc.raw()) };
        }
        // SAFETY: a live context handle.
        unsafe { js_ctx_close(g.ctx.raw()) };
    }
}

/// Build a bridge for `source`, returning it (with its live `MediaStream`) or
/// `None` if the WebAudio graph could not be created. `rate_hint` seeds the
/// `AudioContext` sample rate (the synthetic producers here are 48 kHz); PCM at
/// any other rate is resampled to it, so the hint need only be close.
pub(crate) fn build(source: &AudioStream, rate_hint: u32) -> Option<WebAudioBridge> {
    let ctx_rate = if rate_hint == 0 { 48_000 } else { rate_hint };

    // SAFETY (all three): handles the previous call just returned.
    let ctx = unsafe { JsValue::from_raw(js_ctx_new(ctx_rate as f64).ok()?) };
    let dest = unsafe { JsValue::from_raw(js_ctx_dest(ctx.raw()).ok()?) };
    let stream: MediaStream = unsafe { JsValue::from_raw(js_dest_stream(dest.raw())) }.unchecked_into();

    let graph = Rc::new(RefCell::new(Graph {
        ctx,
        dest,
        proc: None,
        on_audio: None,
        ctx_rate,
    }));
    let ring: Rc<RefCell<Ring>> = Rc::new(RefCell::new(Ring::new(1)));

    // Per-channel linear resampler state (only engaged when a frame's rate
    // differs from the graph rate — rare, since producers here emit 48 kHz).
    let mut resampler = Resampler::new();

    let sub = {
        let graph = graph.clone();
        let ring = ring.clone();
        source.subscribe(move |f: &AudioFrame| {
            let out_channels = ensure_proc(&graph, &ring, f.channels);
            let ctx_rate = graph.borrow().ctx_rate;

            // 1. Remix the interleaved input to the graph's channel count.
            let per_ch = remix(f.samples, f.channels as usize, out_channels);
            // 2. Resample each channel to the graph rate if needed.
            let per_ch = resampler.process(per_ch, f.sample_rate, ctx_rate);

            // 3. Enqueue, dropping the oldest if a stalled sink lets it grow.
            let mut r = ring.borrow_mut();
            for (c, samples) in per_ch.iter().enumerate() {
                if let Some(q) = r.chans.get_mut(c) {
                    q.extend(samples.iter().copied());
                    while q.len() > MAX_BACKLOG_FRAMES {
                        q.pop_front();
                    }
                }
            }
        })
    };

    Some(WebAudioBridge { stream, _sub: sub, graph })
}

/// Ensure the `ScriptProcessorNode` exists (built lazily once the first frame
/// fixes the channel count) and return its output channel count. Idempotent.
fn ensure_proc(graph: &Rc<RefCell<Graph>>, ring: &Rc<RefCell<Ring>>, in_channels: u16) -> usize {
    {
        let g = graph.borrow();
        if let Some(proc) = &g.proc {
            // SAFETY: a live node handle.
            return unsafe { js_proc_channels(proc.raw()) } as usize;
        }
    }
    let out_channels = in_channels.max(1) as usize;
    *ring.borrow_mut() = Ring::new(out_channels);

    let mut g = graph.borrow_mut();
    // SAFETY: a live context handle; the result is a fresh node handle.
    let proc = match unsafe { js_proc_new(g.ctx.raw(), BUFFER_SIZE, out_channels as u32) } {
        Ok(p) => unsafe { JsValue::from_raw(p) },
        Err(_) => return out_channels,
    };

    // The render callback: fill each output channel from the ring, emitting
    // silence until primed or on underrun.
    let on_audio = {
        let ring = ring.clone();
        let mut planar: Vec<f32> = Vec::new();
        Closure::new(move |e: JsValue| {
            // SAFETY: the `AudioProcessingEvent` handed to this listener.
            let shape = unsafe { js_out_shape(e.raw()) };
            let channels = (shape / SHAPE_CHANNEL_SHIFT).floor() as usize;
            let len = (shape % SHAPE_CHANNEL_SHIFT) as usize;
            if channels == 0 || len == 0 {
                return;
            }
            let mut r = ring.borrow_mut();
            if !r.primed && r.chans.first().map_or(0, |q| q.len()) >= PRIME_FRAMES {
                r.primed = true;
            }
            let draining = r.primed;
            planar.clear();
            planar.resize(channels * len, 0.0);
            if draining {
                for (c, chunk) in planar.chunks_exact_mut(len).enumerate() {
                    if let Some(q) = r.chans.get_mut(c) {
                        for s in chunk.iter_mut() {
                            *s = q.pop_front().unwrap_or(0.0);
                        }
                    }
                }
            }
            // SAFETY: `planar` holds exactly `channels * len` f32s.
            unsafe { js_out_fill(e.raw(), planar.as_ptr() as usize, channels as u32, len as u32) };
        })
    };
    // SAFETY: live node / closure / destination handles.
    unsafe { js_proc_attach(proc.raw(), on_audio.as_js().raw(), g.dest.raw()) };

    g.proc = Some(proc);
    g.on_audio = Some(on_audio);
    out_channels
}

/// Remix interleaved `samples` (`in_ch` channels) to `out_ch` de-interleaved
/// channels. Mono fans out to all; stereo→mono averages; otherwise channels are
/// taken positionally (extra output channels reuse the last input channel).
fn remix(samples: &[f32], in_ch: usize, out_ch: usize) -> Vec<Vec<f32>> {
    let in_ch = in_ch.max(1);
    let out_ch = out_ch.max(1);
    let frames = samples.len() / in_ch;
    let mut out = vec![Vec::with_capacity(frames); out_ch];

    if in_ch == out_ch {
        for frame in samples.chunks_exact(in_ch) {
            for (c, s) in frame.iter().enumerate() {
                out[c].push(*s);
            }
        }
    } else if in_ch == 1 {
        for &s in samples {
            for ch in out.iter_mut() {
                ch.push(s);
            }
        }
    } else if out_ch == 1 {
        for frame in samples.chunks_exact(in_ch) {
            let sum: f32 = frame.iter().sum();
            out[0].push(sum / in_ch as f32);
        }
    } else {
        for frame in samples.chunks_exact(in_ch) {
            for (c, ch) in out.iter_mut().enumerate() {
                ch.push(frame[c.min(in_ch - 1)]);
            }
        }
    }
    out
}

/// A continuous-phase linear resampler, one phase/history per channel. Matches
/// the approach in `denoise`'s engine: dependency-free, gapless across chunks.
/// A no-op fast path passes through when the input already matches the target.
struct Resampler {
    rate: u32,
    phase: f64,
    prev: Vec<f32>,
}

impl Resampler {
    fn new() -> Resampler {
        Resampler { rate: 0, phase: 0.0, prev: Vec::new() }
    }

    /// Resample each channel of `input` from `src_rate` to `dst_rate`. State
    /// persists across calls; a change of `src_rate` or channel count restarts
    /// the timeline.
    fn process(&mut self, input: Vec<Vec<f32>>, src_rate: u32, dst_rate: u32) -> Vec<Vec<f32>> {
        if src_rate == 0 || dst_rate == 0 || src_rate == dst_rate {
            return input;
        }
        if self.rate != src_rate || self.prev.len() != input.len() {
            self.rate = src_rate;
            self.phase = 0.0;
            self.prev = vec![0.0; input.len()];
        }
        let step = src_rate as f64 / dst_rate as f64;
        let mut out = Vec::with_capacity(input.len());
        let mut last_phase = self.phase;
        for (c, chan) in input.iter().enumerate() {
            let n = chan.len();
            let prev = self.prev[c];
            let sample_at = |i: isize| -> f32 {
                if i < 0 {
                    prev
                } else {
                    chan[i as usize]
                }
            };
            let mut resampled = Vec::with_capacity(((n as f64) / step) as usize + 1);
            let mut t = self.phase;
            while t <= (n as f64) - 1.0 {
                let i0 = t.floor() as isize;
                let frac = (t - i0 as f64) as f32;
                let a = sample_at(i0);
                let b = sample_at(i0 + 1);
                resampled.push(a + (b - a) * frac);
                t += step;
            }
            last_phase = t - n as f64;
            if let Some(&s) = chan.last() {
                self.prev[c] = s;
            }
            out.push(resampled);
        }
        self.phase = last_phase;
        out
    }
}
