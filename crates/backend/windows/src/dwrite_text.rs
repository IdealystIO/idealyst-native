//! DirectWrite text: formats, metrics and measurement for the Direct2D
//! renderer (`d2d.rs`).
//!
//! ## Why measurement lives with the renderer
//!
//! Text must be measured by the engine that draws it. The GDI painter
//! measures with GDI (`wrap::build_gdi`) and draws with GDI+; if DirectWrite
//! drew text GDI had measured, line breaks and right edges would disagree
//! with what is on screen (advances, kerning and hinting all differ). So when
//! the Direct2D renderer is selected, every text measurement goes through
//! here instead — the same [`WrapPlan`] builder (word breaking, `max_lines`
//! truncation, the Taffy contract) fed DirectWrite advances.
//!
//! ## Line box
//!
//! A format's line height is the font's ascent + descent (design units
//! scaled to the size), the same quantity GDI's `"Mg"` cell height measures,
//! so a line advances by the same amount on both renderers. Each format uses
//! UNIFORM line spacing at exactly that height with the baseline at the
//! ascent: a laid-out line's top is its ascender, as with GDI+'s typographic
//! format, and the half-leading the painter adds for a CSS `line-height` sits
//! on top of that.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::core::HSTRING;
use runtime_shared::AssetSource;
use windows::core::Interface;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory5, IDWriteFontCollection, IDWriteFontCollection1,
    IDWriteFontFile, IDWriteInMemoryFontFileLoader, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_CLUSTER_METRICS, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_WORD_WRAPPING_NO_WRAP,
};

use crate::font::FontKey;
use crate::wrap::WrapPlan;

/// Family used when a key names none (GDI picks the shell default the same
/// way for an empty face name).
const FALLBACK_FAMILY: &str = "Segoe UI";

/// Faces whose metrics stand in for a family the collection doesn't have,
/// first present wins: the Windows UI face, then what Wine ships (no Segoe
/// UI there), then long-standing Windows faces. DirectWrite substitutes its
/// own fallback at draw time; this only keeps the line box and the
/// introspection report on a real face instead of constants.
const METRIC_FALLBACKS: &[&str] = &["Segoe UI", "Tahoma", "Arial", "Microsoft Sans Serif"];

/// Layout box for single-line measurement — effectively unbounded.
const UNBOUNDED: f32 = 1.0e6;

/// One resolved format plus its line metrics, in logical px.
pub(crate) struct TextFormat {
    pub format: IDWriteTextFormat,
    /// Ascent + descent at the key's size — the line box height.
    pub height: f32,
}

/// DirectWrite factory + per-[`FontKey`] format cache.
pub(crate) struct DwText {
    factory: IDWriteFactory,
    formats: RefCell<HashMap<FontKey, Rc<TextFormat>>>,
    /// The app's registered typefaces (`register_typeface`).
    custom: RefCell<Option<CustomFonts>>,
}

/// App-registered faces as a DirectWrite collection.
///
/// The GDI path installs typefaces with `AddFontMemResourceEx` /
/// `AddFontResourceEx(FR_PRIVATE)`, which makes them visible to GDI's
/// `CreateFont*` in this process — and to nothing else: DirectWrite's system
/// collection never sees them, so a registered family silently fell back to
/// a system font. They are loaded here a second time, from the same bytes or
/// path, into a collection of their own (in-memory loader + font set,
/// Windows 10+), which formats naming a registered family use.
struct CustomFonts {
    /// The shared factory the loader is registered with.
    factory: IDWriteFactory,
    loader: IDWriteInMemoryFontFileLoader,
    files: Vec<IDWriteFontFile>,
    collection: Option<IDWriteFontCollection1>,
}

impl Drop for CustomFonts {
    /// The factory is the process-wide SHARED one and outlives this backend:
    /// DirectWrite requires a custom loader to be unregistered before it is
    /// released, or the factory keeps listing a loader nobody owns.
    fn drop(&mut self) {
        // Everything built from the loader goes before it is unregistered.
        self.collection = None;
        self.files.clear();
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}

impl DwText {
    pub(crate) fn new() -> windows::core::Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(DwText { factory, formats: RefCell::new(HashMap::new()), custom: RefCell::new(None) })
    }

