//! `Element::Image` — bitmap rendering on the Win32 backend.
//!
//! The scene painter (`crate::scene`) calls [`paint_into`] to draw an
//! image into the node's box honoring the `object_fit` style (`Fill` /
//! `Contain` / `Cover`).
//!
//! ## Sources and formats
//!
//! The `src` contract is `docs/image-src.md`; routing, rejection and the
//! pure-Rust decoders are `crate::image_policy` (host-testable). This file
//! turns the policy's [`Decoded`] into a GDI+ image:
//!
//! | bytes                         | decoded by                                   |
//! |-------------------------------|----------------------------------------------|
//! | PNG / JPEG / GIF / BMP / TIFF | GDI+, from an in-memory `IStream`            |
//! | WebP                          | `image-webp` → premultiplied BGRA bitmap      |
//! | SVG                           | resvg, re-rasterized at the displayed size   |
//!
//! GIF draws its first frame (GDI+'s active frame by default), matching the
//! contract. `http(s)` sources are fetched by `crate::image_fetch` and land
//! here as bytes like every other source.
//!
//! In-memory bytes go through `SHCreateMemStream` +
//! `GdipLoadImageFromStream` — no temp files. GDI+ may decode a stream
//! lazily, so the stream is kept alive beside the image ([`Backing`]).

use std::cell::RefCell;

use backend_image_source::SvgImage;
use runtime_shared::ObjectFit;

use windows::Win32::Graphics::GdiPlus::{
    CombineModeIntersect, GdipCreateBitmapFromScan0, GdipDisposeImage, GdipDrawImageRectI,
    GdipGetImageHeight, GdipGetImageWidth, GdipLoadImageFromStream, GdipRestoreGraphics,
    GdipSaveGraphics, GdipSetClipRect, GpBitmap, GpGraphics, GpImage,
};
use windows::Win32::System::Com::IStream;
use windows::Win32::UI::Shell::SHCreateMemStream;

use crate::image_policy::{self, BgraImage, Decoded};

/// GDI+ `PixelFormat32bppPARGB` (`gdipluspixelformats.h`: index 14, 32 bpp,
/// `PixelFormatAlpha | PixelFormatPAlpha | PixelFormatGDI`). A `#define`
/// in the SDK, so the `windows` crate has no constant for it. Premultiplied
/// BGRA in memory — what [`BgraImage`] holds.
const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

