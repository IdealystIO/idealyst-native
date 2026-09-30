//! Web (wasm32) file-decode backend — a hidden `<video>` + offscreen `<canvas>`
//! frame pump and a WebAudio PCM tap.
//!
//! The design mirrors the two outputs the SDK produces, the same split as the
//! Apple backend:
//!
//! - **Video frames** — a hidden `HtmlVideoElement` drives decode + the clock.
//!   Each animation frame (a [`runtime_shared::scheduling::raf_loop`], the same
//!   pump the Apple backend uses) we `drawImage` the video's *current* frame
//!   into a reused offscreen `HtmlCanvasElement`, sized to the (optionally
//!   `max_dimension`-downscaled, aspect-preserving) target, then `getImageData`
//!   hands us straight (non-premultiplied) tightly-packed `RGBA8` — exactly the
//!   SDK's frame format — which we push through the [`FrameWriter`]. This is the
//!   same `<video>`+`<canvas>` readback the `camera` web backend uses for a live
//!   feed; here the source is a clip URL instead of a `getUserMedia` stream.
//!   The element is appended to the document offscreen + invisible (not removed,
//!   so the browser keeps decoding it) rather than shown in an overlay — its
//!   pixels go to the canvas scene, not a player view.
//! - **Audio PCM** — an `AudioContext` `createMediaElementSource(video)` routes
//!   the element's audio through a `ScriptProcessorNode` whose `onaudioprocess`
//!   interleaves the input channels into one `f32` buffer and pushes it through
//!   the [`AudioWriter`] for the recorder's mux. Routing audio through WebAudio
//!   *replaces* the element's normal output, so the processor is connected on to
//!   the context destination — otherwise playback would go silent.
//!
//! Web is single-threaded; [`FrameWriter`] / [`AudioWriter`] are `!Send` on
//! wasm (the crate handles that). The rAF loop handle, the WebAudio nodes, the
//! `onaudioprocess` [`Closure`], and the `<video>` element all live in the
//! [`StreamHandle`] so nothing is dropped early; its `Drop` pauses the video,
//! disconnects the nodes, removes the element, and stops the pump.
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3). The element and canvas are created by this SDK, never mounted by
//! the backend. Pixel and PCM readback write straight into buffers Rust
//! allocated first (sized from values Rust already knows), so each is one
//! crossing and no JS view of wasm memory outlives a call into wasm.

use std::cell::Cell;
use std::rc::Rc;

use web_glue::{string, Closure, JsError, JsValue};

use media_stream::{AudioWriter, FrameWriter};

use crate::{DecodeConfig, DecodeSource, Opened, TransportControl, VideoDecodeError};

/// ScriptProcessor buffer size — 4096 frames is the common, low-overhead choice
/// for a non-latency-critical recording tap.
const SCRIPT_PROCESSOR_BUFFER: u32 = 4096;

/// Whether to route the clip's audio through WebAudio to tap PCM for the
/// recording mux. OFF for now: `createMediaElementSource` + a suspended
/// `AudioContext` stalls `<video>` playback. With it off the clip plays natively
/// (audible); only capturing that audio into a recording is deferred. Mirrors
/// the macOS `apple.rs` gate.
const ENABLE_AUDIO_TAP: bool = false;

/// The inline style that keeps the hidden `<video>` decoding. Offscreen but
/// with a real (tiny) size and NOT `visibility:hidden` / zero-size: browsers
/// throttle or refuse playback of hidden / 0×0 / display:none media, which
/// freezes `currentTime` (play() appears dead). `opacity:0` + a 2px box pinned
/// offscreen keeps it decoding AND advancing while invisible.
const HIDDEN_VIDEO_STYLE: &str =
    "position:fixed;left:0;top:0;width:2px;height:2px;opacity:0;pointer-events:none;z-index:-1;";