    /// Add one registered face (see [`CustomFonts`]). Formats built before
    /// the registration are dropped so a family that had fallen back
    /// resolves to the new face. `Remote` sources are skipped, as on the
    /// GDI path.
    pub(crate) fn register_face(&self, source: &AssetSource) -> windows::core::Result<()> {
        let f5: IDWriteFactory5 = self.factory.cast()?;
        let mut custom = self.custom.borrow_mut();
        if custom.is_none() {
            let loader = unsafe { f5.CreateInMemoryFontFileLoader()? };
            unsafe { self.factory.RegisterFontFileLoader(&loader)? };
            *custom = Some(CustomFonts { factory: self.factory.clone(), loader, files: Vec::new(), collection: None });
        }
        let c = custom.as_mut().expect("just created");
        let file = unsafe {
            match source {
                AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => {
                    // No owner object: the loader copies the bytes.
                    c.loader.CreateInMemoryFontFileReference(
                        &self.factory,
                        bytes.as_ptr() as *const core::ffi::c_void,
                        bytes.len() as u32,
                        None,
                    )?
                }
                AssetSource::Bundled { path } => self.factory.CreateFontFileReference(&HSTRING::from(*path), None)?,
                AssetSource::Remote { .. } => return Ok(()),
            }
        };
        c.files.push(file);
        // Rebuild the set: faces register a handful of times at boot.
        let builder = unsafe { f5.CreateFontSetBuilder()? };
        for f in &c.files {
            unsafe { builder.AddFontFile(f)? };
        }
        let set = unsafe { builder.CreateFontSet()? };
        c.collection = Some(unsafe { f5.CreateFontCollectionFromFontSet(&set)? });
        drop(custom);
        self.formats.borrow_mut().clear();
        Ok(())
    }

    /// The registered collection when it has `family`.
    fn custom_collection_for(&self, family: &str) -> Option<IDWriteFontCollection> {
        let custom = self.custom.borrow();
        let collection = custom.as_ref()?.collection.as_ref()?;
        let mut index = 0u32;
        let mut exists = windows::core::BOOL(0);
        unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists).ok()? };
        if exists.as_bool() { collection.cast().ok() } else { None }
    }

    /// The format for `key`, created and cached on first use.
    pub(crate) fn format(&self, key: &FontKey) -> Option<Rc<TextFormat>> {
        if let Some(f) = self.formats.borrow().get(key) {
            return Some(f.clone());
        }
        let made = Rc::new(self.create_format(key).ok()?);
        self.formats.borrow_mut().insert(key.clone(), made.clone());
        Some(made)
    }

    fn create_format(&self, key: &FontKey) -> windows::core::Result<TextFormat> {
        let family = if key.family.is_empty() { FALLBACK_FAMILY } else { key.family.as_str() };
        let weight = DWRITE_FONT_WEIGHT(key.weight);
        let style = if key.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL };
        let size = key.size_px.max(1) as f32;
        unsafe {
            let collection = self.custom_collection_for(family);
            let format = self.factory.CreateTextFormat(
                &HSTRING::from(family),
                collection.as_ref(),
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                &HSTRING::from("en-us"),
            )?;
            // No face at all (not even a fallback) is not a state any
            // Windows or Wine install reaches; the constants are the
            // typical sans-serif proportions.
            let (ascent, descent) = font_ascent_descent(&self.factory, &format, weight, style, size)
                .unwrap_or((size * 0.9, size * 0.25));
            let height = ascent + descent;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, height, ascent)?;
            Ok(TextFormat { format, height })
        }
    }

    /// A single-line layout of `text16` in `key`'s format.
    pub(crate) fn layout(&self, text16: &[u16], key: &FontKey) -> Option<IDWriteTextLayout> {
        let fmt = self.format(key)?;
        unsafe { self.factory.CreateTextLayout(text16, &fmt.format, UNBOUNDED, UNBOUNDED).ok() }
    }

    /// The cumulative advance after each UTF-16 unit of `text16` — the
    /// [`WrapPlan`] extents contract. Units inside a multi-unit cluster
    /// (a surrogate pair, a combining sequence) carry the advance BEFORE
    /// the cluster, so a cut never lands inside one with credit for it.
    pub(crate) fn extents(&self, text16: &[u16], key: &FontKey) -> Vec<f32> {
        if text16.is_empty() {
            return Vec::new();
        }
        let Some(layout) = self.layout(text16, key) else {
            return vec![0.0; text16.len()];
        };
        let clusters = cluster_metrics(&layout);
        let mut out = Vec::with_capacity(text16.len());
        let mut acc = 0.0_f32;
        for c in clusters {
            let len = c.length as usize;
            for _ in 1..len {
                out.push(acc);
            }
            acc += c.width;
            if len > 0 {
                out.push(acc);
            }
        }
        out.resize(text16.len(), acc);
        out
    }

    /// Advance width of `text16` on one line.
    pub(crate) fn width(&self, text16: &[u16], key: &FontKey) -> f32 {
        self.extents(text16, key).last().copied().unwrap_or(0.0)
    }

    /// Line box height for `key` (see module docs).
    pub(crate) fn line_height(&self, key: &FontKey) -> Option<f32> {
        self.format(key).map(|f| f.height)
    }

    /// The wrap plan for `text` in `key` — the DirectWrite counterpart of
    /// `wrap::build_gdi`.
    pub(crate) fn build_plan(&self, text: &str, key: &FontKey, fallback_height: f32) -> WrapPlan {
        let height = self.line_height(key).unwrap_or(fallback_height);
        WrapPlan::build_with(text, height, |s| self.extents(s, key))
    }

    /// `(width, height)` of a single line, rounded up like the GDI
    /// measure it replaces (control intrinsics are integer px).
    pub(crate) fn measure_line(&self, text: &str, key: &FontKey, fallback_height: i32) -> (i32, i32) {
        let height = self.line_height(key).map(|h| h.ceil() as i32).unwrap_or(fallback_height);
        if text.is_empty() {
            return (0, height);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        (self.width(&wide, key).ceil() as i32, height)
    }
}

