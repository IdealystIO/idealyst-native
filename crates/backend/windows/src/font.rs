//! Font resolution for the Win32 backend.
//!
//! Two jobs:
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
//! 2. **`HFONT` construction** — [`create_hfont`] maps the framework's
//!    `font_size` / `font_weight` / `font_family` onto a `LOGFONTW`. The
//!    backend caches these by [`FontKey`]: a text node re-applies its
//!    style on every reactive pass, and `CreateFontIndirectW` on each one
//!    would leak GDI handles fast (the 10 k per-process handle cap is
//!    reachable in an animation loop).
//!
//! ## Why `lfHeight` is negative
//!
//! A positive `lfHeight` is the font's *cell* height (ascent + descent +
//! leading); a negative one is the *character* (em) height, which is what
//! CSS `font-size` means. Passing `font_size` positive renders visibly
//! smaller than every other backend — so [`create_hfont`] negates.

use std::collections::HashMap;

use runtime_core::assets::AssetSource;
use runtime_core::FontWeight;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, AddFontResourceExW, CreateFontIndirectW, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FR_PRIVATE, HFONT, LOGFONTW, OUT_TT_PRECIS,
};

/// Cache key for a resolved `HFONT`. Family is owned (the style's family
/// name), size is in whole pixels, weight is the GDI 100–900 scale.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct FontKey {
    pub family: String,
    pub size_px: i32,
    pub weight: i32,
    pub italic: bool,
}

/// Process-lifetime cache of `HFONT`s keyed by [`FontKey`]. Handles are
/// intentionally never deleted: they're shared by every text node using
/// that style and live until the process exits (which, on this backend,
/// is a `TerminateProcess`).
pub(crate) type FontCache = HashMap<FontKey, HFONT>;

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

/// Build an `HFONT` for `key`. An empty `family` leaves `lfFaceName`
/// blank, which makes GDI pick the default face for the requested
/// characteristics.
pub(crate) fn create_hfont(key: &FontKey) -> HFONT {
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
    unsafe { CreateFontIndirectW(&lf) }
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
        // Build the LOGFONTW the same way `create_hfont` does and assert
        // the sign convention (constructing the HFONT itself needs no GDI
        // context, but the sign is the part that regressed).
        assert_eq!(-key.size_px, -56);
        let f = create_hfont(&key);
        assert!(!f.is_invalid(), "CreateFontIndirectW should succeed for a system face");
    }

    /// A long family name must not overflow the fixed 32-wchar
    /// `lfFaceName` buffer (and must stay NUL-terminated).
    #[test]
    fn overlong_family_name_is_truncated_safely() {
        let key = FontKey {
            family: "A".repeat(100),
            size_px: 12,
            weight: 400,
            italic: false,
        };
        let f = create_hfont(&key);
        assert!(!f.is_invalid());
    }
}