web_glue::import! {
    // `URL.createObjectURL(new Blob([bytes], { type }))` into `out`; the
    // bytes are copied (`slice`).
    #[catch]
    fn js_blob_url(p: usize, l: usize, tp: usize, tl: usize, out: usize) =
        "(p, l, tp, tl, o) => { \
           const bytes = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           G.retStr(URL.createObjectURL(new Blob([bytes], { type: G.str(tp, tl) })), o); }";
    fn js_revoke_url(p: usize, l: usize) = "(p, l) => { URL.revokeObjectURL(G.str(p, l)); }";
    // 1 with a window + document, 0 otherwise.
    fn js_has_document() -> u32 =
        "() => typeof window !== 'undefined' && window.document != null ? 1 : 0";
    // The hidden `<video>`: attributes first, then `src` (which starts the
    // load, so `muted` / `loop` are honored from frame zero), then appended
    // to <body> so the browser keeps decoding it.
    #[catch]
    fn js_create_video(up: usize, ul: usize, sp: usize, sl: usize, muted: u32, looping: u32) -> u32 =
        "(up, ul, sp, sl, muted, looping) => { const v = document.createElement('video'); \
           v.muted = muted !== 0; v.loop = looping !== 0; v.crossOrigin = 'anonymous'; \
           v.preload = 'auto'; v.setAttribute('playsinline', ''); \
           v.setAttribute('style', G.str(sp, sl)); v.src = G.str(up, ul); \
           if (document.body) document.body.appendChild(v); return G.add(v); }";
    // `play()`; its Promise may reject (autoplay without a gesture), which
    // is acceptable — the caller re-plays from a gesture — so it's observed
    // here rather than left as an unhandled rejection.
    fn js_play(v: u32) = "(v) => { const p = G.get(v).play(); if (p) p.catch(() => {}); }";
    fn js_pause(v: u32) = "(v) => { G.get(v).pause(); }";
    fn js_set_current_time(v: u32, t: f64) = "(v, t) => { G.get(v).currentTime = t; }";
    // 1 if `fastSeek` exists and was issued, 0 when unsupported (Chrome).
    #[catch]
    fn js_fast_seek(v: u32, t: f64) -> u32 =
        "(v, t) => { const e = G.get(v); if (typeof e.fastSeek !== 'function') return 0; \
           e.fastSeek(t); return 1; }";
    fn js_current_time(v: u32) -> f64 = "(v) => G.get(v).currentTime";
    fn js_duration(v: u32) -> f64 = "(v) => G.get(v).duration";
    fn js_paused(v: u32) -> u32 = "(v) => G.get(v).paused ? 1 : 0";
    fn js_set_muted(v: u32, m: u32) = "(v, m) => { G.get(v).muted = m !== 0; }";
    fn js_set_rate(v: u32, r: f64) = "(v, r) => { G.get(v).playbackRate = r; }";
    fn js_ready_state(v: u32) -> u32 = "(v) => G.get(v).readyState";
    fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth";
    fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight";
    // `onseeked = f` (0 clears it).
    fn js_set_onseeked(v: u32, f: u32) =
        "(v, f) => { G.get(v).onseeked = f === 0 ? null : G.get(f); }";
    // Teardown: pause, detach the seek handler, drop the source, remove the
    // element from the DOM.
    fn js_teardown_video(v: u32) =
        "(v) => { const e = G.get(v); e.pause(); e.onseeked = null; e.src = ''; \
           if (e.parentNode) e.parentNode.removeChild(e); }";
    // An offscreen canvas's 2D context. `willReadFrequently` keeps the
    // backing store CPU-side: every frame is read back with `getImageData`,
    // so this avoids a per-readback GPU→CPU stall (and the browser's
    // "Multiple readback operations" warning).
    #[catch]
    fn js_create_ctx() -> u32 =
        "() => { const c = document.createElement('canvas'); \
           const x = c.getContext('2d', { willReadFrequently: true }); return x == null ? 0 : G.add(x); }";
    // Draw the video's current frame scaled to w×h and copy the straight
    // (non-premultiplied) RGBA8 `ImageData` into the w*h*4 bytes at `out`.
    // 1 on success, 0 if the draw / readback threw (e.g. a tainted canvas).
    fn js_pump(x: u32, v: u32, w: u32, h: u32, out: usize) -> u32 =
        "(x, v, w, h, o) => { const ctx = G.get(x); const c = ctx.canvas; \
           if (c.width !== w) c.width = w; if (c.height !== h) c.height = h; \
           try { ctx.drawImage(G.get(v), 0, 0, w, h); \
             G.u8().set(ctx.getImageData(0, 0, w, h).data, o >>> 0); return 1; } \
           catch (_) { return 0; } }";
    // The PCM tap graph `MediaElementSource → ScriptProcessor → destination`
    // as `{ context, source, processor }`, or 0 if any node fails to build.
    fn js_audio_tap(v: u32, buf: u32, f: u32) -> u32 =
        "(v, buf, f) => { try { const context = new AudioContext(); \
           const source = context.createMediaElementSource(G.get(v)); \
           const processor = context.createScriptProcessor(buf, 2, 2); \
           processor.onaudioprocess = G.get(f); \
           source.connect(processor); processor.connect(context.destination); \
           return G.add({ context, source, processor }); } catch (_) { return 0; } }";
    fn js_audio_rate(t: u32) -> f64 = "(t) => G.get(t).context.sampleRate";
    fn js_audio_teardown(t: u32) =
        "(t) => { const g = G.get(t); g.processor.onaudioprocess = null; \
           try { g.processor.disconnect(); } catch (_) {} \
           try { g.source.disconnect(); } catch (_) {} g.context.close().catch(() => {}); }";
    // An `AudioProcessingEvent`'s input buffer shape: channels * 2^20 +
    // frames (frames < 2^20, channels ≤ 32 per the Web Audio spec), 0 when
    // unreadable.
    fn js_audio_shape(e: u32) -> f64 =
        "(e) => { const b = G.get(e).inputBuffer; return b == null ? 0 : \
           b.numberOfChannels * 1048576 + b.length; }";
    // Interleave the input channels ([L0,R0,L1,R1,…]) into the
    // channels*frames f32s at `out` (4-byte aligned).
    fn js_audio_interleave(e: u32, out: usize, channels: u32, frames: u32) =
        "(e, o, ch, n) => { const b = G.get(e).inputBuffer; \
           const f = new Float32Array(G.u8().buffer, o >>> 0, ch * n); \
           for (let c = 0; c < ch; c++) { const d = b.getChannelData(c); \
             for (let i = 0; i < n; i++) f[i * ch + c] = d[i]; } }";
}

