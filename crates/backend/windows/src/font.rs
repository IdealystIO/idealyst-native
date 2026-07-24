//! Font resolution for the Win32 backend.
//!
//! Three jobs:
//!
//! 1. **Typeface installation** — the framework's declarative `typeface!`
//!    / `face!` assets arrive through
//!    [`Backend::register_typeface`](runtime_core::Backend::register_typeface)
//!    as TTF bytes (or a bundled path). Win32 needs them installed into
//!    the process font table before `CreateFontIndirectW` can find them by
//!    family name: [`install_face`] uses `AddFontMemResourceEx` for
//!    embedded bytes (process-private, no filesystem round trip) and
//!    `AddFontResourceExW(FR_PRIVATE)` for a bundled path.
//!
//! 2. **`HFONT` construction** for GDI text *measurement* — the layout
//!    pass measures strings with `GetTextExtentPoint32W`, which needs a
//!    selected `HFONT`.
//!
//! 3. **`GpFont` construction** for GDI+ text *drawing* — the scene
//!    painter draws strings with `GdipDrawString`, which takes a GDI+
//!    font object. Both are built from the same `LOGFONTW` so measurement
//!    and rendering agree glyph-for-glyph.
//!
//! Entries are cached by [`FontKey`]: a text node re-applies its style on
//! every reactive pass, and creating GDI objects per apply would leak
//! toward the 10 k per-process GDI handle cap fast in an animation loop.
//!
//! ## Why `lfHeight` is negative
//!
//! A positive `lfHeight` is the font's *cell* height (ascent + descent +
//! leading); a negative one is the *character* (em) height, which is what
//! CSS `font-size` means. Passing `font_size` positive renders visibly
//! smaller than every other backend — so [`build_logfont`] negates.

use std::collections::HashMap;

use runtime_core::assets::AssetSource;
use runtime_core::FontWeight;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, AddFontResourceExW, CreateFontIndirectW, HDC, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FR_PRIVATE, HFONT, LOGFONTW, OUT_TT_PRECIS,
};
use windows::Win32::Graphics::GdiPlus::{GdipCreateFontFromLogfontW, GpFont};

/// Cache key for a resolved font. Family is owned (the style's family
/// name), size is in whole pixels, weight is the GDI 100–900 scale.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct FontKey {
    pub family: String,
    pub size_px: i32,
    pub weight: i32,
    pub italic: bool,
}

/// One cached font: the `LOGFONTW` recipe, the GDI `HFONT` (measurement)
/// and the lazily-built GDI+ `GpFont` (drawing). Handles are intentionally
/// never deleted — they're shared across text nodes and live until the
/// process exits (which, on this backend, is a `TerminateProcess`).
pub(crate) struct FontEntry {
    pub logfont: LOGFONTW,
    pub hfont: HFONT,
    /// Null until the first paint that needs it (GDI+ font creation
    /// wants an HDC, which the scene painter has at paint time).
    pub gpfont: *mut GpFont,
}

/// Process-lifetime cache of fonts keyed by [`FontKey`].
pub(crate) type FontCache = HashMap<FontKey, FontEntry>;

/// Map the framework's [`FontWeight`] onto the GDI 100–900 numeric scale
/// (`FW_THIN`..`FW_HEAVY`), the same numbers CSS uses.
pub(crate) fn weight_to_gdi(w: FontWeight) -> i32 {
    match w {
        FontWeight::Thin => 100,
        FontWeight::ExtraLight => 200,
        FontWeight::Light => 300,
        FontWeight::Normal => 400,
        FontWeight::Medium => 500,
        FontWeight::SemiBold => 600,
        FontWeight::Bold => 700,
        FontWeight::ExtraBold => 800,
        FontWeight::Black => 900,
    }
}

/// Build the `LOGFONTW` recipe for `key`. An empty `family` leaves
/// `lfFaceName` blank, which makes GDI pick the default face for the
/// requested characteristics.
pub(crate) fn build_logfont(key: &FontKey) -> LOGFONTW {
    let mut lf = LOGFONTW {
        // Negative = em height, matching CSS `font-size`. See module docs.
        lfHeight: -key.size_px,
        lfWeight: key.weight,
        lfItalic: u8::from(key.italic),
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_TT_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        ..Default::default()
    };
    // lfFaceName is a fixed 32-wchar buffer and must stay NUL-terminated,
    // so copy at most 31 code units.
    let wide: Vec<u16> = key.family.encode_utf16().take(31).collect();
    lf.lfFaceName[..wide.len()].copy_from_slice(&wide);
    lf
}