/// What a GDI+ image reads its pixels from after creation. GDI+ keeps a
/// pointer to either one (a stream it may decode lazily; a `Scan0` buffer
/// it wraps without copying), so it must outlive the `GpImage`.
enum Backing {
    Stream(#[allow(dead_code)] IStream),
    Pixels(#[allow(dead_code)] Vec<u8>),
}

/// A decoded GDI+ image plus its natural pixel size. Wraps the raw
/// `*mut GpImage` so the paint state can own it and dispose on drop.
pub(crate) struct DecodedImage {
    image: *mut GpImage,
    natural: (u32, u32),
    /// Dropped after `image` is disposed (field drop runs after `Drop::drop`).
    _backing: Backing,
}

impl Drop for DecodedImage {
    fn drop(&mut self) {
        if !self.image.is_null() {
            unsafe {
                let _ = GdipDisposeImage(self.image);
            }
        }
    }
}

/// A displayable image: a bitmap, or an SVG plus its current raster.
pub(crate) enum ImageSource {
    Bitmap(DecodedImage),
    Svg {
        svg: SvgImage,
        /// `(pixels per intrinsic point, raster)` — rebuilt by
        /// [`paint_into`] when the displayed size needs a denser one.
        raster: RefCell<Option<(f64, DecodedImage)>>,
    },
}

impl ImageSource {
    /// Natural size for layout and `on_load`: a bitmap's pixel size, an
    /// SVG's intrinsic size.
    pub(crate) fn natural(&self) -> (f32, f32) {
        match self {
            ImageSource::Bitmap(d) => (d.natural.0 as f32, d.natural.1 as f32),
            ImageSource::Svg { svg, .. } => image_policy::svg_natural(svg),
        }
    }
}

/// Paint state for one image node.
pub(crate) struct ImagePaint {
    /// The image, or `None` while a fetch is in flight or after a failure
    /// (`on_error` has fired) — painted as a blank box.
    pub source: Option<ImageSource>,
    /// The `src` this node shows or is loading (the `Unchanged` guard).
    pub src: Option<String>,
    /// How the bitmap fits the box. Defaults to the framework default,
    /// `Contain` (aspect-fit, letterboxed).
    pub object_fit: ObjectFit,
}

impl Default for ImagePaint {
    fn default() -> Self {
        ImagePaint { source: None, src: None, object_fit: ObjectFit::Contain }
    }
}

// =========================================================================
// Loading
// =========================================================================

/// Turn the policy's decoded bytes into a displayable image. `None` when
/// GDI+ refuses the bytes (not an image it has a codec for).
pub(crate) fn realize(decoded: Decoded<'_>) -> Option<ImageSource> {
    match decoded {
        Decoded::Platform(bytes) => from_encoded(&bytes).map(ImageSource::Bitmap),
        Decoded::Pixels(px) => from_bgra(px).map(ImageSource::Bitmap),
        Decoded::Svg(svg) => {
            // First raster at intrinsic size so a zero-area document fails
            // here (→ `on_error`) rather than painting nothing forever.
            let first = svg.rasterize(1.0)?;
            let bitmap = from_bgra(image_policy::rgba_premul_to_bgra(
                first.width_px,
                first.height_px,
                first.rgba_premultiplied,
            ))?;
            Some(ImageSource::Svg { svg, raster: RefCell::new(Some((first.scale, bitmap))) })
        }
    }
}

/// Decode encoded bytes (PNG/JPEG/GIF/BMP/TIFF/ICO) with GDI+ from memory.
fn from_encoded(bytes: &[u8]) -> Option<DecodedImage> {
    // SHCreateMemStream copies `bytes`, so the stream owns its data.
    let stream = unsafe { SHCreateMemStream(Some(bytes)) }?;
    unsafe {
        let mut image: *mut GpImage = std::ptr::null_mut();
        if GdipLoadImageFromStream(&stream, &mut image).0 != 0 || image.is_null() {
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        let _ = GdipGetImageWidth(image, &mut w);
        let _ = GdipGetImageHeight(image, &mut h);
        Some(DecodedImage { image, natural: (w, h), _backing: Backing::Stream(stream) })
    }
}

/// Wrap premultiplied BGRA pixels in a GDI+ bitmap (no codec involved).
fn from_bgra(px: BgraImage) -> Option<DecodedImage> {
    let (w, h) = (i32::try_from(px.width).ok()?, i32::try_from(px.height).ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let pixels = px.bgra_premultiplied;
    let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
    unsafe {
        if GdipCreateBitmapFromScan0(
            w,
            h,
            w * 4,
            PIXEL_FORMAT_32BPP_PARGB,
            Some(pixels.as_ptr()),
            &mut bitmap,
        )
        .0 != 0
            || bitmap.is_null()
        {
            return None;
        }
    }
    // A GpBitmap IS a GpImage (GDI+'s flat API takes either pointer).
    Some(DecodedImage {
        image: bitmap as *mut GpImage,
        natural: (px.width, px.height),
        _backing: Backing::Pixels(pixels),
    })
}

// =========================================================================
// Scene-paint entry point
// =========================================================================

/// Compute the destination rect `(x, y, w, h)` for `natural` in a
/// `cw × ch` box under `fit`. `Cover` returns a rect larger than the box
/// (negative offsets); the painter clips the overflow to the node rect.
fn dest_rect(natural: (f32, f32), cw: i32, ch: i32, fit: ObjectFit) -> (i32, i32, i32, i32) {
    let (nw, nh) = (natural.0.max(1.0), natural.1.max(1.0));
    let (cwf, chf) = (cw as f32, ch as f32);
    match fit {
        ObjectFit::Fill => (0, 0, cw, ch),
        ObjectFit::Contain | ObjectFit::Cover => {
            let sx = cwf / nw;
            let sy = chf / nh;
            let scale = if matches!(fit, ObjectFit::Cover) { sx.max(sy) } else { sx.min(sy) };
            let dw = (nw * scale).round() as i32;
            let dh = (nh * scale).round() as i32;
            let dx = (cw - dw) / 2;
            let dy = (ch - dh) / 2;
            (dx, dy, dw, dh)
        }
    }
}

/// The SVG raster for a `w × h` box: the cached one when its density
/// still suffices, else a fresh one at the shared `raster_scale` rule
/// (enough pixels that no `ObjectFit` upscales it, capped at
/// `MAX_SVG_RASTER_EDGE_PX`). Screen scale is 1 — this backend is
/// DPI-unaware, so layout px are device px.
fn svg_raster<'a>(
    svg: &SvgImage,
    cache: &'a RefCell<Option<(f64, DecodedImage)>>,
    w: f32,
    h: f32,
) -> std::cell::Ref<'a, Option<(f64, DecodedImage)>> {
    let want = svg.raster_scale((w as f64, h as f64), 1.0);
    let stale = cache.borrow().as_ref().is_none_or(|(scale, _)| *scale != want);
    if stale {
        let fresh = svg.rasterize(want).and_then(|r| {
            let scale = r.scale;
            from_bgra(image_policy::rgba_premul_to_bgra(r.width_px, r.height_px, r.rgba_premultiplied))
                .map(|d| (scale, d))
        });
        // Keep the previous raster if the new one fails (allocation): a
        // blurry image beats a vanished one.
        if fresh.is_some() {
            *cache.borrow_mut() = fresh;
        }
    }
    cache.borrow()
}

/// Draw the image into the current graphics at `(0, 0)`..`(w, h)` —
/// world transform already positions the node's box. `Cover` overflow is
/// clipped to the box. (Cumulative opacity is not yet applied to bitmaps
/// — that needs a per-draw `ImageAttributes` color matrix; images render
/// at full alpha under a faded ancestor. Documented gap, not a silent
/// wrong-path: no current example animates opacity over an image.)
pub(crate) unsafe fn paint_into(g: *mut GpGraphics, p: &ImagePaint, w: f32, h: f32) {
    let Some(source) = &p.source else {
        return;
    };
    let (cw, ch) = (w.round() as i32, h.round() as i32);
    if cw <= 0 || ch <= 0 {
        return;
    }
    let (dx, dy, dw, dh) = dest_rect(source.natural(), cw, ch, p.object_fit);
    let svg_guard;
    let image = match source {
        ImageSource::Bitmap(d) => d.image,
        // Rasterize for the size actually drawn (the dest rect, which
        // `Cover` makes larger than the box).
        ImageSource::Svg { svg, raster } => {
            svg_guard = svg_raster(svg, raster, dw as f32, dh as f32);
            match svg_guard.as_ref() {
                Some((_, d)) => d.image,
                None => return,
            }
        }
    };
    if image.is_null() {
        return;
    }
    // `Cover` deliberately overflows the box; clip so the crop matches
    // the other backends.
    let mut state: u32 = 0;
    let _ = GdipSaveGraphics(g, &mut state);
    let _ = GdipSetClipRect(g, 0.0, 0.0, w, h, CombineModeIntersect);
    let _ = GdipDrawImageRectI(g, image, dx, dy, dw.max(1), dh.max(1));
    let _ = GdipRestoreGraphics(g, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_letterboxes_centered() {
        // 100×100 image in a 200×100 box, Contain → 100×100 centered
        // (scale = min(2.0, 1.0) = 1.0), offset x = 50.
        let (x, y, w, h) = dest_rect((100.0, 100.0), 200, 100, ObjectFit::Contain);
        assert_eq!((x, y, w, h), (50, 0, 100, 100));
    }

    #[test]
    fn cover_fills_and_overflows() {
        // 100×100 in 200×100, Cover → scale = max(2.0,1.0)=2.0 → 200×200,
        // vertically overflowing (y = -50), horizontally exact.
        let (x, y, w, h) = dest_rect((100.0, 100.0), 200, 100, ObjectFit::Cover);
        assert_eq!((x, y, w, h), (0, -50, 200, 200));
    }

    #[test]
    fn fill_ignores_aspect() {
        let (x, y, w, h) = dest_rect((100.0, 50.0), 200, 100, ObjectFit::Fill);
        assert_eq!((x, y, w, h), (0, 0, 200, 100));
    }

    /// The conformance vectors through the REAL decode path, GDI+
    /// included (the host-portable half is `tests/image_src.rs`). Runs on
    /// Windows only, like the rest of this crate's unit tests.
    #[test]
    fn sync_vectors_through_gdiplus() {
        use runtime_shared::image_src::vectors::{Expect, SYNC};
        crate::ensure_gdiplus();
        let assets = std::collections::HashMap::new();
        for (src, expect) in SYNC {
            let got = match image_policy::resolve(None, src, &assets, |_: &str| None) {
                image_policy::Resolution::Ready(d) => realize(d).map(|s| s.natural()),
                _ => None,
            };
            match expect {
                Expect::Loads { width, height } => {
                    assert_eq!(got, Some((*width as f32, *height as f32)), "{src:?}")
                }
                Expect::Errors => assert_eq!(got, None, "{src:?}"),
            }
        }
        // Bytes GDI+ has no codec for are an error, not a blank success.
        let junk = image_policy::Decoded::Platform(b"not an image".as_slice().into());
        assert!(realize(junk).is_none());
    }
}