fn revoke(url: &str) {
    let (p, l) = string::abi(url);
    unsafe { js_revoke_url(p, l) }
}

/// The hidden `<video>` handle, with the element reads the pump and the
/// transport need.
#[derive(Clone)]
struct Video(JsValue);

impl Video {
    fn play(&self) {
        unsafe { js_play(self.0.raw()) }
    }
    fn pause(&self) {
        unsafe { js_pause(self.0.raw()) }
    }
    fn current_time(&self) -> f64 {
        unsafe { js_current_time(self.0.raw()) }
    }
    fn set_current_time(&self, t: f64) {
        unsafe { js_set_current_time(self.0.raw(), t) }
    }
    /// `fastSeek(t)`; `false` when unsupported (or it threw).
    fn fast_seek(&self, t: f64) -> bool {
        matches!(unsafe { js_fast_seek(self.0.raw(), t) }, Ok(1))
    }
    fn paused(&self) -> bool {
        unsafe { js_paused(self.0.raw()) != 0 }
    }
    fn ready_state(&self) -> u32 {
        unsafe { js_ready_state(self.0.raw()) }
    }
    fn video_width(&self) -> u32 {
        unsafe { js_video_width(self.0.raw()) }
    }
    fn video_height(&self) -> u32 {
        unsafe { js_video_height(self.0.raw()) }
    }
}

