//! Web backend: the browser's own Web Audio graph is the mixer.
//!
//! # Graph
//!
//! ```text
//! per voice:  AudioBufferSourceNode (playbackRate, loop)        ┐
//!             or MediaStreamAudioSourceNode (stream voices)     │
//!               → GainNode (gain)                               │  same order as
//!               → BiquadFilterNode ("lowpass", Q 1, frequency)  │  the Rust mixer
//!               → StereoPannerNode (pan)                        │  (`mixer.rs`)
//! engine:     → GainNode (master) → destination                 ┘
//! ```
//!
//! Every formula the Rust mixer transcribes (`dsp.rs`) is the spec's
//! definition of one of these nodes, so the two converge on the same
//! samples; `web/tests.rs` renders one scene through both (an
//! `OfflineAudioContext` and `AudioEngine::offline`) and compares them.
//!
//! # How each semantic is met
//!
//! - **Low-pass `None`** keeps the biquad in the chain with its frequency
//!   at Nyquist, where the spec's (and Chromium's) lowpass is exactly the
//!   identity — the same trick the mixer uses, so toggling it is a cutoff
//!   ramp, never a re-wire, and never clicks.
//! - **Ramps** are `cancelScheduledValues(now)` + `setValueAtTime(current,
//!   now)` + `linearRampToValueAtTime(target, now + secs)`. `current` is
//!   computed here from the last ramp this backend scheduled ([`Track`])
//!   rather than read from `AudioParam.value`, which only reports the value
//!   of the last rendered quantum and so lags a running ramp — the mixer
//!   anchors a new ramp at the exact current value, and so does this.
//! - **`stop(fade)`** ramps the gain to 0 and schedules the source's
//!   `stop(now + fade)`; its `ended` event frees the voice. Stream voices
//!   have no `stop`, so a silent `ConstantSourceNode` (offset 0, summed into
//!   the voice's gain node — adding exact zeros) is the clock whose `ended`
//!   tears them down, on the audio clock like the buffer case.
//! - **`is_playing`** goes false on `ended` (played out / stopped), when a
//!   voice is stolen, or when the engine closes.
//! - **Voice cap / stealing** is the mixer's rule, applied at `play` time
//!   (the mixer applies it when it drains the command, one quantum later).
//! - **Buffers** are uploaded into an `AudioBuffer` once per `SoundBuffer`
//!   per context (cache keyed by the PCM `Arc`'s identity, validated with a
//!   `Weak` so a freed-and-reused address can't hit a stale entry), so rapid
//!   replays create only nodes.
//!
//! # Where the web differs (mechanism, not behavior)
//!
//! - Stream voices play the stream's `native_source()` `MediaStream`
//!   through a `MediaStreamAudioSourceNode`. For a capture stream (a
//!   microphone) that is the browser's own track — no copy, lowest latency.
//!   For a synthetic stream (a `synth::Mixer`), `media-stream`'s WebAudio
//!   bridge produces that track, and its buffering (a ~43 ms
//!   `ScriptProcessorNode` cushion, 1 s backlog cap) — not
//!   `STREAM_BACKLOG_SECS` — governs the latency.
//! - A played-out voice leaves `active_voices()` when its `ended` event is
//!   delivered (a task after the audio thread finishes it), not at the
//!   render quantum.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak as ArcWeak};
use std::time::Duration;

use media_stream::AudioStream;
use web_glue::{Closure, JsValue};

use crate::buffer::BufferData;
use crate::mixer::{VoiceOp, VoiceShared, MAX_VOICES};
use crate::{EngineError, SoundBuffer, VoiceParams};

use super::EngineBackend;

#[cfg(test)]
mod tests;

