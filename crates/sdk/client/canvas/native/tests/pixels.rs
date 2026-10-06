//! Host tests for the straight-RGBA → premultiplied conversions the Android
//! (`ARGB_8888` Bitmap) and Linux (Cairo `ARgb32`) canvas renderers use to
//! upload texture layers and images. Those renderers only compile for their own
//! targets; the conversions are pure, so they're included here by path and
//! checked on every host.

#[path = "../src/pixels.rs"]
mod pixels;

use pixels::{premul, premultiply_rgba8, rgba8_to_cairo_argb32};

/// Regression: Android `copyPixelsFromBuffer` copies bytes verbatim into a
/// premultiplied Bitmap, so straight-alpha input composited too bright (a
/// half-transparent white pixel drew as opaque white). The upload must
/// premultiply; opaque and fully transparent pixels are the edge cases.
#[test]
fn regression_android_bitmap_upload_premultiplies_straight_alpha() {
    let mut px = vec![
        255, 255, 255, 128, // half-transparent white
        200, 100, 50, 255, // opaque: untouched
        90, 80, 70, 0, // fully transparent: color zeroed
    ];
    premultiply_rgba8(&mut px);
    assert_eq!(&px[0..4], &[128, 128, 128, 128]);
    assert_eq!(&px[4..8], &[200, 100, 50, 255]);
    assert_eq!(&px[8..12], &[0, 0, 0, 0]);
}

#[test]
fn premul_rounds_to_nearest() {
    assert_eq!(premul(255, 255), 255);
    assert_eq!(premul(255, 0), 0);
    assert_eq!(premul(1, 128), 1); // 0.502 → 1
    assert_eq!(premul(100, 51), 20);
}

/// Cairo `ARgb32` is a native-endian `0xAARRGGBB` u32 with premultiplied
/// color, rows padded to `stride`. Reading each pixel back as a native-endian
/// u32 must give exactly that packing on any host endianness, and stride
/// padding must be left alone.
#[test]
fn cairo_argb32_is_native_endian_premultiplied_with_stride() {
    // 2×2: red opaque, green half alpha / blue opaque, transparent white.
    let rgba = [
        255, 0, 0, 255, 0, 255, 0, 128, //
        0, 0, 255, 255, 255, 255, 255, 0,
    ];
    let stride = 12; // 8 bytes of pixels + 4 bytes padding per row
    let mut out = vec![0xEEu8; stride * 2];
    rgba8_to_cairo_argb32(&rgba, 2, 2, stride, &mut out);
    let at = |x: usize, y: usize| {
        let o = y * stride + x * 4;
        u32::from_ne_bytes(out[o..o + 4].try_into().unwrap())
    };
    assert_eq!(at(0, 0), 0xFF_FF_00_00);
    assert_eq!(at(1, 0), 0x80_00_80_00);
    assert_eq!(at(0, 1), 0xFF_00_00_FF);
    assert_eq!(at(1, 1), 0x00_00_00_00);
    assert_eq!(&out[8..12], &[0xEE; 4], "row padding must not be written");
    if cfg!(target_endian = "little") {
        // The byte order Cairo reads on every shipping Linux target.
        assert_eq!(&out[0..4], &[0, 0, 255, 255], "red must be B,G,R,A bytes");
    }
}

/// Short input (a truncated frame) must not panic or write past the data.
#[test]
fn cairo_argb32_stops_at_short_input() {
    let rgba = [10, 20, 30, 255]; // one pixel of a claimed 1×2 image
    let mut out = vec![0u8; 8];
    rgba8_to_cairo_argb32(&rgba, 1, 2, 4, &mut out);
    assert_eq!(&out[4..8], &[0; 4]);
    assert_eq!(u32::from_ne_bytes(out[0..4].try_into().unwrap()), 0xFF_0A_14_1E);
}
