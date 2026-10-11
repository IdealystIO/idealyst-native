//! Browser tests for the Web Audio backend.
//!
//! Run: `cargo test -p audio-engine --target wasm32-unknown-unknown --lib`
//! with `CHROMEDRIVER=<driver matching Chrome>` and
//! `WASM_BINDGEN_TEST_ONLY_WEB=1` (the workspace runner supplies web-glue's
//! JS; never `wasm-pack test`).
//!
//! Most tests drive the graph on an `OfflineAudioContext`: it renders
//! faster than real time, needs no user gesture (a real context starts
//! suspended in an automated browser), and hands the samples back.

use std::rc::Rc;

use media_stream::{AudioFormat, AudioStream};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::{JsFuture, JsValue};

use super::{WebEngine, P_GAIN};
use crate::backend::EngineBackend;
use crate::{AudioEngine, SoundBuffer, VoiceParams, MAX_VOICES, QUANTUM};

wasm_bindgen_test_configure!(run_in_browser);

web_glue::import! {
    #[catch]
    fn js_offline_ctx_new(channels: u32, frames: u32, rate: f64) -> u32 =
        "(c, n, r) => G.add(new OfflineAudioContext({ numberOfChannels: c, length: n, sampleRate: r }))";
    // `startRendering()` → its Promise (resolves to the rendered AudioBuffer).
    #[catch]
    fn js_ctx_render(c: u32) -> u32 = "(c) => G.add(G.get(c).startRendering())";
    // `OfflineAudioContext.suspend(t)` → a Promise that resolves when
    // rendering reaches `t` (a quantum boundary); `resume()` continues.
    #[catch]
    fn js_offline_suspend(c: u32, t: f64) -> u32 = "(c, t) => G.add(G.get(c).suspend(t))";
    fn js_offline_resume(c: u32) = "(c) => { G.get(c).resume(); }";
    fn js_buffer_frames(b: u32) -> u32 = "(b) => G.get(b).length >>> 0";
    fn js_buffer_channels(b: u32) -> u32 = "(b) => G.get(b).numberOfChannels >>> 0";
    // Copy channel `k`'s first `n` frames into `n` f32s at `p`.
    fn js_buffer_read(b: u32, k: u32, p: usize, n: u32) =
        "(b, k, p, n) => G.get(b).copyFromChannel(new Float32Array(G.u8().buffer, p >>> 0, n), k)";
    fn js_sleep(ms: f64) -> u32 = "(ms) => G.add(new Promise((r) => setTimeout(r, ms)))";
    // The scheduled value of a param at time `t` is not readable; its
    // `value` after rendering is. `which` as in the backend.
    fn js_param_value(n: u32, which: u32) -> f64 =
        "(n, w) => G.get(n)[['gain', 'frequency', 'pan', 'playbackRate'][w]].value";
}

const RATE: u32 = 48_000;

/// A web engine on a fresh `OfflineAudioContext` (stereo, `frames` long).
fn offline_web(frames: usize) -> (AudioEngine, Rc<WebEngine>) {
    // SAFETY: a fresh slot back.
    let raw = unsafe { js_offline_ctx_new(2, frames as u32, RATE as f64) }.expect("OfflineAudioContext");
    let web = Rc::new(WebEngine::new(unsafe { JsValue::from_raw(raw) }, true));
    let backend: Rc<dyn EngineBackend> = web.clone();
    (AudioEngine { backend }, web)
}

fn ctx(web: &WebEngine) -> u32 {
    web.inner.ctx.raw()
}

/// Read a rendered AudioBuffer back as interleaved samples.
fn read_back(buf: &JsValue) -> Vec<f32> {
    // SAFETY: the rendered AudioBuffer.
    let (frames, ch) = unsafe { (js_buffer_frames(buf.raw()) as usize, js_buffer_channels(buf.raw()) as usize) };
    let mut planar = vec![0.0f32; frames];
    let mut out = vec![0.0f32; frames * ch];
    for k in 0..ch {
        // SAFETY: `planar` holds `frames` f32s.
        unsafe { js_buffer_read(buf.raw(), k as u32, planar.as_mut_ptr() as usize, frames as u32) };
        for (i, s) in planar.iter().enumerate() {
            out[i * ch + k] = *s;
        }
    }
    out
}