web_glue::import! {
    // ---- contexts --------------------------------------------------------
    // `new AudioContext({ latencyHint: "interactive" })` — the smallest
    // buffer the platform offers. Slot 0 (undefined) when Web Audio is
    // missing.
    #[catch]
    fn js_ctx_new() -> u32 =
        "() => { const C = globalThis.AudioContext || globalThis.webkitAudioContext; \
           return C ? G.add(new C({ latencyHint: 'interactive' })) : 0; }";
    fn js_ctx_rate(c: u32) -> f64 = "(c) => G.get(c).sampleRate";
    fn js_ctx_time(c: u32) -> f64 = "(c) => G.get(c).currentTime";
    // 0 suspended, 1 running, 2 closed (anything else).
    fn js_ctx_state(c: u32) -> u32 =
        "(c) => { const s = G.get(c).state; return s === 'suspended' ? 0 : s === 'running' ? 1 : 2; }";
    // `baseLatency + outputLatency` in seconds; -1 when neither is reported
    // (an OfflineAudioContext, older Safari). A missing `outputLatency`
    // counts as 0.
    fn js_ctx_latency(c: u32) -> f64 =
        "(c) => { const x = G.get(c); const b = x.baseLatency, o = x.outputLatency; \
           if (typeof b !== 'number' && typeof o !== 'number') return -1; \
           return (typeof b === 'number' ? b : 0) + (typeof o === 'number' ? o : 0); }";
    // `resume()` / `suspend()` / `close()` return Promises; their
    // rejections (no user gesture yet, already closed) are observed here.
    fn js_ctx_resume(c: u32) =
        "(c) => { const x = G.get(c); if (x.resume) { const p = x.resume(); if (p) p.catch(() => {}); } }";
    fn js_ctx_suspend(c: u32) =
        "(c) => { const x = G.get(c); if (x.suspend && !(x instanceof OfflineAudioContext)) { \
           const p = x.suspend(); if (p) p.catch(() => {}); } }";
    fn js_ctx_close(c: u32) =
        "(c) => { const x = G.get(c); if (x.close) { const p = x.close(); if (p) p.catch(() => {}); } }";

    // ---- nodes -----------------------------------------------------------
    // A GainNode at `v`, connected to `dest` (slot 0 = the destination).
    fn js_gain_new(c: u32, v: f64, dest: u32) -> u32 =
        "(c, v, d) => { const x = G.get(c); const n = x.createGain(); n.gain.value = v; \
           n.connect(d ? G.get(d) : x.destination); return G.add(n); }";
    // A lowpass BiquadFilterNode, Q = 1 (the spec default, in dB for
    // lowpass), cutoff `f`, connected to `dest`.
    fn js_lowpass_new(c: u32, f: f64, dest: u32) -> u32 =
        "(c, f, d) => { const n = G.get(c).createBiquadFilter(); n.type = 'lowpass'; \
           n.Q.value = 1; n.frequency.value = f; n.connect(G.get(d)); return G.add(n); }";
    #[catch]
    fn js_panner_new(c: u32, pan: f64, dest: u32) -> u32 =
        "(c, p, d) => { const n = G.get(c).createStereoPanner(); n.pan.value = p; \
           n.connect(G.get(d)); return G.add(n); }";
    // An AudioBuffer of `n` frames × `ch` channels at `r` Hz, filled from
    // `ch`-wide interleaved f32s at `p`. The view is made and used inside
    // this one call (memory may grow between calls).
    #[catch]
    fn js_buffer_new(c: u32, ch: u32, n: u32, r: f64, p: usize) -> u32 =
        "(c, ch, n, r, p) => { const b = G.get(c).createBuffer(ch, n, r); \
           const src = new Float32Array(G.u8().buffer, p >>> 0, n * ch); \
           for (let k = 0; k < ch; k++) { const d = new Float32Array(n); \
             for (let i = 0; i < n; i++) d[i] = src[i * ch + k]; b.copyToChannel(d, k); } \
           return G.add(b); }";
    // An AudioBufferSourceNode on `b`, connected to `dest`, NOT started.
    fn js_source_new(c: u32, b: u32, rate: f64, looping: u32, dest: u32) -> u32 =
        "(c, b, r, l, d) => { const n = G.get(c).createBufferSource(); n.buffer = G.get(b); \
           n.playbackRate.value = r; n.loop = l !== 0; n.connect(G.get(d)); return G.add(n); }";
    fn js_source_start(s: u32, when: f64, offset: f64) =
        "(s, w, o) => G.get(s).start(w, o)";
    // `stop(when)`; a later call replaces an earlier one (spec), and a
    // node that already ended ignores it — older engines throw instead.
    fn js_source_stop(s: u32, when: f64) =
        "(s, w) => { try { G.get(s).stop(w); } catch (_) {} }";
    fn js_source_loop(s: u32, on: u32) = "(s, on) => { G.get(s).loop = on !== 0; }";
    fn js_onended(s: u32, f: u32) = "(s, f) => { G.get(s).onended = f ? G.get(f) : null; }";
    // A ConstantSourceNode at offset 0 summed into `dest` — adds exact
    // zeros; it exists for its `ended` event (the stream-voice stop clock).
    fn js_timer_new(c: u32, dest: u32, when: f64) -> u32 =
        "(c, d, w) => { const n = G.get(c).createConstantSource(); n.offset.value = 0; \
           n.connect(G.get(d)); n.start(w); return G.add(n); }";
    // `createMediaStreamSource(stream)` connected to `dest`. Throws when the
    // stream has no audio track.
    #[catch]
    fn js_stream_source_new(c: u32, ms: u32, dest: u32) -> u32 =
        "(c, m, d) => { const n = G.get(c).createMediaStreamSource(G.get(m)); \
           n.connect(G.get(d)); return G.add(n); }";
    fn js_disconnect(n: u32) = "(n) => { try { G.get(n).disconnect(); } catch (_) {} }";

    // ---- automation ------------------------------------------------------
    // `which`: 0 gain, 1 frequency, 2 pan, 3 playbackRate.
    // Jump to `v` at `t`, dropping anything scheduled from `t` on.
    fn js_param_set(n: u32, which: u32, v: f64, t: f64) =
        "(n, w, v, t) => { const p = G.get(n)[['gain', 'frequency', 'pan', 'playbackRate'][w]]; \
           p.cancelScheduledValues(t); p.setValueAtTime(v, t); }";
    // Linear ramp `from`@`t0` → `to`@`t1`, replacing anything from `t0` on.
    fn js_param_ramp(n: u32, which: u32, from: f64, t0: f64, to: f64, t1: f64) =
        "(n, w, a, t0, b, t1) => { const p = G.get(n)[['gain', 'frequency', 'pan', 'playbackRate'][w]]; \
           p.cancelScheduledValues(t0); p.setValueAtTime(a, t0); p.linearRampToValueAtTime(b, t1); }";
}