/// What DirectWrite resolved a [`FontKey`] to, for the introspection
/// reader: the family [`matched_font`] found (the registered or system
/// family, else the metric fallback that sizes the line box), the matched
/// face's weight, and the size in logical px.
#[cfg_attr(not(any(test, feature = "robot")), allow(dead_code))]
pub(crate) struct Realized {
    pub family: String,
    pub weight: i32,
    pub size: f32,
}

impl DwText {
    #[cfg_attr(not(any(test, feature = "robot")), allow(dead_code))]
    pub(crate) fn realized(&self, key: &FontKey) -> Option<Realized> {
        let fmt = self.format(key)?;
        let style = if key.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL };
        let (family, font) = matched_font(&self.factory, &fmt.format, DWRITE_FONT_WEIGHT(key.weight), style)?;
        Some(Realized { family, weight: unsafe { font.GetWeight().0 }, size: unsafe { fmt.format.GetFontSize() } })
    }
}

fn cluster_metrics(layout: &IDWriteTextLayout) -> Vec<DWRITE_CLUSTER_METRICS> {
    unsafe {
        let mut count = 0u32;
        // The sizing call reports E_NOT_SUFFICIENT_BUFFER with the count.
        let _ = layout.GetClusterMetrics(None, &mut count);
        let mut out = vec![DWRITE_CLUSTER_METRICS::default(); count as usize];
        if count == 0 || layout.GetClusterMetrics(Some(&mut out), &mut count).is_err() {
            return Vec::new();
        }
        out.truncate(count as usize);
        out
    }
}

/// The face `format` resolves to: its family in the format's collection,
/// else the first [`METRIC_FALLBACKS`] face the system collection has.
/// Returns the family name actually matched with the font.
fn matched_font(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    weight: DWRITE_FONT_WEIGHT,
    style: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE,
) -> Option<(String, windows::Win32::Graphics::DirectWrite::IDWriteFont)> {
    unsafe {
        let len = format.GetFontFamilyNameLength() as usize;
        let mut name = vec![0u16; len + 1];
        format.GetFontFamilyName(&mut name).ok()?;
        let requested = String::from_utf16_lossy(&name[..len]);
        let find = |collection: &IDWriteFontCollection, family: &str| {
            let mut index = 0u32;
            let mut exists = windows::core::BOOL(0);
            collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists).ok()?;
            if !exists.as_bool() {
                return None;
            }
            collection
                .GetFontFamily(index)
                .ok()?
                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
                .ok()
        };
        if let Some(font) = format.GetFontCollection().ok().and_then(|c| find(&c, &requested)) {
            return Some((requested, font));
        }
        let mut system: Option<IDWriteFontCollection> = None;
        factory.GetSystemFontCollection(&mut system, false).ok()?;
        let system = system?;
        METRIC_FALLBACKS
            .iter()
            .find_map(|f| find(&system, f).map(|font| (f.to_string(), font)))
    }
}

