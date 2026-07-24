//! `Element::Image` — bitmap rendering on the Win32 backend.
//!
//! The scene painter (`crate::scene`) calls [`paint_into`] to draw a
//! decoded GDI+ `GpImage` into the node's box honoring the `object_fit`
//! style (`Fill` / `Contain` / `Cover`). GDI+ decodes PNG / JPEG / GIF /
//! BMP / TIFF, so the backend supports every raster format the OS ships
//! a codec for.
//!
//! ## Source support
//!
//! | scheme                | handling                                   |
//! |-----------------------|--------------------------------------------|
//! | bare path, `file://`  | `GdipLoadImageFromFile`                     |
//! | `data:…;base64,…`     | base64-decode → temp file → file load       |
//! | `asset://{id}`        | resolved via [`Backend::register_asset`]    |
//! | `http(s)://`          | **unsupported** — blank (no native fetch)   |
//!
//! Remote URLs are a no-op (blank box), matching the framework's
//! documented "Android `ImageView` has no URL loader" posture — a native
//! HTTP client is out of scope for the backend. Bundled assets, embedded
//! bytes, `data:` URLs, and local files all decode.
//!
//! In-memory bytes (embedded assets, `data:` URLs) are staged to a temp
//! file keyed by a content hash and loaded via the file path — this
//! sidesteps the `IStream`/COM plumbing `GdipLoadImageFromStream` needs,
//! and the temp file is written at most once per distinct payload.

use std::io::Write as _;
use std::path::PathBuf;

use runtime_core::ObjectFit;

use windows::core::PCWSTR;
use windows::Win32::Graphics::GdiPlus::{
    CombineModeIntersect, GdipDisposeImage, GdipDrawImageRectI, GdipGetImageHeight,
    GdipGetImageWidth, GdipLoadImageFromFile, GdipRestoreGraphics, GdipSaveGraphics,
    GdipSetClipRect, GpGraphics, GpImage,
};

/// A decoded GDI+ image plus its natural pixel size. Wraps the raw
/// `*mut GpImage` so the paint state can own it and dispose on drop.
pub(crate) struct DecodedImage {
    image: *mut GpImage,
    natural: (u32, u32),
}

impl DecodedImage {
    pub(crate) fn natural(&self) -> (u32, u32) {
        self.natural
    }
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

/// Paint state for one `IdealystImage`, held behind its `GWLP_USERDATA`.
pub(crate) struct ImagePaint {
    /// The decoded bitmap, or `None` for an unsupported / failed source
    /// (remote URL, missing file, undecodable bytes) — painted as a blank
    /// box over the parent background.
    pub image: Option<DecodedImage>,
    /// How the bitmap fits the box. Defaults to the framework default,
    /// `Contain` (aspect-fit, letterboxed).
    pub object_fit: ObjectFit,
}

impl Default for ImagePaint {
    fn default() -> Self {
        ImagePaint { image: None, object_fit: ObjectFit::Contain }
    }
}

// =========================================================================
// Loading
// =========================================================================

/// Load a bitmap from a filesystem path via GDI+. Returns the image + its
/// natural pixel size, or `None` if the file is missing / undecodable.
pub(crate) fn load_image_file(path: &str) -> Option<DecodedImage> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut image: *mut GpImage = std::ptr::null_mut();
        if GdipLoadImageFromFile(PCWSTR(wide.as_ptr()), &mut image).0 != 0 || image.is_null() {
            return None;
        }
        let mut w: u32 = 0;
        let mut h: u32 = 0;
        let _ = GdipGetImageWidth(image, &mut w);
        let _ = GdipGetImageHeight(image, &mut h);
        Some(DecodedImage { image, natural: (w, h) })
    }
}

/// Stage `bytes` to a temp file (named by a content hash so identical
/// payloads reuse one file) and load it via [`load_image_file`]. Used for
/// embedded assets and `data:` URLs — GDI+'s file loader is far simpler to
/// drive than the `IStream` decode path, and the write happens at most
/// once per distinct payload.
pub(crate) fn load_image_bytes(bytes: &[u8], ext: &str) -> Option<DecodedImage> {
    let hash = fnv1a(bytes);
    let ext = if ext.is_empty() { "img" } else { ext };
    let mut path: PathBuf = std::env::temp_dir();
    path.push(format!("idealyst-img-{hash:016x}.{ext}"));
    if !path.exists() {
        let Ok(mut f) = std::fs::File::create(&path) else {
            return None;
        };
        if f.write_all(bytes).is_err() {
            return None;
        }
    }
    load_image_file(&path.to_string_lossy())
}

/// Decode a `data:[<mime>][;base64],<payload>` URL into `(bytes,
/// extension)`. Only base64 payloads are supported (the common case for
/// inline images); a non-base64 (percent-encoded) data URL returns `None`.
pub(crate) fn decode_data_url(src: &str) -> Option<(Vec<u8>, String)> {
    let rest = src.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let (meta, payload) = rest.split_at(comma);
    let payload = &payload[1..]; // skip the comma
    if !meta.contains("base64") {
        return None;
    }
    // Extension from the mime type: `image/png` → `png`, `image/jpeg` →
    // `jpeg`, `image/svg+xml` → `svg+xml` (GDI+ can't decode SVG, but the
    // loader will simply fail → blank, which is the honest outcome).
    let ext = meta
        .split(';')
        .next()
        .and_then(|m| m.strip_prefix("image/"))
        .map(|e| e.to_string())
        .unwrap_or_else(|| "img".to_string());
    let bytes = base64_decode(payload)?;
    Some((bytes, ext))
}