const P_GAIN: u32 = 0;
const P_FREQ: u32 = 1;
const P_PAN: u32 = 2;
const P_RATE: u32 = 3;

/// The schedule this backend last put on one `AudioParam`: a linear segment
/// `from`@`t0` → `to`@`t1`, holding `to` after. Gives the exact current
/// value to anchor the next ramp on (see the module docs).
#[derive(Clone, Copy, Debug)]
struct Track {
    from: f64,
    t0: f64,
    to: f64,
    t1: f64,
}

impl Track {
    fn at(v: f64) -> Track {
        Track { from: v, t0: 0.0, to: v, t1: 0.0 }
    }

    fn value(&self, t: f64) -> f64 {
        if t >= self.t1 || self.t1 <= self.t0 {
            self.to
        } else if t <= self.t0 {
            self.from
        } else {
            self.from + (self.to - self.from) * (t - self.t0) / (self.t1 - self.t0)
        }
    }

    fn settled(&self, t: f64) -> bool {
        t >= self.t1
    }
}

/// Ramp `node`'s `which` param from its current value to `to` over `secs`
/// (`0` = jump), starting at `now`.
fn ramp(node: &JsValue, which: u32, track: &mut Track, to: f64, secs: f32, now: f64) {
    if secs <= 0.0 {
        // SAFETY: a live node handle owning the named param.
        unsafe { js_param_set(node.raw(), which, to, now) };
        *track = Track::at(to);
    } else {
        let from = track.value(now);
        let t1 = now + secs as f64;
        // SAFETY: as above.
        unsafe { js_param_ramp(node.raw(), which, from, now, to, t1) };
        *track = Track { from, t0: now, to, t1 };
    }
}

enum VoiceSource {
    Buffer(JsValue),
    /// The stream source node, and the stop clock once `stop` scheduled one.
    Stream {
        node: JsValue,
        timer: Option<JsValue>,
    },
}

struct WebVoice {
    id: u64,
    /// Start order, for "steal the oldest".
    seq: u64,
    shared: Arc<VoiceShared>,
    source: VoiceSource,
    gain: JsValue,
    filter: JsValue,
    panner: JsValue,
    gain_t: Track,
    freq_t: Track,
    pan_t: Track,
    rate_t: Track,
    looping: bool,
    stopping: bool,
    /// The `ended` listener (on the buffer source, or the stream's stop
    /// clock). Detached before it drops.
    on_ended: Option<Closure>,
}

impl WebVoice {
    fn is_buffer(&self) -> bool {
        matches!(self.source, VoiceSource::Buffer(_))
    }

