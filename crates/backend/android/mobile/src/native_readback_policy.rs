//! The pure half of the Android native readback (`imp/introspect.rs`): how what
//! a `View` reports over JNI becomes the canonical
//! [`runtime_shared::introspect`] values. Un-gated so the tests run from any
//! host, same pattern as `border_dash_policy`.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

/// A packed `@ColorInt` ARGB as straight sRGB `[r, g, b, a]`, `0.0..=1.0`.
pub(crate) fn argb_to_rgba(argb: i32) -> [f32; 4] {
    let c = argb as u32;
    let channel = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
    [channel(16), channel(8), channel(0), channel(24)]
}

/// The CSS keyword for `RustBorderDrawable.borderStyleCode()`.
pub(crate) fn border_keyword(code: i32) -> &'static str {
    match crate::border_dash_policy::style_from_code(code) {
        runtime_shared::BorderStyle::Solid => "solid",
        runtime_shared::BorderStyle::Dashed => "dashed",
        runtime_shared::BorderStyle::Dotted => "dotted",
    }
}

/// The top-left radius of `GradientDrawable.getCornerRadii()`
/// (`[tl, tl, tr, tr, br, br, bl, bl]`, px) in dp, the corner every reader
/// reports; `None` for a square corner, which reports no key at all.
pub(crate) fn corner_radius_dp(radii_px: &[f32], density: f32) -> Option<f32> {
    let tl = *radii_px.first()?;
    (tl > 0.0).then(|| tl / density)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Color.parseColor("#80336699")`. A sign-extending shift would make the
    /// alpha of a translucent color garbage: the high bit is set.
    #[test]
    fn regression_android_translucent_color_reads_its_alpha() {
        let [r, g, b, a] = argb_to_rgba(0x8033_6699_u32 as i32);
        assert!((r - 0.2).abs() < 0.002 && (g - 0.4).abs() < 0.002 && (b - 0.6).abs() < 0.002);
        assert!((a - 128.0 / 255.0).abs() < 0.002);
    }

    #[test]
    fn border_keywords_follow_the_drawable_codes() {
        use crate::border_dash_policy::{STYLE_DASHED, STYLE_DOTTED, STYLE_SOLID};
        assert_eq!(border_keyword(STYLE_SOLID), "solid");
        assert_eq!(border_keyword(STYLE_DASHED), "dashed");
        assert_eq!(border_keyword(STYLE_DOTTED), "dotted");
    }

    #[test]
    fn corner_radius_is_the_top_left_in_dp() {
        assert_eq!(corner_radius_dp(&[21.0, 21.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 3.5), Some(6.0));
        assert_eq!(corner_radius_dp(&[0.0; 8], 3.5), None);
        assert_eq!(corner_radius_dp(&[], 3.5), None);
    }

    /// The readback calls these by name over JNI: a rename in the Kotlin is a
    /// `NoSuchMethodError` at run time, and the border silently stops reading.
    #[test]
    fn kotlin_border_drawable_exposes_the_readback() {
        let kt = include_str!("../runtime/kotlin/io/idealyst/runtime/RustBorderDrawable.kt");
        for decl in ["fun topWidthPx(): Int", "fun topColorArgb(): Int", "fun borderStyleCode(): Int"] {
            assert!(kt.contains(decl), "RustBorderDrawable.kt is missing `{decl}`");
        }
    }
}