/// Render the whole offline context; `at` = (frame, ops) pairs run while
/// rendering is suspended at that frame (a quantum boundary), in order.
async fn render_web(web: &WebEngine, mut at: Vec<(usize, Box<dyn FnOnce()>)>) -> Vec<f32> {
    let c = ctx(web);
    let suspends: Vec<JsFuture> = at
        .iter()
        // SAFETY: live context; a fresh promise slot back.
        .map(|(frame, _)| {
            let t = *frame as f64 / RATE as f64;
            JsFuture::new(&unsafe { JsValue::from_raw(js_offline_suspend(c, t).expect("suspend(t)")) })
        })
        .collect();
    let rendering = JsFuture::new(&unsafe { JsValue::from_raw(js_ctx_render(c).expect("startRendering")) });
    for (s, (_, ops)) in suspends.into_iter().zip(at.drain(..)) {
        s.await.expect("suspend reached");
        ops();
        unsafe { js_offline_resume(c) };
    }
    let buf = rendering.await.expect("rendering");
    read_back(&buf)
}

async fn sleep(ms: f64) {
    let _ = JsFuture::new(&unsafe { JsValue::from_raw(js_sleep(ms)) }).await;
}

/// Deterministic white noise in [-amp, amp] (xorshift32).
fn noise(frames: usize, channels: usize, amp: f32, mut seed: u32) -> Vec<f32> {
    (0..frames * channels)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
        })
        .collect()
}

fn mono(rate: u32, samples: Vec<f32>) -> SoundBuffer {
    SoundBuffer::from_samples(AudioFormat { sample_rate: rate, channels: 1 }, samples)
}

/// Largest per-sample difference, and the reference's RMS.
fn compare(web: &[f32], core: &[f32]) -> (f32, usize, f32) {
    assert_eq!(web.len(), core.len());
    let mut worst = (0.0f32, 0usize);
    for (i, (a, b)) in web.iter().zip(core).enumerate() {
        // The browser destination clamps when it reaches the speakers; an
        // OfflineAudioContext's buffer is unclamped. The mixer clamps.
        let d = (a.clamp(-1.0, 1.0) - b).abs();
        if d > worst.0 {
            worst = (d, i);
        }
    }
    let rms = (core.iter().map(|s| s * s).sum::<f32>() / core.len() as f32).sqrt();
    (worst.0, worst.1, rms)
}