    /// The node whose `ended` frees this voice, if any.
    fn ended_node(&self) -> Option<&JsValue> {
        match &self.source {
            VoiceSource::Buffer(s) => Some(s),
            VoiceSource::Stream { timer, .. } => timer.as_ref(),
        }
    }

    /// Silence and disconnect everything now, detach the `ended` listener,
    /// and report not playing. The `Closure` is returned so the caller can
    /// drop it OUTSIDE the engine's state borrow (it may be the closure
    /// currently running).
    fn kill(mut self) -> Option<Closure> {
        // Listener first: the node may still deliver a queued `ended`, and
        // it must not reach a closure that is about to be dropped.
        if let Some(n) = self.ended_node() {
            // SAFETY: live node handle; slot 0 clears the listener.
            unsafe { js_onended(n.raw(), 0) };
        }
        // SAFETY (all): live node handles.
        match &self.source {
            VoiceSource::Buffer(s) => unsafe {
                js_source_stop(s.raw(), 0.0);
                js_disconnect(s.raw());
            },
            VoiceSource::Stream { node, timer } => unsafe {
                js_disconnect(node.raw());
                if let Some(t) = timer {
                    js_source_stop(t.raw(), 0.0);
                    js_disconnect(t.raw());
                }
            },
        }
        unsafe {
            js_disconnect(self.gain.raw());
            js_disconnect(self.filter.raw());
            js_disconnect(self.panner.raw());
        }
        self.shared.playing.store(false, Ordering::Release);
        self.on_ended.take()
    }
}

struct State {
    next_id: u64,
    next_seq: u64,
    voices: Vec<WebVoice>,
    /// `AudioBuffer`s already uploaded to this context, keyed by the PCM
    /// `Arc`'s address; the `Weak` proves the entry is still that `Arc`.
    buffers: Vec<(ArcWeak<BufferData>, JsValue)>,
    master_t: Track,
}

struct Inner {
    // Field order: nothing depends on drop order here (Drop for WebEngine
    // tears the graph down explicitly first).
    ctx: JsValue,
    master: JsValue,
    sample_rate: u32,
    offline: bool,
    /// `suspend()` was called and no `resume()` since: `play` must not
    /// auto-resume a context the app paused on purpose.
    user_suspended: Cell<bool>,
    state: RefCell<State>,
}

/// [`EngineBackend`] on a Web Audio context.
pub(crate) struct WebEngine {
    inner: Rc<Inner>,
}

pub(crate) async fn start() -> Result<Rc<dyn EngineBackend>, EngineError> {
    // SAFETY: a fresh slot, or 0 when the browser has no Web Audio.
    let raw = unsafe { js_ctx_new() }.map_err(|e| EngineError::Backend(format!("AudioContext: {e}")))?;
    if raw == 0 {
        return Err(EngineError::Unsupported);
    }
    // SAFETY: the fresh slot above.
    let ctx = unsafe { JsValue::from_raw(raw) };
    // A context made before any user gesture starts `suspended`; it is kept
    // and `resume()` (or the first `play` after `user_activated`) wakes it.
    Ok(Rc::new(WebEngine::new(ctx, false)))
}

impl WebEngine {
    fn new(ctx: JsValue, offline: bool) -> WebEngine {
        // SAFETY: live context handle; slot 0 = connect to the destination.
        let sample_rate = unsafe { js_ctx_rate(ctx.raw()) }.round().max(1.0) as u32;
        let master = unsafe { JsValue::from_raw(js_gain_new(ctx.raw(), 1.0, 0)) };
        WebEngine {
            inner: Rc::new(Inner {
                ctx,
                master,
                sample_rate,
                offline,
                user_suspended: Cell::new(false),
                state: RefCell::new(State {
                    next_id: 0,
                    next_seq: 0,
                    voices: Vec::new(),
                    buffers: Vec::new(),
                    master_t: Track::at(1.0),
                }),
            }),
        }
    }

    fn now(&self) -> f64 {
        // SAFETY: live context handle.
        unsafe { js_ctx_time(self.inner.ctx.raw()) }
    }

    fn nyquist(&self) -> f64 {
        self.inner.sample_rate as f64 / 2.0
    }