/// The matched face's ascent and descent at `size`.
fn font_ascent_descent(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    weight: DWRITE_FONT_WEIGHT,
    style: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE,
    size: f32,
) -> Option<(f32, f32)> {
    let (_, font) = matched_font(factory, format, weight, style)?;
    let mut m = DWRITE_FONT_METRICS::default();
    unsafe { font.GetMetrics(&mut m) };
    let em = m.designUnitsPerEm.max(1) as f32;
    Some((m.ascent as f32 / em * size, m.descent as f32 / em * size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered typeface was invisible to DirectWrite (GDI-private
    /// install only), so its family fell back to a system font. Inter is
    /// not a system font on Windows or Wine: it can only resolve through
    /// the registered collection.
    #[test]
    fn regression_registered_typeface_resolves_in_directwrite() {
        let _serial = crate::render_test_lock();
        static INTER: &[u8] = include_bytes!("../../../../websites/website/fonts/Inter-Regular.ttf");
        let dw = DwText::new().expect("DirectWrite");
        let key = FontKey { family: "Inter".into(), size_px: 16, weight: 400, italic: false };
        let before = dw.format(&key).expect("format");
        assert!(!family_in(&before.format, "Inter"), "Inter must not be a system font here");

        dw.register_face(&AssetSource::Embedded { bytes: INTER, extension: "ttf" }).expect("register");
        let after = dw.format(&key).expect("format");
        assert!(family_in(&after.format, "Inter"), "the format resolves Inter from the registered collection");
        // And it measures in Inter (a different face than the fallback).
        let sample: Vec<u16> = "Hamburgefonstiv".encode_utf16().collect();
        let w = dw.width(&sample, &key);
        assert!(w > 0.0);
        let r = dw.realized(&key).expect("realized");
        assert_eq!((r.family.as_str(), r.weight, r.size), ("Inter", 400, 16.0));
    }

    /// Backends with registered fonts come and go (every test builds one):
    /// dropping one unregisters its loader from the shared factory, and the
    /// next registers its own without disturbing DirectWrite.
    #[test]
    fn custom_fonts_can_be_dropped_and_registered_again() {
        let _serial = crate::render_test_lock();
        static INTER: &[u8] = include_bytes!("../../../../websites/website/fonts/Inter-Regular.ttf");
        let key = FontKey { family: "Inter".into(), size_px: 14, weight: 400, italic: false };
        let sample: Vec<u16> = "Hamburgefonstiv".encode_utf16().collect();
        for _ in 0..8 {
            let dw = DwText::new().expect("DirectWrite");
            dw.register_face(&AssetSource::Embedded { bytes: INTER, extension: "ttf" }).expect("register");
            assert!(dw.width(&sample, &key) > 0.0);
        }
        // Fresh factories and file references after the drops.
        for _ in 0..8 {
            let dw = DwText::new().expect("DirectWrite");
            dw.register_face(&AssetSource::Embedded { bytes: INTER, extension: "ttf" }).expect("register");
            let other = FontKey { family: "Arial".into(), ..key.clone() };
            assert!(dw.width(&sample, &other) > 0.0);
            assert!(dw.width(&sample, &key) > 0.0);
        }
    }

    /// A family the system lacks still gets real metrics and a real
    /// reported face — the first metric fallback present.
    #[test]
    fn missing_family_measures_with_a_real_fallback_face() {
        let _serial = crate::render_test_lock();
        let dw = DwText::new().expect("DirectWrite");
        let key = FontKey { family: "No Such Family Anywhere".into(), size_px: 20, weight: 700, italic: false };
        let r = dw.realized(&key).expect("a fallback face is always present");
        assert!(METRIC_FALLBACKS.contains(&r.family.as_str()), "{}", r.family);
        let h = dw.line_height(&key).unwrap();
        // Real faces put ascent + descent between ~1.0 and ~1.5 em; the
        // constants (0.9 + 0.25 = 1.15 em) would also pass, so also check
        // they weren't used.
        assert!(h > 20.0 && h < 30.0, "line box {h}");
        assert!((h - 23.0).abs() > 0.01, "line box came from the constants, not a face");
    }

    fn family_in(format: &IDWriteTextFormat, family: &str) -> bool {
        unsafe {
            let Ok(collection) = format.GetFontCollection() else { return false };
            let mut index = 0u32;
            let mut exists = windows::core::BOOL(0);
            collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists).is_ok() && exists.as_bool()
        }
    }
}