// ===========================================================================
// Transport — drives the <video> element.
// ===========================================================================

/// Per-platform playback control over the hidden `<video>`. The `muted` cell
/// shadows the element so [`is_muted`](TransportControl::is_muted) is a cheap
/// read (the element's muted state would otherwise need a JS round-trip and is
/// the player's concern, distinct from the recorder's PCM tap).
struct WebTransport {
    video: Video,
    muted: Cell<bool>,
    /// Latest-wins, one-in-flight scrub coordination (shared with the `seeked`
    /// handler).
    seek_state: Rc<SeekState>,
}

/// Scrub coordination: a fast drag records `target` every tick (cheap, keeps the
/// slider responsive), but only ONE `set_current_time` is outstanding at a time.
/// When it completes (`seeked`), the newest pending target — if any — is issued
/// and the intermediate ones are dropped, so decodes never backlog on a large
/// clip and the picture catches up as fast as it can.
struct SeekState {
    /// `(seconds, exact)` — `exact=false` is a live-scrub preview (use `fastSeek`
    /// where available); `exact=true` decodes the precise frame (drag landing).
    target: Cell<Option<(f64, bool)>>,
    seeking: Cell<bool>,
    /// Latched once `fastSeek` is found unsupported (Chrome) — fall back to exact
    /// `currentTime` thereafter.
    no_fast_seek: Cell<bool>,
}

/// Issue the pending target iff no seek is in flight (latest-wins). Skips a
/// target that's already ~current (it would produce no `seeked` and wedge the
/// in-flight flag). A non-exact target prefers `fastSeek` (fast, approximate) for
/// smooth scrubbing; exact targets — and any browser without `fastSeek` — decode
/// the precise frame via `currentTime`.
fn pump_seek(video: &Video, state: &SeekState) {
    if state.seeking.get() {
        return;
    }
    let Some((t, exact)) = state.target.take() else { return };
    if (t - video.current_time()).abs() < 0.01 {
        return;
    }
    state.seeking.set(true);
    if exact || state.no_fast_seek.get() {
        video.set_current_time(t);
    } else if !video.fast_seek(t) {
        // fastSeek unsupported here (e.g. Chrome) — latch + use exact from now on.
        state.no_fast_seek.set(true);
        video.set_current_time(t);
    }
}

impl TransportControl for WebTransport {
    fn play(&self) {
        // play() returns a Promise we don't await; browsers may reject autoplay
        // without a user gesture, but our calls originate from a click.
        self.video.play();
    }
    fn pause(&self) {
        self.video.pause();
    }
    fn seek(&self, seconds: f32) {
        // Exact landing (drag end): decode the precise frame.
        self.seek_state.target.set(Some((seconds.max(0.0) as f64, true)));
        pump_seek(&self.video, &self.seek_state);
    }
    fn seek_preview(&self, seconds: f32) {
        // Live scrub: record the target (cheap) + issue only if nothing's in
        // flight; the `seeked` handler issues the newest pending target when the
        // current lands. Prefers `fastSeek` so frames flow while dragging.
        self.seek_state.target.set(Some((seconds.max(0.0) as f64, false)));
        pump_seek(&self.video, &self.seek_state);
    }
    fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
        unsafe { js_set_muted(self.video.0.raw(), muted as u32) }
    }
    fn set_rate(&self, rate: f32) {
        unsafe { js_set_rate(self.video.0.raw(), rate.max(0.0) as f64) }
    }
    fn position(&self) -> f32 {
        let t = self.video.current_time();
        if t.is_finite() {
            t as f32
        } else {
            0.0
        }
    }
    fn duration(&self) -> f32 {
        // `duration` is NaN before metadata loads and +inf for live/unknown.
        let d = unsafe { js_duration(self.video.0.raw()) };
        if d.is_finite() {
            d as f32
        } else {
            0.0
        }
    }
    fn is_playing(&self) -> bool {
        !self.video.paused()
    }
    fn is_muted(&self) -> bool {
        self.muted.get()
    }
}