    /// Browsers start a context suspended until a user gesture. Once the
    /// app has seen one (`app_state::current().user_activated`), the first
    /// `play` resumes it — unless the app suspended it itself.
    fn auto_resume(&self) {
        let i = &self.inner;
        if i.offline || i.user_suspended.get() {
            return;
        }
        // SAFETY: live context handle.
        let suspended = unsafe { js_ctx_state(i.ctx.raw()) } == 0;
        if suspended && runtime_shared::app_state::current().user_activated {
            unsafe { js_ctx_resume(i.ctx.raw()) };
        }
    }

    /// The `AudioBuffer` for `data`, uploading it on first use.
    fn audio_buffer(&self, st: &mut State, data: &Arc<BufferData>) -> Option<JsValue> {
        st.buffers.retain(|(w, _)| w.strong_count() > 0);
        let ptr = Arc::as_ptr(data);
        if let Some((_, b)) = st.buffers.iter().find(|(w, _)| w.as_ptr() == ptr) {
            return Some(b.clone());
        }
        // SAFETY: live context; `samples` holds `frames * channels` f32s.
        let raw = unsafe {
            js_buffer_new(
                self.inner.ctx.raw(),
                data.channels as u32,
                data.frames as u32,
                data.sample_rate as f64,
                data.samples.as_ptr() as usize,
            )
        }
        .ok()?;
        let b = unsafe { JsValue::from_raw(raw) };
        st.buffers.push((Arc::downgrade(data), b.clone()));
        Some(b)
    }

    /// Make room under [`MAX_VOICES`]: the mixer's rule — steal the oldest
    /// non-looping buffer voice; `false` if nothing can be stolen.
    fn make_room(&self, st: &mut State, dead: &mut Vec<Closure>) -> bool {
        if st.voices.len() < MAX_VOICES {
            return true;
        }
        let victim = st
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.looping && v.is_buffer())
            .min_by_key(|(_, v)| v.seq)
            .map(|(i, _)| i);
        match victim {
            Some(i) => {
                dead.extend(st.voices.swap_remove(i).kill());
                true
            }
            None => false,
        }
    }

    /// Build gain → filter → panner (→ master) for a voice.
    fn chain(&self, params: &VoiceParams) -> (JsValue, JsValue, JsValue, f64) {
        let ctx = self.inner.ctx.raw();
        let nyquist = self.nyquist();
        let cutoff = params.lowpass_hz.map_or(nyquist, |hz| (hz as f64).min(nyquist));
        // SAFETY (all): live context / node handles; fresh slots back.
        unsafe {
            let panner = JsValue::from_raw(
                js_panner_new(ctx, params.pan as f64, self.inner.master.raw()).expect("StereoPannerNode"),
            );
            let filter = JsValue::from_raw(js_lowpass_new(ctx, cutoff, panner.raw()));
            let gain = JsValue::from_raw(js_gain_new(ctx, params.gain as f64, filter.raw()));
            (gain, filter, panner, cutoff)
        }
    }

    /// The listener that frees voice `id` when its node ends.
    fn ended_listener(&self, id: u64) -> Closure {
        let weak: Weak<Inner> = Rc::downgrade(&self.inner);
        Closure::new(move |_event| {
            let Some(inner) = weak.upgrade() else { return };
            let gone = {
                let mut st = inner.state.borrow_mut();
                st.voices.iter().position(|v| v.id == id).map(|i| st.voices.swap_remove(i))
            };
            // Outside the borrow: `kill` returns THIS closure, and dropping
            // a running closure is deferred by web-glue's registry.
            drop(gone.and_then(WebVoice::kill));
        })
    }

    fn rejected(&self, st: &mut State) -> (u64, Arc<VoiceShared>) {
        let id = st.next_id;
        st.next_id += 1;
        let shared = VoiceShared::new();
        shared.playing.store(false, Ordering::Release);
        (id, shared)
    }
}

impl Drop for WebEngine {
    fn drop(&mut self) {
        let voices = std::mem::take(&mut self.inner.state.borrow_mut().voices);
        let closures: Vec<Closure> = voices.into_iter().filter_map(WebVoice::kill).collect();
        drop(closures);
        // SAFETY: live handles.
        unsafe {
            js_disconnect(self.inner.master.raw());
            if !self.inner.offline {
                js_ctx_close(self.inner.ctx.raw());
            }
        }
    }
}

