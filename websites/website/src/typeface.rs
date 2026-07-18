//! Inter typeface bundle. Mirrors `examples/welcome/src/typeface.rs`
//! \u{2014} the framework's `typeface!` macro registers the face files
//! with the active backend the first time the bundle is referenced
//! from a stylesheet, so applying `font_family: &INTER` to the
//! root container is enough to make the whole tree render in Inter.

use runtime_core::{face, typeface, FontStyle, FontWeight, SystemFallback, Typeface};

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
