//! Decode a PDF [`Image`] into a canvas [`ImageSource`] + placement.
//!
//! Returns `(source, dst, eff_transform)` where `dst` is the decoded image's
//! own pixel rect and `eff_transform` maps that rect into device space. The
//! caller emits `Save · Transform(eff) · Image(dst) · Restore`, which reproduces
//! the reference renderer's `set_transform(transform · scale(scale_factors));
//! draw_image(decoded)` — the `scale_factors` correct for any decode-time
//! resolution/metadata mismatch.

use canvas_core::{ImageSource, Rect};
use hayro_interpret::{CacheKey, Image, ImageData, LumaData, Paint as PdfPaint};
use kurbo::Affine;

/// Cap on a decoded image's larger dimension. PDFs routinely embed images at
/// print resolution (e.g. a 2550×3300 / 8-megapixel full-page background) that
/// are displayed a few hundred pixels wide — decoding them at full size wastes
/// tens of MB per image and pushes a huge texture through the renderer's image
/// path (a known failure mode for the GPU image atlas). hayro's resolution hint
/// lets the decoder extract a smaller version; `scale_factors` (handled in
/// [`eff`]) keep the placement exact. 1600px keeps a document page crisp at any
/// reasonable on-screen size while bounding memory.
const MAX_IMAGE_DIM: u32 = 1600;

/// A resolution hint capping the larger side to [`MAX_IMAGE_DIM`], preserving
/// aspect — or `None` when the image is already within the cap (decode as-is).
fn cap_hint(w: u32, h: u32) -> Option<(u32, u32)> {
    let max = w.max(h);
    if max <= MAX_IMAGE_DIM {
        return None;
    }
    let s = MAX_IMAGE_DIM as f64 / max as f64;
    Some((((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1)))
}

/// Convert a hayro image + its unit→device affine into a canvas image blit.
/// `None` if the image can't be decoded, or is a pattern-painted stencil (not
/// modeled yet).
pub fn convert(image: Image<'_, '_>, transform: Affine) -> Option<(ImageSource, Rect, Affine)> {
    match image {
        Image::Raster(r) => {
            let id = r.cache_key();
            let mut out = None;
            // Cap the decode resolution: a print-res image shown a few hundred px
            // wide is decoded smaller (scale_factors keep placement exact).
            let hint = cap_hint(r.width(), r.height());
            r.with_rgba(
                |data, alpha| {
                    out = build_rgba(id, &data, alpha.as_ref())
                        .map(|src| (src, decoded_dst(&data), eff(transform, &data)));
                },
                hint,
            );
            out
        }
        Image::Stencil(s) => {
            let id = s.cache_key();
            let mut out = None;
            s.with_stencil(
                |mask, paint| {
                    // A stencil paints `paint`'s color through the 1-channel mask
                    // (mask value = coverage). Only solid-color stencils are
                    // modeled; pattern-painted stencils are skipped upstream.
                    if let PdfPaint::Color(c) = paint {
                        let [r, g, b, a] = c.to_rgba().to_rgba8();
                        let (w, h) = (mask.width, mask.height);
                        let mut rgba = Vec::with_capacity(mask.data.len() * 4);
                        for &m in &mask.data {
                            // Premultiply the mask coverage into alpha.
                            let av = ((m as u16 * a as u16) / 255) as u8;
                            rgba.extend_from_slice(&[r, g, b, av]);
                        }
                        let src = ImageSource::from_rgba8(id as u64, w, h, rgba);
                        let dst = Rect::new(0.0, 0.0, w as f32, h as f32);
                        let e = transform
                            * Affine::scale_non_uniform(
                                mask.scale_factors.0 as f64,
                                mask.scale_factors.1 as f64,
                            );
                        out = Some((src, dst, e));
                    }
                },
                cap_hint(s.width(), s.height()),
            );
            out
        }
    }
}

/// The decoded image's own pixel rect (what the blit fills before `eff`).
fn decoded_dst(data: &ImageData) -> Rect {
    Rect::new(0.0, 0.0, data.width() as f32, data.height() as f32)
}

/// `transform · scale(scale_factors)` — maps decoded-pixel space → device.
fn eff(transform: Affine, data: &ImageData) -> Affine {
    let (sx, sy) = data.scale_factors();
    transform * Affine::scale_non_uniform(sx as f64, sy as f64)
}

/// Expand hayro [`ImageData`] (+ optional alpha) into straight RGBA8.
fn build_rgba(id: u128, data: &ImageData, alpha: Option<&LumaData>) -> Option<ImageSource> {
    let (w, h) = (data.width(), data.height());
    let px = (w as usize).checked_mul(h as usize)?;
    let mut rgba = Vec::with_capacity(px * 4);

    match data {
        ImageData::Rgb(rgb) => {
            if rgb.data.len() < px * 3 {
                return None;
            }
            match alpha {
                Some(a) if a.data.len() >= px => {
                    for i in 0..px {
                        let j = i * 3;
                        rgba.extend_from_slice(&[rgb.data[j], rgb.data[j + 1], rgb.data[j + 2], a.data[i]]);
                    }
                }
                _ => {
                    for i in 0..px {
                        let j = i * 3;
                        rgba.extend_from_slice(&[rgb.data[j], rgb.data[j + 1], rgb.data[j + 2], 255]);
                    }
                }
            }
        }
        ImageData::Luma(luma) => {
            if luma.data.len() < px {
                return None;
            }
            match alpha {
                Some(a) if a.data.len() >= px => {
                    for i in 0..px {
                        let g = luma.data[i];
                        rgba.extend_from_slice(&[g, g, g, a.data[i]]);
                    }
                }
                _ => {
                    for i in 0..px {
                        let g = luma.data[i];
                        rgba.extend_from_slice(&[g, g, g, 255]);
                    }
                }
            }
        }
    }

    // hayro's resolution hint is advisory — it only down-decodes "in certain
    // cases" (e.g. JPEG2000), so a Flate/raw image comes back full size. Cap it
    // here so the renderer never gets a multi-megapixel texture for a small
    // on-screen image. The ImageSource shrinks; the caller's `dst` keeps the
    // original decoded size, so the canvas scales the smaller image to fill the
    // same area (same placement, far less memory).
    match cap_hint(w, h) {
        Some((tw, th)) => {
            let scaled = downscale_rgba(&rgba, w, h, tw, th);
            Some(ImageSource::from_rgba8(id as u64, tw, th, scaled))
        }
        None => Some(ImageSource::from_rgba8(id as u64, w, h, rgba)),
    }
}

/// Box-average downscale of straight-RGBA8 `(w×h)` → `(tw×th)`. One pass over
/// the source; each destination pixel averages its source footprint. Adequate
/// for a document image (no premultiply finesse needed — interior runs are
/// uniform, only thin antialiased edges differ slightly).
fn downscale_rgba(rgba: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        let sy0 = ty * h / th;
        let sy1 = (((ty + 1) * h / th).max(sy0 + 1)).min(h);
        for tx in 0..tw {
            let sx0 = tx * w / tw;
            let sx1 = (((tx + 1) * w / tw).max(sx0 + 1)).min(w);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy * w + sx) * 4) as usize;
                    r += rgba[i] as u32;
                    g += rgba[i + 1] as u32;
                    b += rgba[i + 2] as u32;
                    a += rgba[i + 3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = ((ty * tw + tx) * 4) as usize;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    out
}