impl EngineBackend for WebEngine {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate
    }

    fn output_latency(&self) -> Option<Duration> {
        // SAFETY: live context handle.
        let secs = unsafe { js_ctx_latency(self.inner.ctx.raw()) };
        (secs >= 0.0 && secs.is_finite()).then(|| Duration::from_secs_f64(secs))
    }

    fn play(&self, buf: &SoundBuffer, params: VoiceParams) -> (u64, Arc<VoiceShared>) {
        self.auto_resume();
        let mut dead = Vec::new();
        let result = {
            let mut st = self.inner.state.borrow_mut();
            let data = &buf.data;
            let frames = data.frames as f64;
            // The mixer's start-offset rule: wrap for loops, nothing to play
            // past the end otherwise. (An empty buffer can't be an
            // AudioBuffer: it never plays.)
            let mut pos = params.start_offset_secs as f64 * data.sample_rate as f64;
            if params.looping && frames > 0.0 {
                pos %= frames;
            }
            if data.frames == 0 || pos >= frames || !self.make_room(&mut st, &mut dead) {
                self.rejected(&mut st)
            } else if let Some(audio_buffer) = self.audio_buffer(&mut st, data) {
                let now = self.now();
                let (gain, filter, panner, cutoff) = self.chain(&params);
                // SAFETY: live handles; a fresh slot back.
                let source = unsafe {
                    JsValue::from_raw(js_source_new(
                        self.inner.ctx.raw(),
                        audio_buffer.raw(),
                        params.rate as f64,
                        params.looping as u32,
                        gain.raw(),
                    ))
                };
                let id = st.next_id;
                st.next_id += 1;
                let seq = st.next_seq;
                st.next_seq += 1;
                let on_ended = self.ended_listener(id);
                unsafe {
                    js_onended(source.raw(), on_ended.as_js().raw());
                    js_source_start(source.raw(), now, pos / data.sample_rate as f64);
                }
                let shared = VoiceShared::new();
                st.voices.push(WebVoice {
                    id,
                    seq,
                    shared: shared.clone(),
                    source: VoiceSource::Buffer(source),
                    gain,
                    filter,
                    panner,
                    gain_t: Track::at(params.gain as f64),
                    freq_t: Track::at(cutoff),
                    pan_t: Track::at(params.pan as f64),
                    rate_t: Track::at(params.rate as f64),
                    looping: params.looping,
                    stopping: false,
                    on_ended: Some(on_ended),
                });
                (id, shared)
            } else {
                self.rejected(&mut st)
            }
        };
        drop(dead);
        result
    }

    fn play_stream(&self, stream: &AudioStream, params: VoiceParams) -> (u64, Arc<VoiceShared>) {
        self.auto_resume();
        let ms = stream.native_source().and_then(|src| {
            src.downcast_ref::<web_glue::dom::MediaStream>()
                .map(|m| m.as_js().clone())
                .or_else(|| src.downcast_ref::<JsValue>().cloned())
        });
        let mut dead = Vec::new();
        let result = {
            let mut st = self.inner.state.borrow_mut();
            match ms {
                Some(ms) if self.make_room(&mut st, &mut dead) => {
                    let (gain, filter, panner, cutoff) = self.chain(&params);
                    // SAFETY: live handles; a fresh slot back.
                    match unsafe { js_stream_source_new(self.inner.ctx.raw(), ms.raw(), gain.raw()) } {
                        Ok(raw) => {
                            let node = unsafe { JsValue::from_raw(raw) };
                            let id = st.next_id;
                            st.next_id += 1;
                            let seq = st.next_seq;
                            st.next_seq += 1;
                            let shared = VoiceShared::new();
                            st.voices.push(WebVoice {
                                id,
                                seq,
                                shared: shared.clone(),
                                source: VoiceSource::Stream { node, timer: None },
                                gain,
                                filter,
                                panner,
                                gain_t: Track::at(params.gain as f64),
                                freq_t: Track::at(cutoff),
                                pan_t: Track::at(params.pan as f64),
                                rate_t: Track::at(1.0),
                                // Never looping; never stolen (not a buffer).
                                looping: false,
                                stopping: false,
                                on_ended: None,
                            });
                            (id, shared)
                        }
                        Err(_) => {
                            // No audio track: unwire the chain we built.
                            unsafe {
                                js_disconnect(gain.raw());
                                js_disconnect(filter.raw());
                                js_disconnect(panner.raw());
                            }
                            self.rejected(&mut st)
                        }
                    }
                }
                _ => self.rejected(&mut st),
            }
        };
        drop(dead);
        result
    }

    fn voice(&self, id: u64, op: VoiceOp) {
        let now = self.now();
        let nyquist = self.nyquist();
        let mut dead = None;
        {
            let mut st = self.inner.state.borrow_mut();
            let Some(idx) = st.voices.iter().position(|v| v.id == id) else {
                return;
            };
            match op {
                VoiceOp::Stop { fade_secs } => {
                    // The mixer cuts when the fade is under one frame.
                    if crate::dsp::secs_to_frames(fade_secs, self.inner.sample_rate) == 0 {
                        dead = st.voices.swap_remove(idx).kill();
                    } else {
                        let ctx = self.inner.ctx.raw();
                        let v = &mut st.voices[idx];
                        v.stopping = true;
                        ramp(&v.gain, P_GAIN, &mut v.gain_t, 0.0, fade_secs, now);
                        let end = now + fade_secs as f64;
                        match &mut v.source {
                            // SAFETY: live source handle.
                            VoiceSource::Buffer(s) => unsafe { js_source_stop(s.raw(), end) },
                            VoiceSource::Stream { timer, .. } => {
                                // A repeated stop replaces the clock.
                                if let Some(t) = timer.take() {
                                    unsafe {
                                        js_onended(t.raw(), 0);
                                        js_disconnect(t.raw());
                                    }
                                }
                                // SAFETY: live handles; a fresh slot back.
                                let t = unsafe { JsValue::from_raw(js_timer_new(ctx, v.gain.raw(), now)) };
                                let on_ended = self.ended_listener(id);
                                unsafe {
                                    js_onended(t.raw(), on_ended.as_js().raw());
                                    js_source_stop(t.raw(), end);
                                }
                                *timer = Some(t);
                                dead = v.on_ended.replace(on_ended);
                            }
                        }
                    }
                }
                VoiceOp::Gain { gain, ramp_secs } => {
                    let v = &mut st.voices[idx];
                    // A stopping voice's gain belongs to its fade.
                    if !v.stopping {
                        ramp(&v.gain, P_GAIN, &mut v.gain_t, gain as f64, ramp_secs, now);
                    }
                }
                VoiceOp::Pan { pan, ramp_secs } => {
                    let v = &mut st.voices[idx];
                    ramp(&v.panner, P_PAN, &mut v.pan_t, pan as f64, ramp_secs, now);
                }
                VoiceOp::Rate { rate, ramp_secs } => {
                    let v = &mut st.voices[idx];
                    if let VoiceSource::Buffer(s) = &v.source {
                        ramp(s, P_RATE, &mut v.rate_t, rate as f64, ramp_secs, now);
                    }
                }
                VoiceOp::Lowpass { hz, ramp_secs } => {
                    let v = &mut st.voices[idx];
                    // `None` = the cutoff at Nyquist (the identity). Ramping
                    // from the tracked current value covers every case the
                    // mixer distinguishes: an unfiltered voice sits at
                    // Nyquist, so `Some` enters from there; a voice mid-way
                    // through turning off continues from where it is.
                    let target = hz.map_or(nyquist, |hz| (hz as f64).min(nyquist));
                    let idle = v.freq_t.settled(now) && v.freq_t.to == target;
                    if !idle {
                        ramp(&v.filter, P_FREQ, &mut v.freq_t, target, ramp_secs, now);
                    }
                }
                VoiceOp::Looping(looping) => {
                    let v = &mut st.voices[idx];
                    if let VoiceSource::Buffer(s) = &v.source {
                        v.looping = looping;
                        // SAFETY: live source handle.
                        unsafe { js_source_loop(s.raw(), looping as u32) };
                    }
                }
            }
        }
        drop(dead);
    }

    fn set_master_gain(&self, gain: f32, ramp_secs: f32) {
        let now = self.now();
        let mut st = self.inner.state.borrow_mut();
        ramp(&self.inner.master, P_GAIN, &mut st.master_t, gain as f64, ramp_secs, now);
    }

    fn resume(&self) {
        self.inner.user_suspended.set(false);
        if !self.inner.offline {
            // SAFETY: live context handle.
            unsafe { js_ctx_resume(self.inner.ctx.raw()) };
        }
    }

    fn suspend(&self) {
        self.inner.user_suspended.set(true);
        if !self.inner.offline {
            // SAFETY: live context handle.
            unsafe { js_ctx_suspend(self.inner.ctx.raw()) };
        }
    }

    fn active_voices(&self) -> usize {
        self.inner.state.borrow().voices.len()
    }
}
