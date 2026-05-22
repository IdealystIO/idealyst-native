//! Inter typeface — all nine upright weights bundled into the
//! binary via the framework's `typeface!` + `face!` macros.
//!
//! The macros wrap `include_bytes!` for the font data, derive a
//! stable `AssetId` / `TypefaceId` from a `const_hash` of the crate
//! name + path, and produce the `Typeface` / `TypefaceFace` literals
//! the backends consume. This is the single supported interface for
//! declaring a typeface anywhere in the workspace — keep it the
//! same in every project so backend cache keys, registry ids, and
//! font-face matching all stay consistent.
//!
//! Each backend registers the bytes through its native font API at
//! first style-apply: web wraps them in a `Blob` URL + `@font-face`
//! rule, iOS hands them to CoreText (`CGFontCreateWithDataProvider`
//! → `CTFontManagerRegisterGraphicsFont`), Android writes them to
//! `cacheDir/idealyst-fonts/<id>.ttf` + `Typeface.createFromFile`.
//! No network fetch on any platform — the typeface ships with the
//! app.
//!
//! Only upright (non-italic) faces are bundled. If italic copy
//! shows up later, add the matching `face!(... style: FontStyle::Italic ...)`
//! entries below.

use framework_core::{face, typeface, FontStyle, FontWeight, SystemFallback, Typeface};

pub static INTER: Typeface = typeface! {
    name: "Inter",
    faces: [
        face!(weight: FontWeight::Thin,       style: FontStyle::Normal, src: "../fonts/Inter-Thin.ttf"),
        face!(weight: FontWeight::ExtraLight, style: FontStyle::Normal, src: "../fonts/Inter-ExtraLight.ttf"),
        face!(weight: FontWeight::Light,      style: FontStyle::Normal, src: "../fonts/Inter-Light.ttf"),
        face!(weight: FontWeight::Normal,     style: FontStyle::Normal, src: "../fonts/Inter-Regular.ttf"),
        face!(weight: FontWeight::Medium,     style: FontStyle::Normal, src: "../fonts/Inter-Medium.ttf"),
        face!(weight: FontWeight::SemiBold,   style: FontStyle::Normal, src: "../fonts/Inter-SemiBold.ttf"),
        face!(weight: FontWeight::Bold,       style: FontStyle::Normal, src: "../fonts/Inter-Bold.ttf"),
        face!(weight: FontWeight::ExtraBold,  style: FontStyle::Normal, src: "../fonts/Inter-ExtraBold.ttf"),
        face!(weight: FontWeight::Black,      style: FontStyle::Normal, src: "../fonts/Inter-Black.ttf"),
    ],
    fallback: SystemFallback::SansSerif,
};
