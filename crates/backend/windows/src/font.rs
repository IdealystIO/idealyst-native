//! Font resolution for the Win32 backend.
//!
//! Three jobs:
//!
//! 1. **Typeface installation** — the framework's declarative `typeface!`
//!    / `face!` assets arrive through
//!    [`Backend::register_typeface`](runtime_shared::Backend::register_typeface)
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

use runtime_shared::assets::AssetSource;
use runtime_shared::FontWeight;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, AddFontResourceExW, CreateFontIndirectW, HDC, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FR_PRIVATE, HFONT, LOGFONTW, OUT_TT_PRECIS,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateFontFamilyFromName, GdipCreateFontFromLogfontW, GdipDeleteFontFamily, GpFont,
    GpFontFamily,
};

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

/// Resolve a CSS `font-family` value — possibly a comma-separated
/// fallback STACK (`system-ui, -apple-system, "Segoe UI", Roboto, …`)
/// — to the first concrete family GDI+ can draw, else `shell`.
///
/// Styles carry the stack verbatim (the web backend hands it to CSS
/// untouched). Passing it through as a `LOGFONTW` face name is a trap:
/// GDI's font MAPPER accepts any garbage face name and silently
/// substitutes (so `CreateFontIndirectW` + measurement "work"), but
/// GDI+'s `GdipCreateFontFromLogfontW` needs a real family and fails —
/// null `GpFont`, painter skips the run, and the text is INVISIBLE
/// while still occupying layout space (the website's sidebar section
/// labels and the "Dark mode" switch label).
///
/// CSS generic/system keywords map to their Windows equivalents;
/// concrete names are probed with `GdipCreateFontFamilyFromName` so
/// the returned family is one GDI+ provably has.
pub(crate) fn resolve_family_stack(stack: &str, shell: &str) -> String {
    for raw in stack.split(',') {
        let name = raw.trim().trim_matches('"').trim_matches('\'').trim();
        if name.is_empty() {
            continue;
        }
        let candidate = match name.to_ascii_lowercase().as_str() {
            // The platform UI font — exactly what the shell font is.
            "system-ui" | "-apple-system" | "blinkmacsystemfont" | "ui-sans-serif"
            | "sans-serif" => shell,
            "serif" | "ui-serif" => "Times New Roman",
            "monospace" | "ui-monospace" => "Consolas",
            _ => name,
        };
        if gdiplus_family_exists(candidate) {
            return candidate.to_string();
        }
    }
    shell.to_string()
}

/// True iff GDI+ can resolve `name` as a font family (its default
/// system collection — the same lookup `GdipCreateFontFromLogfontW`
/// ultimately needs to succeed).
fn gdiplus_family_exists(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut family: *mut GpFontFamily = std::ptr::null_mut();
    unsafe {
        let status =
            GdipCreateFontFamilyFromName(PCWSTR(wide.as_ptr()), std::ptr::null_mut(), &mut family);
        if status.0 == 0 && !family.is_null() {
            let _ = GdipDeleteFontFamily(family);
            true
        } else {
            false
        }
    }
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

    /// The website theme's real font-family stack must land on a
    /// family GDI+ can draw. Passed through raw, GDI measurement
    /// "works" (mapper substitution) but `GdipCreateFontFromLogfontW`
    /// fails and the painter skips the run — sidebar section labels
    /// and the "Dark mode" switch label rendered invisible.
    #[test]
    fn regression_css_font_stack_resolves_to_drawable_family() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
        crate::ensure_gdiplus();
        let stack = r#"system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif"#;
        let family = resolve_family_stack(stack, "Segoe UI");
        assert!(!family.contains(','), "a single concrete family, got {family:?}");
        let key = FontKey { family, size_px: 11, weight: 600, italic: false };
        let dc = unsafe { GetDC(HWND(std::ptr::null_mut())) };
        let mut cache = FontCache::new();
        let entry = entry_for(&mut cache, &key).expect("hfont");
        let gp = gpfont_for(entry, dc);
        unsafe {
            ReleaseDC(HWND(std::ptr::null_mut()), dc);
        }
        assert!(!gp.is_null(), "resolved family must be drawable by GDI+");
        // Unknown-first stacks fall through; garbage-only falls back.
        assert_eq!(resolve_family_stack("NoSuchFont, Arial", "Segoe UI"), "Arial");
        assert_eq!(resolve_family_stack("NoSuchFont, AlsoMissing", "Segoe UI"), "Segoe UI");
        assert_eq!(resolve_family_stack("monospace", "Segoe UI"), "Consolas");
    }

    /// Every CSS weight must yield a drawable GDI+ font — the painter
    /// silently skips text whose `gpfont` is null, so a weight that
    /// fails `GdipCreateFontFromLogfontW` renders as INVISIBLE text
    /// (the website's SemiBold sidebar section labels and the Medium
    /// "Dark mode" switch label).
    #[test]
    fn regression_all_css_weights_yield_drawable_gpfont() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
        crate::ensure_gdiplus();
        let dc = unsafe { GetDC(HWND(std::ptr::null_mut())) };
        assert!(!dc.is_invalid());
        let mut cache = FontCache::new();
        let mut failures = Vec::new();
        for weight in [100, 200, 300, 400, 500, 600, 700, 800, 900] {
            for size in [11, 13, 16, 56] {
                let key = FontKey {
                    family: "Segoe UI".into(),
                    size_px: size,
                    weight,
                    italic: false,
                };
                match entry_for(&mut cache, &key) {
                    Some(entry) => {
                        if gpfont_for(entry, dc).is_null() {
                            failures.push(format!("weight {weight} size {size}: gpfont null"));
                        }
                    }
                    None => failures.push(format!("weight {weight} size {size}: no HFONT")),
                }
            }
        }
        unsafe {
            ReleaseDC(HWND(std::ptr::null_mut()), dc);
        }
        assert!(failures.is_empty(), "undrawable fonts:\n{}", failures.join("\n"));
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