/// Minimal standard-alphabet base64 decoder (RFC 4648). Ignores ASCII
/// whitespace (data URLs are sometimes line-wrapped); rejects any other
/// non-alphabet byte. Avoids adding a `base64` crate dependency for one
/// call site.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    #[inline]
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut quad = [0u8; 4];
    let mut n = 0;
    let mut pad = 0;
    for &c in s.as_bytes() {
        if c == b'=' {
            quad[n] = 0;
            n += 1;
            pad += 1;
        } else if c.is_ascii_whitespace() {
            continue;
        } else {
            quad[n] = val(c)?;
            n += 1;
        }
        if n == 4 {
            out.push((quad[0] << 2) | (quad[1] >> 4));
            if pad < 2 {
                out.push((quad[1] << 4) | (quad[2] >> 2));
            }
            if pad < 1 {
                out.push((quad[2] << 6) | quad[3]);
            }
            n = 0;
            pad = 0;
        }
    }
    Some(out)
}

/// FNV-1a 64-bit hash — used only to name temp files, so collision
/// resistance isn't security-critical; the low collision rate just keeps
/// distinct images in distinct temp files.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// =========================================================================
// Scene-paint entry point
// =========================================================================

/// Compute the destination rect `(x, y, w, h)` for `natural` in a
/// `cw × ch` box under `fit`. `Cover` returns a rect larger than the box
/// (negative offsets); the painter clips the overflow to the node rect.
fn dest_rect(natural: (u32, u32), cw: i32, ch: i32, fit: ObjectFit) -> (i32, i32, i32, i32) {
    let (nw, nh) = (natural.0.max(1) as f32, natural.1.max(1) as f32);
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

/// Draw the image into the current graphics at `(0, 0)`..`(w, h)` —
/// world transform already positions the node's box. `Cover` overflow is
/// clipped to the box. (Cumulative opacity is not yet applied to bitmaps
/// — that needs a per-draw `ImageAttributes` color matrix; images render
/// at full alpha under a faded ancestor. Documented gap, not a silent
/// wrong-path: no current example animates opacity over an image.)
pub(crate) unsafe fn paint_into(g: *mut GpGraphics, p: &ImagePaint, w: f32, h: f32) {
    let Some(img) = &p.image else {
        return;
    };
    let (cw, ch) = (w.round() as i32, h.round() as i32);
    if cw <= 0 || ch <= 0 || img.image.is_null() {
        return;
    }
    let (dx, dy, dw, dh) = dest_rect(img.natural, cw, ch, p.object_fit);
    // `Cover` deliberately overflows the box; clip so the crop matches
    // the other backends.
    let mut state: u32 = 0;
    let _ = GdipSaveGraphics(g, &mut state);
    let _ = GdipSetClipRect(g, 0.0, 0.0, w, h, CombineModeIntersect);
    let _ = GdipDrawImageRectI(g, img.image, dx, dy, dw.max(1), dh.max(1));
    let _ = GdipRestoreGraphics(g, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_known_vector() {
        // "Man" → "TWFu"; "hello" → "aGVsbG8=" (1 pad); "hi" → "aGk=".
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
    }

    #[test]
    fn base64_ignores_whitespace_and_rejects_bad_bytes() {
        assert_eq!(base64_decode("TW\nFu").unwrap(), b"Man");
        assert!(base64_decode("**bad**").is_none());
    }

    #[test]
    fn data_url_splits_mime_and_payload() {
        let (bytes, ext) = decode_data_url("data:image/png;base64,TWFu").unwrap();
        assert_eq!(bytes, b"Man");
        assert_eq!(ext, "png");
    }

    #[test]
    fn data_url_non_base64_unsupported() {
        assert!(decode_data_url("data:text/plain,hello").is_none());
    }

    #[test]
    fn contain_letterboxes_centered() {
        // 100×100 image in a 200×100 box, Contain → 100×100 centered
        // (scale = min(2.0, 1.0) = 1.0), offset x = 50.
        let (x, y, w, h) = dest_rect((100, 100), 200, 100, ObjectFit::Contain);
        assert_eq!((x, y, w, h), (50, 0, 100, 100));
    }

    #[test]
    fn cover_fills_and_overflows() {
        // 100×100 in 200×100, Cover → scale = max(2.0,1.0)=2.0 → 200×200,
        // vertically overflowing (y = -50), horizontally exact.
        let (x, y, w, h) = dest_rect((100, 100), 200, 100, ObjectFit::Cover);
        assert_eq!((x, y, w, h), (0, -50, 200, 200));
    }

    #[test]
    fn fill_ignores_aspect() {
        let (x, y, w, h) = dest_rect((100, 50), 200, 100, ObjectFit::Fill);
        assert_eq!((x, y, w, h), (0, 0, 200, 100));
    }
}
