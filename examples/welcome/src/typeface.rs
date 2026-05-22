//! Inter typeface — bundled into the binary via the framework's
//! `face!` macro (which itself wraps `include_bytes!`). Each backend
//! registers the font through its native font API: web wraps the
//! bytes in a `Blob` URL and emits a matching `@font-face` rule;
//! iOS / Android register via their native font APIs. No network
//! fetch on any platform — the typeface ships with the app.
//!
//! Only the Regular weight is bundled in this example; Bold
//! appearances (the welcome headline) get a synthesised bold from
//! the renderer when the matching face isn't present. To get crisp
//! bold on every backend, add an `Inter-Bold.ttf` face entry below.

use framework_core::{face, typeface, FontStyle, FontWeight, SystemFallback, Typeface};

pub static INTER: Typeface = typeface! {
    name: "Inter",
    faces: [
        face!(
            weight: FontWeight::Normal,
            style: FontStyle::Normal,
            src: "../fonts/Inter-Regular.ttf",
        ),
    ],
    fallback: SystemFallback::SansSerif,
};
