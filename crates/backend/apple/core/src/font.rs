//! Custom font registration & resolution for the Apple backends.
//!
//! The CoreText/CoreGraphics half — register bytes with the process-
//! wide font manager, read back the PostScript name, and match a
//! `(weight, style)` against the registered face set for a typeface.
//! This is the cross-Apple half (iOS, tvOS, macOS) because CoreText
//! and CoreGraphics are identical across all three.
//!
//! The UIKit/AppKit half — constructing a `UIFont` / `NSFont` from
//! the resolved PostScript name + size, and applying it to a view —
//! lives in the leaf backend crates because it depends on the UI
//! toolkit.
//!
//! Flow:
//!
//! 1. The framework calls `register_asset(id, AssetTag::Font, source)`
//!    the first time a `TypefaceFace` is observed. We decode the bytes
//!    into a `CGFont`, register it with CoreText, and stash the
//!    PostScript name keyed by `AssetId`.
//! 2. Then `register_typeface(id, family_name, faces, fallback)` lands.
//!    We materialize each face into `(weight, style, postscript_name)`
//!    by looking up `face.asset` in the per-asset cache, and store the
//!    record keyed by `TypefaceId`.
//! 3. At style-apply time the leaf crate calls
//!    [`FontRegistry::resolve_typeface`] to get the best-matching
//!    face's PS name. If `None` is returned, the leaf falls back to
//!    its system-font path.
//!
//! The CoreText/CoreGraphics symbols aren't exposed by `objc2-*` at
//! the version this workspace pins, so they're declared inline as
//! `extern "C"` and called through `unsafe` blocks.

use std::collections::HashMap;

use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace, TypefaceId,
};
use runtime_core::{FontStyle, FontWeight};
use objc2::rc::Retained;
use objc2::msg_send_id;
use objc2_foundation::{NSObject, NSString};

// ---------------------------------------------------------------------------
// CoreGraphics / CoreText FFI
// ---------------------------------------------------------------------------

// Opaque pointers from CoreGraphics / CoreText / CoreFoundation. We
// never deref them; we only pass them between the platform functions.
type CFDataRef = *const std::ffi::c_void;
type CFStringRef = *const std::ffi::c_void;
type CFErrorRef = *const std::ffi::c_void;
type CGDataProviderRef = *const std::ffi::c_void;
type CGFontRef = *const std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDataProviderCreateWithCFData(data: CFDataRef) -> CGDataProviderRef;
    fn CGDataProviderRelease(provider: CGDataProviderRef);
    fn CGFontCreateWithDataProvider(provider: CGDataProviderRef) -> CGFontRef;
    fn CGFontRelease(font: CGFontRef);
    fn CGFontCopyPostScriptName(font: CGFontRef) -> CFStringRef;
}

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    fn CTFontManagerRegisterGraphicsFont(font: CGFontRef, error: *mut CFErrorRef) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

// ---------------------------------------------------------------------------
// Registry types
// ---------------------------------------------------------------------------

/// One registered face: the PostScript name we'll hand to UIFont /
/// NSFont, plus the (weight, style) the author authored it under.
#[derive(Clone, Debug)]
struct RegisteredFace {
    weight: FontWeight,
    style: FontStyle,
    postscript_name: String,
}

/// One registered typeface: the family + its faces. The fallback
/// lives on the framework's `Typeface` (a `Copy` POD), so we read it
/// from the caller's value at resolve time rather than caching it
/// here — keeps the registry data minimal.
#[derive(Clone, Debug)]
struct RegisteredTypeface {
    family_name: String,
    faces: Vec<RegisteredFace>,
}

/// Result of a successful typeface resolution. `postscript_name` is
/// the best-matching face's PS name (what UIKit/AppKit want); the
/// `family_name` is the typeface's declared family, useful as a
/// secondary fallback if `+[UIFont fontWithName:postscript_name]`
/// fails (some bundled fonts register under their family name
/// instead of their PS name).
#[derive(Clone, Debug)]
pub struct ResolvedFace<'a> {
    pub family_name: &'a str,
    pub postscript_name: &'a str,
}