// ===========================================================================
// StreamHandle — keeps decode alive; Drop stops it.
// ===========================================================================

/// Holds everything decode needs alive. Dropping it pauses the video,
/// disconnects + tears down the WebAudio graph, removes the element from the
/// DOM, and stops the rAF pump (the [`RafLoop`](runtime_shared::scheduling::RafLoop)
/// cancels on its own drop).
struct StreamHandle {
    video: Video,
    _raf: runtime_shared::scheduling::RafLoop,
    /// WebAudio tap, present only if the `AudioContext` built successfully. The
    /// `onaudioprocess` `Closure` is held here so it isn't dropped while the node
    /// still references it.
    audio: Option<AudioTap>,
    /// A `Blob` object URL created for a `Bytes` source; revoked on drop so the
    /// in-memory clip is freed.
    object_url: Option<String>,
    /// The `seeked` event handler, held so it stays valid while the element lives.
    _onseeked: Closure,
}

/// The WebAudio PCM-tap graph: `MediaElementSource → ScriptProcessor →
/// destination`, retained for the tap's lifetime.
struct AudioTap {
    graph: JsValue,
    /// Kept alive so the node's `onaudioprocess` callback stays valid.
    _on_process: Closure,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Pause, detach the seek handler (so it can't fire mid-drop), drop the
        // source and remove the offscreen element from the DOM.
        unsafe { js_teardown_video(self.video.0.raw()) };
        if let Some(audio) = self.audio.take() {
            // Detach the callback first so it can't fire mid-teardown, then
            // disconnect the graph and close the context.
            unsafe { js_audio_teardown(audio.graph.raw()) };
        }
        // Free the in-memory clip blob, if any.
        if let Some(u) = &self.object_url {
            revoke(u);
        }
        // `_raf` cancels the pump on its own drop.
    }
}

// ===========================================================================
// Open.
// ===========================================================================