/// Get-or-create the cache entry for `key`. The `HFONT` is built
/// eagerly (measurement can happen before any paint); the `GpFont` is
/// deferred to [`gpfont_for`].
pub(crate) fn entry_for<'a>(cache: &'a mut FontCache, key: &FontKey) -> Option<&'a mut FontEntry> {
    if !cache.contains_key(key) {
        let logfont = build_logfont(key);
        let hfont = unsafe { CreateFontIndirectW(&logfont) };
        if hfont.is_invalid() {
            return None;
        }
        cache.insert(
            key.clone(),
            FontEntry { logfont, hfont, gpfont: std::ptr::null_mut() },
        );
    }
    cache.get_mut(key)
}

/// The entry's GDI+ font, created on first use against `hdc` (any DC
/// works — the LOGFONT carries the size in device pixels).
pub(crate) fn gpfont_for(entry: &mut FontEntry, hdc: HDC) -> *mut GpFont {
    if entry.gpfont.is_null() {
        let mut f: *mut GpFont = std::ptr::null_mut();
        let status = unsafe { GdipCreateFontFromLogfontW(hdc, &entry.logfont, &mut f) };
        if status.0 == 0 && !f.is_null() {
            entry.gpfont = f;
        }
    }
    entry.gpfont
}

/// Install one typeface face into the process font table so
/// `CreateFontIndirectW` can resolve it by family name.
///
/// Fonts added via `AddFontMemResourceEx` are process-private and are NOT
/// enumerable (`EnumFontFamilies` won't list them), but `CreateFont*`
/// resolves them by face name within the process — which is exactly how
/// the backend consumes them. `Remote` sources are skipped: there's no
/// native fetch (same posture as remote images).
pub(crate) fn install_face(source: &AssetSource) {
    match source {
        AssetSource::Embedded { bytes, .. }
        | AssetSource::BundledEmbedded { bytes, .. } => unsafe {
            let mut installed: u32 = 0;
            AddFontMemResourceEx(
                bytes.as_ptr() as *const core::ffi::c_void,
                bytes.len() as u32,
                None,
                &mut installed,
            );
        },
        AssetSource::Bundled { path } => unsafe {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            AddFontResourceExW(PCWSTR(wide.as_ptr()), FR_PRIVATE, None);
        },
        AssetSource::Remote { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_map_to_the_css_numeric_scale() {
        assert_eq!(weight_to_gdi(FontWeight::Thin), 100);
        assert_eq!(weight_to_gdi(FontWeight::Normal), 400);
        assert_eq!(weight_to_gdi(FontWeight::Bold), 700);
        assert_eq!(weight_to_gdi(FontWeight::Black), 900);
    }

    /// `lfHeight` must be NEGATIVE so GDI treats the value as the em
    /// height (CSS `font-size`) rather than the cell height — a positive
    /// value renders noticeably smaller than the other backends.
    #[test]
    fn regression_lfheight_is_negative_em_height() {
        let key = FontKey { family: "Segoe UI".into(), size_px: 56, weight: 700, italic: false };
        let lf = build_logfont(&key);
        assert_eq!(lf.lfHeight, -56);
        assert_eq!(lf.lfWeight, 700);
    }

    /// A long family name must not overflow the fixed 32-wchar
    /// `lfFaceName` buffer (and must stay NUL-terminated).
    #[test]
    fn overlong_family_name_is_truncated_safely() {
        let key = FontKey { family: "A".repeat(100), size_px: 12, weight: 400, italic: false };
        let lf = build_logfont(&key);
        // 31 chars + the NUL terminator the Default zeroed in.
        assert_eq!(lf.lfFaceName[31], 0);
        assert_eq!(lf.lfFaceName[30], 'A' as u16);
    }

    /// The same key must reuse one cache entry (GDI handle dedup).
    #[test]
    fn cache_returns_same_entry_for_same_key() {
        let mut cache = FontCache::new();
        let key = FontKey { family: String::new(), size_px: 14, weight: 400, italic: false };
        let a = entry_for(&mut cache, &key).map(|e| e.hfont.0 as usize);
        let b = entry_for(&mut cache, &key).map(|e| e.hfont.0 as usize);
        assert!(a.is_some());
        assert_eq!(a, b);
        assert_eq!(cache.len(), 1);
    }
}