/// The shared system-fallback role. Returned to the caller (the leaf
/// backend) so it can dispatch into UIKit/AppKit's nearest equivalent
/// — e.g. UIKit maps `Serif` → `Times New Roman`, AppKit might pick
/// a different default.
pub type SystemFallbackRole = SystemFallback;

/// Per-backend font state. The leaf crates (`backend-ios-core`,
/// `backend-macos`) own one of these and forward
/// `Backend::register_asset` / `register_typeface` calls into it;
/// resolution at style-apply time goes through
/// [`FontRegistry::resolve_typeface`].
#[derive(Default)]
pub struct FontRegistry {
    /// Maps each registered font asset to the PostScript name CoreText
    /// assigned it. Populated by `register_asset` (font branch only).
    /// `register_typeface` consults this when materializing each face.
    asset_psnames: HashMap<AssetId, String>,
    /// One entry per `Typeface` we've seen at least once.
    typefaces: HashMap<TypefaceId, RegisteredTypeface>,
}

impl FontRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Backend::register_asset` entry point for the font branch.
    /// Returns `true` if `kind == AssetTag::Font` and the call was
    /// handled (success or failure); the caller can use that as a
    /// signal to skip the image-cache branch.
    pub fn register_asset(
        &mut self,
        id: AssetId,
        kind: AssetTag,
        source: &AssetSource,
    ) -> bool {
        if kind != AssetTag::Font {
            return false;
        }
        if self.asset_psnames.contains_key(&id) {
            return true;
        }
        // The public `face!` macro always emits `Embedded` (via
        // `include_bytes!`), so this is the only branch fonts can
        // actually take. `Bundled` / `Remote` are intentionally
        // unreachable for fonts — URL-loaded fonts are not supported
        // and only project-shipped files are permitted. The
        // fallthrough is defense-in-depth for anyone hand-constructing
        // a `TypefaceFace` outside the macro.
        let AssetSource::Embedded { bytes, .. } = source else {
            crate::log::apple_log(&format!(
                "[font] register_asset id={:?} skipped — non-Embedded source (URL fonts are unsupported)",
                id
            ));
            return true;
        };
        crate::log::apple_log(&format!(
            "[font] register_asset id={:?} bytes={}",
            id,
            bytes.len()
        ));
        match register_font_bytes(bytes) {
            Some(name) => {
                crate::log::apple_log(&format!(
                    "[font] register_asset id={:?} → PostScript name {:?}",
                    id, name
                ));
                self.asset_psnames.insert(id, name);
            }
            None => {
                crate::log::apple_log(&format!(
                    "[font] register_asset id={:?} FAILED to register with CoreText",
                    id
                ));
            }
        }
        true
    }

    /// `Backend::register_typeface` entry point. Stores the family by
    /// id; each face's PostScript name comes from `asset_psnames`
    /// (populated by `register_asset` immediately prior — the
    /// framework guarantees that ordering).
    pub fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        let mut registered_faces = Vec::with_capacity(faces.len());
        for f in faces {
            if let Some(psname) = self.asset_psnames.get(&f.asset).cloned() {
                registered_faces.push(RegisteredFace {
                    weight: f.weight,
                    style: f.style,
                    postscript_name: psname,
                });
            }
        }
        self.typefaces.insert(
            id,
            RegisteredTypeface {
                family_name: family_name.to_string(),
                faces: registered_faces,
            },
        );
    }

    pub fn unregister_typeface(&mut self, id: TypefaceId) {
        self.typefaces.remove(&id);
    }

    pub fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        if kind == AssetTag::Font {
            self.asset_psnames.remove(&id);
        }
    }

    /// Resolve a registered typeface to the best-matching face. The
    /// leaf backend translates `ResolvedFace` into a UIFont (UIKit)
    /// or NSFont (AppKit) at the requested size.
    ///
    /// Returns `None` if the typeface isn't registered, or has no
    /// registered faces (e.g. its bytes failed to register with
    /// CoreText). The leaf crate is then expected to fall back to
    /// its `SystemFallback`-driven path.
    pub fn resolve_typeface(
        &self,
        t: &Typeface,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<ResolvedFace<'_>> {
        let entry = self.typefaces.get(&t.id)?;
        let face = pick_face(&entry.faces, weight, style)?;
        Some(ResolvedFace {
            family_name: entry.family_name.as_str(),
            postscript_name: face.postscript_name.as_str(),
        })
    }
}

// ---------------------------------------------------------------------------
// CGFont registration
// ---------------------------------------------------------------------------

/// Decode `bytes`, register the resulting `CGFont` with the process-
/// wide CoreText manager, and read back its PostScript name. Returns
/// `None` on any failure path (unsupported font format, registration
/// rejected by CoreText, missing PostScript name, etc.) so the caller
/// can fall back to a system font instead of crashing the apply path.
fn register_font_bytes(bytes: &[u8]) -> Option<String> {
    unsafe {
        let data: Retained<NSObject> = msg_send_id![
            objc2::class!(NSData),
            dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void,
            length: bytes.len()
        ];
        // NSData is toll-free-bridged to CFDataRef. The cast is the
        // standard ObjC ↔ CF interop pattern.
        let cf_data: CFDataRef = &*data as *const NSObject as *const _;
        let provider = CGDataProviderCreateWithCFData(cf_data);
        if provider.is_null() {
            return None;
        }
        let cg_font = CGFontCreateWithDataProvider(provider);
        CGDataProviderRelease(provider);
        if cg_font.is_null() {
            return None;
        }
        let mut err: CFErrorRef = std::ptr::null();
        let ok = CTFontManagerRegisterGraphicsFont(cg_font, &mut err as *mut _);
        if !err.is_null() {
            CFRelease(err);
        }
        // We deliberately tolerate `ok == false` here: a duplicate
        // registration (same PostScript name across two app launches
        // sharing one process) is reported as a failure but the font
        // is still usable. Read the name and try.
        let ps_ref = CGFontCopyPostScriptName(cg_font);
        let name = if ps_ref.is_null() {
            None
        } else {
            // CFStringRef ↔ NSString is toll-free-bridged.
            let ns: &NSString = &*(ps_ref as *const NSString);
            let s = ns.to_string();
            CFRelease(ps_ref);
            Some(s)
        };
        // Per CoreText conventions the CGFont is retained by the
        // manager when registration succeeds; release our local ref
        // either way (the registration call doesn't transfer
        // ownership in the CG sense).
        CGFontRelease(cg_font);
        if !ok && name.is_some() {
            // Registration may have failed because the font is
            // already present (most often during hot-reload). The
            // name is still valid for lookup.
        }
        name
    }
}

// ---------------------------------------------------------------------------
// Face matching
// ---------------------------------------------------------------------------

/// Pick the registered face that best matches `(weight, style)`. The
/// scoring is simple-first: exact-style match wins; within that,
/// closest weight wins. Returns `None` only if the typeface has zero
/// registered faces.
fn pick_face<'a>(
    faces: &'a [RegisteredFace],
    weight: FontWeight,
    style: FontStyle,
) -> Option<&'a RegisteredFace> {
    if faces.is_empty() {
        return None;
    }
    let want_weight = weight_to_axis(weight);
    faces.iter().min_by(|a, b| {
        let key_a = (
            if a.style == style { 0 } else { 1 },
            (weight_to_axis(a.weight) - want_weight).abs(),
        );
        let key_b = (
            if b.style == style { 0 } else { 1 },
            (weight_to_axis(b.weight) - want_weight).abs(),
        );
        key_a.partial_cmp(&key_b).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Map `FontWeight` to its CSS numeric weight (100..900). Used for
/// nearest-weight matching within a typeface's face set; nothing
/// else here.
fn weight_to_axis(w: FontWeight) -> f32 {
    match w {
        FontWeight::Thin => 100.0,
        FontWeight::ExtraLight => 200.0,
        FontWeight::Light => 300.0,
        FontWeight::Normal => 400.0,
        FontWeight::Medium => 500.0,
        FontWeight::SemiBold => 600.0,
        FontWeight::Bold => 700.0,
        FontWeight::ExtraBold => 800.0,
        FontWeight::Black => 900.0,
    }
}