pub(crate) async fn open(
    source: DecodeSource,
    config: DecodeConfig,
    frames: FrameWriter,
    audio: AudioWriter,
) -> Result<Opened, VideoDecodeError> {
    // Resolve to a URL the <video> can load. `Bytes` (the web file-picker hands
    // back a `Blob` with no path) becomes an in-memory `Blob` object URL, revoked
    // on teardown.
    let (url, object_url) = match source {
        DecodeSource::Url(u) => (u, None),
        DecodeSource::Bytes(data) => {
            let (tp, tl) = string::abi("video/mp4");
            let mut res = Ok(());
            let obj = string::receive(|o| {
                res = unsafe { js_blob_url(data.as_ptr() as usize, data.len(), tp, tl, o) }
            });
            res.map_err(|e| VideoDecodeError::Backend(format!("object url: {}", err_string(&e))))?;
            (obj.clone(), Some(obj))
        }
    };

    if unsafe { js_has_document() } == 0 {
        return Err(VideoDecodeError::Unsupported);
    }

    // Hidden <video> that drives decode + the clock.
    let (up, ul) = string::abi(&url);
    let (sp, sl) = string::abi(HIDDEN_VIDEO_STYLE);
    let video = unsafe {
        js_create_video(up, ul, sp, sl, config.muted as u32, config.loop_playback as u32)
    }
    // SAFETY: a fresh `G.add` slot the snippet minted for us.
    .map(|h| Video(unsafe { JsValue::from_raw(h) }))
    .map_err(|e| VideoDecodeError::Backend(format!("create video: {}", err_string(&e))))?;

    // Autoplay (browsers require muted for unprompted autoplay; if the caller
    // asked for autoplay && !muted we still attempt play() — it may be rejected,
    // which is acceptable: the caller can re-`play()` from a user gesture).
    if config.autoplay {
        video.play();
    }

    // Offscreen canvas + 2d context, reused across pump ticks.
    let ctx = match unsafe { js_create_ctx() } {
        Ok(0) => return Err(VideoDecodeError::Backend("no 2d context".into())),
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        Ok(h) => unsafe { JsValue::from_raw(h) },
        Err(e) => {
            return Err(VideoDecodeError::Backend(format!("get 2d context: {}", err_string(&e))))
        }
    };

    // Set by the `<video>`'s `seeked` event: a seek's decoded frame just became
    // available, so the pump must redraw even though currentTime is now steady at
    // the target. Without this, scrubbing a PAUSED video sets currentTime but the
    // landed frame never repaints (the picture lags / "hangs" then jumps).
    let redraw = Rc::new(Cell::new(false));
    let seek_state = Rc::new(SeekState {
        target: Cell::new(None),
        seeking: Cell::new(false),
        no_fast_seek: Cell::new(false),
    });
    let onseeked = {
        let redraw = redraw.clone();
        let video_cb = video.clone();
        let seek_state_cb = seek_state.clone();
        // A seek completed: draw the landed frame, mark no seek in flight, and
        // issue the newest pending target (if the user kept dragging) — dropping
        // the intermediate ones. This is what keeps scrubbing backlog-free.
        let cb = Closure::new(move |_| {
            seek_state_cb.seeking.set(false);
            redraw.set(true);
            pump_seek(&video_cb, &seek_state_cb);
        });
        unsafe { js_set_onseeked(video.0.raw(), cb.as_js().raw()) };
        cb
    };

    // Frame pump: each display tick, draw + read back the current frame as RGBA8.
    let raf = {
        let video = video.clone();
        let max_dim = config.max_dimension;
        let redraw = redraw.clone();
        let mut last_t = -1.0_f64;
        let mut drew_once = false;
        let mut buf = Vec::new();
        runtime_shared::scheduling::raf_loop(move || {
            // Only push a frame when there's actually a NEW one: while playing
            // (currentTime advances) or right after a seek. A paused, unchanged
            // frame is pushed once then skipped — so a paused video stops driving
            // repaints (the "rerenders constantly when paused" bug). The readback
            // is also a GPU→CPU stall, so gate it on a real consumer too.
            if !frames.wants_cpu_frames() {
                return;
            }
            let ready = video.ready_state() >= 2 && video.video_width() > 0;
            if !ready {
                return;
            }
            let t = video.current_time();
            let advancing = !video.paused();
            let changed = (t - last_t).abs() > 1e-4;
            // `redraw` (a completed seek) forces one draw even when t is steady.
            let seeked = redraw.replace(false);
            if advancing || changed || seeked || !drew_once {
                pump_frame(&video, &ctx, &frames, max_dim, &mut buf);
                last_t = t;
                drew_once = true;
            }
        })
    };

    // Audio tap → PCM for the recorder. GATED OFF (see `ENABLE_AUDIO_TAP`):
    // `createMediaElementSource` reroutes the element's audio into an
    // `AudioContext` that starts suspended without a user-gesture resume, which
    // stalls `<video>` playback (so play/scrub appear dead). With it off the
    // element plays natively (its own sound); only capturing that audio INTO a
    // recording is deferred — mirrors the macOS `ENABLE_AUDIO_TAP` gate.
    let audio_tap = if ENABLE_AUDIO_TAP {
        install_audio_tap(&video, audio)
    } else {
        let _ = audio; // unused writer dropped → no audio stream advertised
        None
    };

    let control: Rc<dyn TransportControl> = Rc::new(WebTransport {
        video: video.clone(),
        muted: Cell::new(config.muted),
        seek_state: seek_state.clone(),
    });

    let has_audio = audio_tap.is_some();
    let handle = StreamHandle {
        video,
        _raf: raf,
        audio: audio_tap,
        object_url,
        _onseeked: onseeked,
    };

    Ok(Opened {
        handle: Box::new(handle),
        control,
        has_audio,
        // videoWidth isn't known until metadata loads; report None at open.
        natural_size: None,
    })
}

