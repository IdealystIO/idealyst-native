//! Browser-native image decoding for the wgpu renderer on wasm32.
//!
//! On native the renderer decodes with the `image` crate (PNG / JPEG / GIF /
//! WebP) and rasterizes SVG with resvg (`backend-image-source`). In the
//! browser both already exist — the page has an image decoder for every
//! format `docs/image-src.md` promises and a vector renderer for SVG — so
//! linking Rust copies of them only grows the bundle. Measured on the fiddle
//! simulator bundle (`host-web` + `ios-sim`, release, `opt-level = "z"`,
//! LTO): resvg + the `image` decoders cost several hundred KB of wasm; see
//! the `render-wgpu` Cargo.toml for the numbers. The DOM web backend keeps
//! resvg out for the same reason (`backend-image-source`'s Cargo.toml).
//!
//! **Rasters** — `createImageBitmap(blob)` decodes off the main thread, the
//! bitmap is drawn once into an `OffscreenCanvas` at its natural size and
//! read back with `getImageData`, which yields exactly what
//! [`DecodedKind::Raster`](super::DecodedKind::Raster) holds: straight-alpha
//! sRGB RGBA8. The bitmap is closed immediately; only the Rust pixels stay.
//!
//! **SVG** — `createImageBitmap` can't decode SVG in Chrome, so an SVG goes
//! through an `HTMLImageElement` over a `blob:` URL (type
//! `image/svg+xml`, which the browser requires — it never sniffs SVG). The
//! decoded element is kept: [`BrowserSvg::rasterize`] draws it into a canvas
//! at whatever pixel size the renderer needs, synchronously, so the SVG is
//! re-rasterized at its displayed size × devicePixelRatio exactly like the
//! native resvg path ([`svg_raster_scale`](runtime_shared::image_src::svg_raster_scale)
//! picks the scale on both).
//!
//! The SVG's intrinsic size is resolved the way usvg (the native path)
//! resolves it — root `width`/`height` in absolute units, else the
//! `viewBox` size, a lone axis following the `viewBox` aspect — and written
//! back onto the root before the blob is built. That keeps `on_load` and an
//! unsized image's layout identical on web and native, and makes the
//! element's natural size equal the intrinsic size, so `drawImage` at a
//! proportional size fills the canvas exactly. (Left alone, Chrome gives a
//! `viewBox`-only SVG a 300×150 natural size.)
//!
//! **Errors** — a rejected `createImageBitmap` / `img.decode()`, an XML
//! parse error, or an SVG with no resolvable size rejects the decode
//! promise; the caller turns that into `on_error`, same as the native
//! decoder's `Err`.
//!
//! Bindings follow the framework's own web boundary (`web-glue`, see
//! `docs/proposals/own-web-bindings.md`): the JS lives here, no
//! wasm-bindgen / web-sys.

use web_glue::js::Uint8Array;
use web_glue::{JsCast, JsFuture, JsValue};

use super::{next_image_id, DecodedImage, DecodedKind};

web_glue::js_module!(fn decode_module = "render-wgpu/image-decode", r#"
// One 2D context per size request. OffscreenCanvas where available (every
// current engine); a detached <canvas> otherwise.
const ctx2d = (w, h) => {
  const c = typeof OffscreenCanvas === 'function'
    ? new OffscreenCanvas(w, h)
    : Object.assign(document.createElement('canvas'), { width: w, height: h });
  const ctx = c.getContext('2d', { willReadFrequently: true });
  if (!ctx) throw new Error('no 2d canvas context');
  return ctx;
};
// Straight-alpha RGBA8 of `src` drawn at w×h.
const pixels = (src, w, h) => {
  const ctx = ctx2d(w, h);
  ctx.drawImage(src, 0, 0, w, h);
  const d = ctx.getImageData(0, 0, w, h).data;
  return new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
};
// Absolute length of the root's `width`/`height`, or null when absent,
// a percentage, or not resolvable outside a rendered document.
const len = (root, name) => {
  if (!root.hasAttribute(name)) return null;
  try {
    const l = root[name].baseVal;
    if (l.unitType === SVGLength.SVG_LENGTHTYPE_PERCENTAGE) return null;
    return l.value > 0 ? l.value : null;
  } catch (_) {
    return null;
  }
};
const svgSize = (root) => {
  const vb = root.viewBox && root.viewBox.baseVal;
  const vw = vb && vb.width > 0 ? vb.width : null;
  const vh = vb && vb.height > 0 ? vb.height : null;
  let w = len(root, 'width');
  let h = len(root, 'height');
  if (w == null && h == null) { w = vw; h = vh; }
  else if (w == null) w = vw && vh ? h * vw / vh : null;
  else if (h == null) h = vw && vh ? w * vh / vw : null;
  return w > 0 && h > 0 ? [w, h] : null;
};
const decodeSvg = async (bytes) => {
  const text = new TextDecoder().decode(bytes);
  const doc = new DOMParser().parseFromString(text, 'image/svg+xml');
  const root = doc.documentElement;
  if (doc.getElementsByTagName('parsererror').length > 0 || root.localName !== 'svg') {
    throw new Error('invalid SVG document');
  }
  const size = svgSize(root);
  if (!size) throw new Error('SVG has no intrinsic size (no width/height or viewBox)');
  root.setAttribute('width', String(size[0]));
  root.setAttribute('height', String(size[1]));
  const blob = new Blob([new XMLSerializer().serializeToString(root)], { type: 'image/svg+xml' });
  const url = URL.createObjectURL(blob);
  const img = new Image();
  img.src = url;
  try {
    await img.decode();
  } catch (e) {
    URL.revokeObjectURL(url);
    throw e;
  }
  return { svg: true, w: size[0], h: size[1], img, url };
};
const decodeRaster = async (bytes) => {
  const bmp = await createImageBitmap(new Blob([bytes]));
  try {
    const w = bmp.width, h = bmp.height;
    if (!(w > 0 && h > 0)) throw new Error('image has no pixels');
    return { svg: false, w, h, px: pixels(bmp, w, h) };
  } finally {
    bmp.close();
  }
};
return {
  // `p/l` are image bytes in wasm memory, COPIED before returning (the
  // decode is asynchronous and wasm memory may grow meanwhile). Returns a
  // promise of `{ svg, w, h, px }` (raster) or `{ svg, w, h, img, url }`.
  decode(p, l, svg) {
    const bytes = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0));
    return svg ? decodeSvg(bytes) : decodeRaster(bytes);
  },
  rasterize(st, w, h) {
    return pixels(st.img, w, h);
  },
  release(st) {
    URL.revokeObjectURL(st.url);
    st.img = null;
  },
};
"#);

