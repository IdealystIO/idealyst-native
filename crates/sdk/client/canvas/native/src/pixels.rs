//! Pure pixel-format conversions for the CPU renderers that hand straight-alpha
//! RGBA8 (the `MediaStream` / `ImageSource` layout) to a 2D engine whose
//! surfaces are PREMULTIPLIED: Android's `ARGB_8888` `Bitmap` and Cairo's
//! `ARgb32` `ImageSurface`.
//!
//! No platform dependency, so `tests/pixels.rs` includes this file by path and
//! checks it on every host — the renderers that use it (`android`, `linux`)
//! only compile for their own targets. Included by `#[path]` from each
//! renderer module rather than declared in `lib.rs`, so it is compiled only
//! into the targets that need it.

#![allow(dead_code)] // each including module uses a subset

/// `c` premultiplied by `a`, rounded to nearest (`c·a/255`).
#[inline]
pub fn premul(c: u8, a: u8) -> u8 {
    ((c as u32 * a as u32 + 127) / 255) as u8
}

/// Premultiply tightly-packed straight-alpha RGBA8 in place (byte order kept:
/// R, G, B, A). Opaque pixels are left untouched, so an all-opaque camera
/// frame costs one alpha compare per pixel.
pub fn premultiply_rgba8(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a == 255 {
            continue;
        }
        px[0] = premul(px[0], a);
        px[1] = premul(px[1], a);
        px[2] = premul(px[2], a);
    }
}

/// Convert tightly-packed straight-alpha RGBA8 (`width × height`) into Cairo
/// `ARgb32` rows of `stride` bytes in `out`.
///
/// Cairo's `ARgb32` is a NATIVE-ENDIAN `u32` per pixel, `0xAARRGGBB`, with
/// premultiplied color. So each pixel is premultiplied and packed into that
/// `u32`, then written with `to_ne_bytes` — which lands as bytes B, G, R, A on
/// little-endian hosts and A, R, G, B on big-endian ones. Skipping the
/// premultiply renders every semi-transparent pixel too bright; writing a fixed
/// byte order would swap channels on a big-endian host.
///
/// Rows beyond the data in `rgba`, or beyond `out`, are left untouched.
pub fn rgba8_to_cairo_argb32(rgba: &[u8], width: usize, height: usize, stride: usize, out: &mut [u8]) {
    for y in 0..height {
        let src = y * width * 4;
        let dst = y * stride;
        if src + width * 4 > rgba.len() || dst + width * 4 > out.len() {
            break;
        }
        for x in 0..width {
            let s = src + x * 4;
            let (r, g, b, a) = (rgba[s], rgba[s + 1], rgba[s + 2], rgba[s + 3]);
            let px = ((a as u32) << 24)
                | ((premul(r, a) as u32) << 16)
                | ((premul(g, a) as u32) << 8)
                | premul(b, a) as u32;
            out[dst + x * 4..dst + x * 4 + 4].copy_from_slice(&px.to_ne_bytes());
        }
    }
}