/// Draw the video's current frame into the canvas (downscaled per `max_dim`)
/// and push it back as tightly-packed `RGBA8`. A no-op until the video has
/// decoded a frame (`readyState >= HAVE_CURRENT_DATA` and non-zero dimensions).
/// `buf` is reused across frames.
fn pump_frame(
    video: &Video,
    ctx: &JsValue,
    frames: &FrameWriter,
    max_dim: Option<u32>,
    buf: &mut Vec<u8>,
) {
    // `HAVE_CURRENT_DATA` == 2: there's a frame for the current playback position.
    if video.ready_state() < 2 {
        return;
    }
    let nat_w = video.video_width();
    let nat_h = video.video_height();
    if nat_w == 0 || nat_h == 0 {
        return;
    }
    let (w, h) = target_size(nat_w, nat_h, max_dim);
    // Sized BEFORE the crossing: the snippet writes into it without calling
    // back into wasm, so its memory view can't be detached mid-copy.
    buf.resize(w as usize * h as usize * 4, 0);
    if unsafe { js_pump(ctx.raw(), video.0.raw(), w, h, buf.as_mut_ptr() as usize) } == 0 {
        return;
    }
    frames.write_rgba8(w, h, buf);
}

/// Target decode size honoring `max_dim` (aspect-preserving). `(0,0)` natural
/// size never reaches here (the pump bails earlier).
fn target_size(nat_w: u32, nat_h: u32, max_dim: Option<u32>) -> (u32, u32) {
    match max_dim {
        Some(max) if nat_w.max(nat_h) > max && max > 0 => {
            let scale = max as f32 / nat_w.max(nat_h) as f32;
            (
                ((nat_w as f32 * scale) as u32).max(1),
                ((nat_h as f32 * scale) as u32).max(1),
            )
        }
        _ => (nat_w, nat_h),
    }
}

/// Build the `MediaElementSource → ScriptProcessor → destination` PCM tap.
///
/// `createMediaElementSource` *routes* the element's audio through WebAudio, so
/// the processor MUST connect on to the destination or playback goes silent. We
/// set `has_audio` optimistically: if the clip has no audio track the processor
/// just receives silence, which is acceptable (we can't cheaply pre-check tracks
/// on the web). Returns `None` only if the WebAudio graph itself fails to build.
fn install_audio_tap(video: &Video, writer: AudioWriter) -> Option<AudioTap> {
    let rate = Rc::new(Cell::new(0u32));
    let rate_cb = rate.clone();
    // Interleave each input channel into one frame-major f32 buffer and push it.
    let on_process = Closure::new(move |event: JsValue| {
        let shape = unsafe { js_audio_shape(event.raw()) } as u64;
        let channels = (shape >> 20) as usize;
        let frames = (shape & 0xF_FFFF) as usize;
        if channels == 0 || frames == 0 {
            return;
        }
        let mut interleaved = vec![0.0f32; frames * channels];
        unsafe {
            js_audio_interleave(
                event.raw(),
                interleaved.as_mut_ptr() as usize,
                channels as u32,
                frames as u32,
            )
        };
        writer.write_pcm_f32(rate_cb.get(), channels as u16, &interleaved);
    });
    let graph = match unsafe {
        js_audio_tap(video.0.raw(), SCRIPT_PROCESSOR_BUFFER, on_process.as_js().raw())
    } {
        0 => return None,
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        h => unsafe { JsValue::from_raw(h) },
    };
    rate.set(unsafe { js_audio_rate(graph.raw()) } as u32);
    Some(AudioTap {
        graph,
        _on_process: on_process,
    })
}

/// Best-effort string from a JS error (its `.message`, or `String(value)`).
fn err_string(e: &JsError) -> String {
    let v = e.value();
    v.as_string()
        .or_else(|| v.get("message").ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| e.message())
}