web_glue::import! {
    fn js_decode(p: usize, l: usize, svg: u32) -> u32 =
        "(p, l, svg) => G.add(G.m('render-wgpu/image-decode').decode(p, l, svg !== 0))";
    #[catch]
    fn js_rasterize(st: u32, w: u32, h: u32) -> u32 =
        "(st, w, h) => G.add(G.m('render-wgpu/image-decode').rasterize(G.get(st), w >>> 0, h >>> 0))";
    fn js_release(st: u32) = "(st) => { G.m('render-wgpu/image-decode').release(G.get(st)); }";
}

/// A decoded SVG held by the browser: an `HTMLImageElement` over a `blob:`
/// URL, rasterized on demand. Mirrors `backend_image_source::SvgImage`'s
/// surface so the renderer treats both the same.
pub struct BrowserSvg {
    state: JsValue,
    size: (f64, f64),
}

impl BrowserSvg {
    pub fn intrinsic_size(&self) -> (f64, f64) {
        self.size
    }

    /// Straight-alpha RGBA8 at `scale` pixels per intrinsic point. `None`
    /// if the browser refuses (e.g. a canvas over its size limit).
    pub fn rasterize(&self, scale: f64) -> Option<image::RgbaImage> {
        let (iw, ih) = self.size;
        let w = (iw * scale).round().max(1.0) as u32;
        let h = (ih * scale).round().max(1.0) as u32;
        decode_module();
        // SAFETY: a live state handle minted by `js_decode`'s promise.
        let px = match unsafe { js_rasterize(self.state.raw(), w, h) } {
            Ok(raw) => unsafe { JsValue::from_raw(raw) },
            Err(e) => {
                log::warn!("image(): SVG rasterize at {w}x{h} failed: {}", e.message());
                return None;
            }
        };
        let px = px.dyn_into::<Uint8Array>().ok()?.to_vec();
        image::RgbaImage::from_raw(w, h, px)
    }
}

impl Drop for BrowserSvg {
    fn drop(&mut self) {
        decode_module();
        // SAFETY: a live state handle; releasing twice is impossible (Drop).
        unsafe { js_release(self.state.raw()) }
    }
}

/// Decode `bytes` with the browser. SVG is recognized by sniffing
/// ([`runtime_shared::image_src::looks_like_svg`]), the same rule the
/// native decoder applies.
pub async fn decode(bytes: Vec<u8>) -> Result<DecodedImage, String> {
    let svg = runtime_shared::image_src::looks_like_svg(&bytes);
    decode_module(); // the anchor: keeps the module record linked
    // SAFETY: borrowed bytes for the duration of the call (the module
    // copies them before returning); the result is a fresh promise handle.
    let promise = unsafe { JsValue::from_raw(js_decode(bytes.as_ptr() as usize, bytes.len(), svg as u32)) };
    drop(bytes);
    let out = JsFuture::new(&promise).await.map_err(|e| e.message())?;
    let num = |key: &str| -> Result<f64, String> {
        out.get(key)
            .map_err(|e| e.message())?
            .as_f64()
            .ok_or_else(|| format!("decoder result has no {key}"))
    };
    let (w, h) = (num("w")?, num("h")?);
    let kind = if svg {
        DecodedKind::Svg(BrowserSvg { state: out, size: (w, h) })
    } else {
        let px = out
            .get("px")
            .map_err(|e| e.message())?
            .dyn_into::<Uint8Array>()
            .map_err(|_| "decoder returned no pixels".to_string())?
            .to_vec();
        DecodedKind::Raster(
            image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or("decoder pixel buffer size mismatch")?,
        )
    };
    Ok(DecodedImage { id: next_image_id(), kind })
}