/// **The rule-7 proof.** One scene rendered through the browser's Web Audio
/// graph (this backend, on an `OfflineAudioContext`) and through the Rust
/// mixer (`AudioEngine::offline`) at the same rate; every sample must
/// agree.
///
/// The scene covers each semantic the two must share: a mono noise voice
/// with a 1 kHz low-pass, pan 0.5, rate 1.5 (k-rate: one value per
/// 128-frame quantum in both) and a gain ramp; a stereo noise voice at
/// 24 kHz (buffer-rate conversion) entering its filter from Nyquist
/// (`None → Some` ramp) with a pan ramp; a master-gain ramp; then, at a
/// quantum boundary mid-render, a ramp that interrupts a running one (the
/// new ramp must anchor on the exact current value), a filter-off ramp back
/// to Nyquist, and a rate change.
///
/// Tolerance 1e-5 (−100 dBFS; measured on Chrome 154: 1.2e-6). Both sides
/// compute in f32 with f64 biquad state, but not with identical operation
/// order — Chromium evaluates automation in double from `frame /
/// sampleRate`, and its panner's sin/cos come from a different libm — so a
/// few f32 ULPs per stage are expected, and the IIR filter carries that
/// rounding forward. The bound leaves ~8× headroom for other engines'
/// rounding while staying far below any real divergence: a wrong formula,
/// ramp anchor, quantum alignment or interpolation shows up as errors of
/// 1e-3 or more (the scene's RMS is ~0.1).
#[wasm_bindgen_test]
async fn web_graph_converges_with_the_rust_mixer() {
    const TOLERANCE: f32 = 1e-5;
    const FRAMES: usize = QUANTUM * 200; // 0.53 s
    const MID: usize = QUANTUM * 70;

    let a_buf = mono(RATE, noise(RATE as usize, 1, 0.3, 0x1234_5678));
    let b_buf = SoundBuffer::from_samples(
        AudioFormat { sample_rate: 24_000, channels: 2 },
        noise(24_000, 2, 0.3, 0x0bad_cafe),
    );

    fn begin(e: &AudioEngine, a: &SoundBuffer, b: &SoundBuffer) -> (crate::Voice, crate::Voice) {
        let va = e.play(a, VoiceParams::new().lowpass(1_000.0).pan(0.5).rate(1.5));
        va.set_gain(0.2, 0.25);
        let vb = e.play(b, VoiceParams::new().pan(-0.3).gain(0.8));
        vb.set_lowpass(Some(2_000.0), 0.1);
        vb.set_pan(0.4, 0.2);
        e.set_master_gain(0.8, 0.3);
        (va, vb)
    }
    fn middle(va: &crate::Voice, vb: &crate::Voice) {
        va.set_gain(0.9, 0.1); // interrupts the 0.25 s ramp
        va.set_rate(0.75, 0.0);
        vb.set_lowpass(None, 0.05); // back to Nyquist, click-free
    }

    // Rust mixer.
    let (core_engine, mut r) = AudioEngine::offline(RATE, 2);
    let (ca, cb) = begin(&core_engine, &a_buf, &b_buf);
    let mut core = r.render_frames(MID);
    middle(&ca, &cb);
    core.extend(r.render_frames(FRAMES - MID));

    // Web Audio graph.
    let (web_engine, web) = offline_web(FRAMES);
    let (wa, wb) = begin(&web_engine, &a_buf, &b_buf);
    let out = render_web(&web, vec![(MID, Box::new(move || middle(&wa, &wb)))]).await;

    let (diff, at, rms) = compare(&out, &core);
    web_glue::dom::console::log_1(&JsValue::from_str(&format!(
        "convergence: max |web − core| = {diff:e} at frame {} ch {} (scene RMS {rms:.4})",
        at / 2,
        at % 2
    )));
    assert!(rms > 0.03, "the scene must actually sound (RMS {rms})");
    assert!(
        diff <= TOLERANCE,
        "web and core diverge: {diff:e} at frame {} ch {} (web {}, core {})",
        at / 2,
        at % 2,
        out[at],
        core[at]
    );
}

