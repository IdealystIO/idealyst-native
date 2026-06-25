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

use wasm_bindgen::JsCast;
use web_sys::{Blob, BlobPropertyBag, HtmlAudioElement, Url};

use crate::{AudioError, AudioSource};

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
            let _ = Url::revoke_object_url(&url);
        }
    }
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        // A fresh element per play() → independent, overlapping voices.
        let el = HtmlAudioElement::new_with_src(&self.src)
            .ok()
            .unwrap_or_else(|| {
                // `new Audio()` is infallible in practice; if the URL form
                // is rejected, fall back to an empty element + set_src so we
                // still return a (silent) handle rather than panic.
                let el = HtmlAudioElement::new().expect("new Audio() must construct");
                el.set_src(&self.src);
                el
            });
        // Kick off playback. The returned promise can reject (autoplay
        // policy before a user gesture); we ignore it — when this is called
        // from a press handler (the supported path), the gesture is present.
        let _ = el.play();
        PlaybackHandle { el }
    }
}

/// A running playback wrapping one `HTMLAudioElement`. `Drop` pauses it and
/// clears its `src` so the browser releases the decoder and any buffered
/// audio — RAII stop.
pub(crate) struct PlaybackHandle {
    el: HtmlAudioElement,
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        self.el.pause().ok();
        // Detach the source so the media element releases its resources.
        self.el.set_src("");
    }
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        let _ = self.el.pause();
    }

    pub(crate) fn resume(&self) {
        let _ = self.el.play();
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        // `HTMLMediaElement.volume` is in [0,1]; the public wrapper already
        // clamped.
        self.el.set_volume(volume as f64);
    }

    pub(crate) fn set_looping(&self, looping: bool) {
        self.el.set_loop(looping);
    }

    pub(crate) fn is_playing(&self) -> bool {
        // Playing == not paused and not ended.
        !self.el.paused() && !self.el.ended()
    }
}

/// Prepare a sound. For `Bytes` we wrap the encoded data in a `Blob` and
/// mint an object URL; for `Path`/`Url` the string is used as `src`
/// directly (the browser fetches it).
pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    match source {
        AudioSource::Bytes(bytes) => {
            // A Blob from the encoded bytes; the browser sniffs the format,
            // so an explicit MIME type isn't required.
            let array = js_sys::Uint8Array::from(bytes.as_slice());
            let parts = js_sys::Array::new();
            parts.push(&array);
            // A generic audio MIME hint; the browser still sniffs the actual
            // container format from the bytes.
            let options = BlobPropertyBag::new();
            options.set_type("audio/*");
            let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
                .map_err(|e| AudioError::Backend(format!("blob construct failed: {e:?}")))?
                .unchecked_into::<Blob>();
            let url = Url::create_object_url_with_blob(&blob)
                .map_err(|e| AudioError::Backend(format!("object URL failed: {e:?}")))?;
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
