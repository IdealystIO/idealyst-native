//! The web decoder: WebCodecs `VideoDecoder`, drawn onto a canvas whose
//! `captureStream()` backs the [`MediaStream`].
//!
//! Each decoded frame is drawn as soon as it comes out: a live picture wants
//! the newest frame, not a smooth one. The decoder is made at the first
//! keyframe, from the codec string its SPS gives, and remade when a keyframe
//! names another (the sender restarted at another size) or after it has
//! closed itself on an error. The calls go through `web_glue::import!`, the
//! framework's JS boundary.

use std::rc::Rc;

use media_stream::FrameWriter;
use web_glue::{JsCast, JsValue};

use crate::{h264, Codec, EncodedFrame, MediaStream, VideoCodecError};

web_glue::import! {
    #[catch]
    fn js_open() -> u32 =
        "() => { \
           if (typeof VideoDecoder === 'undefined') throw new Error('this browser has no WebCodecs VideoDecoder'); \
           const c = document.createElement('canvas'); c.width = 16; c.height = 16; \
           const x = c.getContext('2d'); \
           const st = { c, x, dec: null, codec: '', done: false, w: 0, h: 0 }; \
           st.make = (codec) => { \
             st.codec = codec; \
             st.dec = new VideoDecoder({ \
               output: (f) => { \
                 if (st.done) { f.close(); return; } \
                 if (c.width !== f.displayWidth) c.width = f.displayWidth; \
                 if (c.height !== f.displayHeight) c.height = f.displayHeight; \
                 x.drawImage(f, 0, 0); st.w = f.displayWidth; st.h = f.displayHeight; f.close(); }, \
               error: (e) => console.error('[video-codec] decoder', e), \
             }); \
             st.dec.configure({ codec, optimizeForLatency: true }); }; \
           st.stream = c.captureStream(); \
           return G.add(st); }";
    fn js_stream(s: u32) -> u32 = "(s) => G.add(G.get(s).stream)";
    // 1 when the frame went to the decoder; 0 when it could not, and only a
    // keyframe can follow.
    fn js_decode(s: u32, cp: usize, cl: usize, p: usize, l: usize, key: u32, ts: f64) -> u32 =
        "(s, cp, cl, p, l, k, ts) => { const t = G.get(s); const codec = G.str(cp, cl); \
           try { \
             if (k && codec && (!t.dec || t.dec.state === 'closed' || t.codec !== codec)) { \
               if (t.dec && t.dec.state !== 'closed') t.dec.close(); \
               t.make(codec); } \
             if (!t.dec || t.dec.state === 'closed') return 0; \
             t.dec.decode(new EncodedVideoChunk({ type: k ? 'key' : 'delta', timestamp: ts, \
               data: G.u8().slice(p >>> 0, (p >>> 0) + l) })); \
             return 1; \
           } catch (e) { \
             console.error('[video-codec] decode', e); \
             try { if (t.dec && t.dec.state !== 'closed') t.dec.close(); } catch (_) {} \
             return 0; } }";
    fn js_size(s: u32) -> u32 = "(s) => { const t = G.get(s); return (((t.w & 0xffff) << 16) | (t.h & 0xffff)) >>> 0; }";
    fn js_close(s: u32) =
        "(s) => { const t = G.get(s); t.done = true; \
           try { if (t.dec && t.dec.state !== 'closed') t.dec.close(); } catch (_) {} \
           t.stream.getTracks().forEach((k) => k.stop()); }";
}

pub(crate) struct Decoder {
    handle: JsValue,
    stream: MediaStream,
    _writer: FrameWriter,
    /// Until a keyframe has gone in, a delta frame has nothing to build on.
    keyed: bool,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: a live handle from `js_open`.
        unsafe { js_close(self.handle.raw()) };
    }
}

impl Decoder {
    pub(crate) fn new(codec: Codec) -> Result<Decoder, VideoCodecError> {
        let Codec::H264 = codec;
        // SAFETY: fresh handles, owned by the `JsValue`s that wrap them.
        let handle = unsafe { js_open() }
            .map(|h| unsafe { JsValue::from_raw(h) })
            .map_err(|e| VideoCodecError::Platform(e.to_string()))?;
        let native =
            unsafe { JsValue::from_raw(js_stream(handle.raw())) }.unchecked_into::<web_glue::dom::MediaStream>();
        // No CPU frames: the canvas's stream is the whole of it.
        let (stream, writer) = MediaStream::new();
        stream.set_native_source(Rc::new(native));
        Ok(Decoder { handle, stream, _writer: writer, keyed: false })
    }

    pub(crate) fn decode(&mut self, frame: &EncodedFrame) -> bool {
        if !frame.key && !self.keyed {
            return false;
        }
        let codec = if frame.key { h264::codec_string(&frame.data).unwrap_or_default() } else { String::new() };
        // SAFETY: both slices outlive the synchronous call, which copies them.
        let ok = unsafe {
            js_decode(
                self.handle.raw(),
                codec.as_ptr() as usize,
                codec.len(),
                frame.data.as_ptr() as usize,
                frame.data.len(),
                frame.key as u32,
                frame.pts_us as f64,
            )
        } != 0;
        self.keyed = ok;
        ok
    }

    pub(crate) fn stream(&self) -> MediaStream {
        self.stream.clone()
    }

    pub(crate) fn size(&self) -> Option<(u32, u32)> {
        // SAFETY: a live handle from `js_open`.
        let packed = unsafe { js_size(self.handle.raw()) };
        let (w, h) = (packed >> 16, packed & 0xffff);
        (w > 0 && h > 0).then_some((w, h))
    }
}
