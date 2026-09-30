//! Web playback via `HTMLAudioElement`.
//!
//! `new Audio(src)` is the simplest cross-browser way to play a sound: it
//! handles decoding, buffering, and (for a URL/path) fetching, and exposes
//! exactly the controls this SDK needs — `play()` / `pause()`, `.volume`,
//! `.loop`. We pick it over the Web Audio graph
//! (`AudioContext`/`decodeAudioData`/`AudioBufferSourceNode`) deliberately:
//! the Web Audio path is lower-latency and gives sample-accurate scheduling,
//! but it needs an `AudioContext` (which browsers only let you `resume()`
//! after a user gesture) and a decode step, which is more machinery than a
//! "load and play a sound" SDK warrants. A future low-latency/spatial layer
//! can sit on Web Audio behind this same API.
//!
//! Concurrency: each [`Sound::play`](crate::Sound::play) clones the prepared
//! source's blob URL into a *new* `HTMLAudioElement`, so overlapping voices
//! from one `Sound` work — fine for layering short SFX.
//!
//! Every browser call is a web-glue binding declared here (own-web-bindings
//! phase 3).

use web_glue::{string, JsValue};

use crate::{AudioError, AudioSource};

web_glue::import! {
    // `URL.createObjectURL(new Blob([bytes], { type }))` into `out`. The
    // bytes are copied (`slice`) into the Blob, never viewed.
    #[catch]
    fn js_blob_url(p: usize, l: usize, tp: usize, tl: usize, out: usize) =
        "(p, l, tp, tl, o) => { \
           const bytes = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           G.retStr(URL.createObjectURL(new Blob([bytes], { type: G.str(tp, tl) })), o); }";
    fn js_revoke_url(p: usize, l: usize) = "(p, l) => { URL.revokeObjectURL(G.str(p, l)); }";
    // `new Audio(src)`.
    fn js_new_audio(p: usize, l: usize) -> u32 = "(p, l) => G.add(new Audio(G.str(p, l)))";
    // `play()`. Its Promise rejects under the autoplay policy (no user
    // gesture); that is expected and handled by the caller re-playing from a
    // press, so the rejection is observed here instead of surfacing as an
    // unhandled-rejection error in the console.
    fn js_play(e: u32) = "(e) => { const p = G.get(e).play(); if (p) p.catch(() => {}); }";
    fn js_pause(e: u32) = "(e) => { G.get(e).pause(); }";
    // Pause and detach the source so the element releases its decoder.
    fn js_release(e: u32) = "(e) => { const a = G.get(e); a.pause(); a.src = ''; }";
    fn js_set_volume(e: u32, v: f64) = "(e, v) => { G.get(e).volume = v; }";
    fn js_set_loop(e: u32, on: u32) = "(e, on) => { G.get(e).loop = on !== 0; }";
    fn js_is_playing(e: u32) -> u32 = "(e) => { const a = G.get(e); return !a.paused && !a.ended ? 1 : 0; }";
}

/// `URL.revokeObjectURL(url)`.
fn revoke(url: &str) {
    let (p, l) = string::abi(url);
    unsafe { js_revoke_url(p, l) }
}

/// A prepared sound: the resolved `src` URL to feed each new
/// `HTMLAudioElement`, plus (for `Bytes`) the object URL we created and must
/// revoke when the `Sound` drops to avoid leaking it.
pub(crate) struct PreparedSound {
    /// The `src` to set on each playback element.
    src: String,
    /// `Some` when `src` is an object URL we own (from a `Bytes` source);
    /// revoked on `Drop`. `None` for path/URL sources (the page owns those).
    object_url: Option<String>,
}

impl Drop for PreparedSound {
    fn drop(&mut self) {
        if let Some(url) = self.object_url.take() {
            // Best-effort: free the blob backing the object URL. New
            // playbacks already copied the string, but the browser keeps
            // the blob alive until every URL referencing it is revoked AND
            // no element is loading it; this releases our handle.
            revoke(&url);
        }
    }
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        // A fresh element per play() → independent, overlapping voices.
        let (p, l) = string::abi(&self.src);
        // SAFETY: a fresh `G.add` slot the snippet minted for us.
        let el = unsafe { JsValue::from_raw(js_new_audio(p, l)) };
        // Kick off playback. The returned promise can reject (autoplay
        // policy before a user gesture); the binding swallows it — when this
        // is called from a press handler (the supported path), the gesture
        // is present.
        unsafe { js_play(el.raw()) };
        PlaybackHandle { el }
    }
}

/// A running playback wrapping one `HTMLAudioElement`. `Drop` pauses it and
/// clears its `src` so the browser releases the decoder and any buffered
/// audio — RAII stop.
pub(crate) struct PlaybackHandle {
    el: JsValue,
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        // Pause and detach the source so the media element releases its
        // resources.
        unsafe { js_release(self.el.raw()) }
    }
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        unsafe { js_pause(self.el.raw()) }
    }

    pub(crate) fn resume(&self) {
        unsafe { js_play(self.el.raw()) }
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        // `HTMLMediaElement.volume` is in [0,1]; the public wrapper already
        // clamped.
        unsafe { js_set_volume(self.el.raw(), volume as f64) }
    }

    pub(crate) fn set_looping(&self, looping: bool) {
        unsafe { js_set_loop(self.el.raw(), looping as u32) }
    }

    pub(crate) fn is_playing(&self) -> bool {
        // Playing == not paused and not ended.
        unsafe { js_is_playing(self.el.raw()) != 0 }
    }
}

/// Prepare a sound. For `Bytes` we wrap the encoded data in a `Blob` and
/// mint an object URL; for `Path`/`Url` the string is used as `src`
/// directly (the browser fetches it).
pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    match source {
        AudioSource::Bytes(bytes) => {
            // A Blob from the encoded bytes with a generic audio MIME hint;
            // the browser still sniffs the actual container format.
            const MIME: &str = "audio/*";
            let (tp, tl) = string::abi(MIME);
            let mut res = Ok(());
            let url = string::receive(|o| {
                res = unsafe { js_blob_url(bytes.as_ptr() as usize, bytes.len(), tp, tl, o) }
            });
            res.map_err(|e| AudioError::Backend(format!("object URL failed: {e}")))?;
            Ok(PreparedSound {
                src: url.clone(),
                object_url: Some(url),
            })
        }
        AudioSource::Path(path) => Ok(PreparedSound {
            src: path.to_string_lossy().into_owned(),
            object_url: None,
        }),
        AudioSource::Url(url) => Ok(PreparedSound {
            src: url,
            object_url: None,
        }),
    }
}