/// One-shots end on their own (`ended` → not playing), loops keep going,
/// `stop(fade)` fades to silence and then frees the voice, and
/// `set_looping(false)` lets the current pass finish.
#[wasm_bindgen_test]
async fn play_stop_loop_and_ended_state() {
    let frames = QUANTUM * 150; // 0.4 s
    let (engine, web) = offline_web(frames);
    let short = mono(RATE, vec![0.25; 2_400]); // 50 ms
    let long = mono(RATE, vec![0.25; RATE as usize]); // 1 s

    let one_shot = engine.play(&short, VoiceParams::new());
    let looped = engine.play(&short, VoiceParams::new().looping(true));
    let unlooped = engine.play(&short, VoiceParams::new().looping(true));
    let faded = engine.play(&long, VoiceParams::new());
    let cut = engine.play(&long, VoiceParams::new());
    assert!(one_shot.is_playing() && looped.is_playing() && faded.is_playing());
    assert_eq!(engine.active_voices(), 5);

    cut.stop(0.0);
    assert!(!cut.is_playing(), "stop(0) cuts at once");
    assert_eq!(engine.active_voices(), 4);
    let fade_end = 0.1;
    faded.stop(fade_end as f32);
    faded.set_gain(1.0, 0.0); // ignored: the fade owns the gain
    unlooped.set_looping(false);

    let out = render_web(&web, Vec::new()).await;
    sleep(50.0).await; // `ended` events are delivered as tasks

    assert!(!one_shot.is_playing(), "a one-shot ends when played out");
    assert!(!unlooped.is_playing(), "set_looping(false) finishes the pass, then ends");
    assert!(!faded.is_playing(), "stop(fade) frees the voice after the fade");
    assert!(looped.is_playing(), "a loop keeps playing");
    assert_eq!(engine.active_voices(), 1);

    // Past the fade only the loop sounds: mono 0.25 centered = 0.25·cos(π/4).
    let after = (fade_end * RATE as f64) as usize + QUANTUM;
    let centered = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
    for f in after..frames {
        assert!((out[2 * f] - centered).abs() < 1e-5, "frame {f}: {}", out[2 * f]);
    }
}

/// A gain ramp is a linear `linearRampToValueAtTime` from the current value,
/// reaching the target on time; a ramp issued mid-ramp anchors on the value
/// at that instant (not the param's lagging `.value`).
#[wasm_bindgen_test]
async fn ramps_are_scheduled_linearly_from_the_current_value() {
    let frames = QUANTUM * 100;
    let (engine, web) = offline_web(frames);
    let dc = mono(RATE, vec![1.0; QUANTUM]);
    // Looping DC, all left: L = gain.
    let v = engine.play(&dc, VoiceParams::new().pan(-1.0).looping(true));
    let ramp_frames = 4_800; // 0.1 s
    v.set_gain(0.0, 0.1);
    let mid = QUANTUM * 20; // 2560: 53.3 % into the ramp
    let v2 = v.clone();
    let out = render_web(&web, vec![(mid, Box::new(move || v2.set_gain(1.0, 0.02)))]).await;

    for f in (0..mid).step_by(97) {
        let want = 1.0 - f as f32 / ramp_frames as f32;
        assert!((out[2 * f] - want).abs() < 1e-5, "frame {f}: {} vs {want}", out[2 * f]);
    }
    let anchor = 1.0 - mid as f32 / ramp_frames as f32;
    assert!((out[2 * mid] - anchor).abs() < 1e-5, "re-anchored at {}: {}", anchor, out[2 * mid]);
    let back = mid + 960; // 20 ms later: at the new target, held
    for f in back..frames {
        assert!((out[2 * f] - 1.0).abs() < 1e-5, "frame {f}: {}", out[2 * f]);
    }
    // The param itself settled on the target.
    let node = web.inner.state.borrow().voices[0].gain.clone();
    // SAFETY: live gain node.
    let gain = unsafe { js_param_value(node.raw(), P_GAIN) };
    assert!((gain - 1.0).abs() < 1e-6);
}

/// The 128-voice cap: the oldest non-looping buffer voice is stolen; loops
/// are never stolen, and with nothing stealable the new voice doesn't start.
#[wasm_bindgen_test]
fn voice_stealing_matches_the_mixer_rule() {
    let (engine, _web) = offline_web(QUANTUM);
    let buf = mono(RATE, vec![0.1; RATE as usize]);

    let first_loop = engine.play(&buf, VoiceParams::new().looping(true));
    let voices: Vec<_> = (1..MAX_VOICES).map(|_| engine.play(&buf, VoiceParams::new())).collect();
    assert_eq!(engine.active_voices(), MAX_VOICES);

    let extra = engine.play(&buf, VoiceParams::new());
    assert!(extra.is_playing());
    assert!(first_loop.is_playing(), "a loop is never stolen");
    assert!(!voices[0].is_playing(), "the oldest one-shot was stolen");
    assert!(voices[1].is_playing());
    assert_eq!(engine.active_voices(), MAX_VOICES);

    let (engine, _web) = offline_web(QUANTUM);
    let loops: Vec<_> = (0..MAX_VOICES)
        .map(|_| engine.play(&buf, VoiceParams::new().looping(true)))
        .collect();
    let rejected = engine.play(&buf, VoiceParams::new());
    assert!(!rejected.is_playing(), "nothing stealable: the new voice doesn't start");
    assert!(loops.iter().all(|v| v.is_playing()));
    // Un-looping makes a voice stealable again.
    loops[5].set_looping(false);
    let admitted = engine.play(&buf, VoiceParams::new());
    assert!(admitted.is_playing());
    assert!(!loops[5].is_playing());
    assert_eq!(engine.active_voices(), MAX_VOICES);
}

/// A `SoundBuffer` is uploaded into one `AudioBuffer` per context, reused by
/// every replay (and clone); the entry goes once the `SoundBuffer` is gone.
#[wasm_bindgen_test]
fn audio_buffers_are_cached_per_sound_buffer() {
    let (engine, web) = offline_web(QUANTUM);
    let a = mono(RATE, vec![0.1; 1_000]);
    let b = mono(RATE, vec![0.2; 1_000]);
    let cached = || web.inner.state.borrow().buffers.len();

    engine.play(&a, VoiceParams::new());
    let first = web.inner.state.borrow().buffers[0].1.clone();
    for _ in 0..20 {
        engine.play(&a, VoiceParams::new());
    }
    engine.play(&a.clone(), VoiceParams::new());
    assert_eq!(cached(), 1, "replays and clones reuse the upload");
    assert!(web.inner.state.borrow().buffers[0].1.strict_eq(&first), "the same AudioBuffer");

    engine.play(&b, VoiceParams::new());
    assert_eq!(cached(), 2);
    drop(a);
    engine.play(&b, VoiceParams::new());
    assert_eq!(cached(), 1, "a dropped SoundBuffer's upload is released");
}

/// Offsets follow the mixer: past the end a one-shot doesn't start; a loop
/// wraps.
#[wasm_bindgen_test]
fn start_offset_rules() {
    let (engine, _web) = offline_web(QUANTUM);
    let buf = mono(RATE, vec![0.1; 4_800]); // 0.1 s
    assert!(!engine.play(&buf, VoiceParams::new().start_offset(0.2)).is_playing());
    assert!(engine.play(&buf, VoiceParams::new().start_offset(0.2).looping(true)).is_playing());
    let empty = mono(RATE, Vec::new());
    assert!(!engine.play(&empty, VoiceParams::new()).is_playing());
}

/// A real `AudioContext`: `start()` opens it (possibly suspended — no user
/// gesture in an automated browser), reports its rate and latency; a live
/// stream voice plays the stream's `native_source` and `stop(0)` frees it.
#[wasm_bindgen_test]
async fn real_context_and_stream_voice() {
    let engine = AudioEngine::start().await.expect("AudioContext");
    assert!(engine.sample_rate() >= 8_000);
    if let Some(l) = engine.output_latency() {
        assert!(l.as_secs_f64() < 1.0, "latency {l:?}");
    }
    let (stream, writer) = AudioStream::new();
    writer.write_pcm_f32(48_000, 1, &[0.0; 480]);
    let v = engine.play_stream(&stream, VoiceParams::new().gain(0.5).lowpass(3_000.0));
    assert!(v.is_playing(), "a stream with an audio track plays");
    assert_eq!(engine.active_voices(), 1);
    v.set_pan(0.5, 0.1);
    v.set_lowpass(None, 0.1);
    v.set_rate(2.0, 0.0); // ignored for streams
    v.stop(0.0);
    assert!(!v.is_playing());
    assert_eq!(engine.active_voices(), 0);
    engine.suspend();
    engine.resume();
}
