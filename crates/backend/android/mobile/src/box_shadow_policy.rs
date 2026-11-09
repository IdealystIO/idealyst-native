//! A style `shadow` (`x`, `y`, `blur`, color) as Android's elevation shadow.
//!
//! Android draws a view's drop shadow from its outline at its elevation,
//! lit from above the screen, so it cannot take a free offset: the shadow
//! always falls downward and grows with elevation. The mapping keeps what
//! the platform can honor — how far the shadow spreads and how dark it is —
//! and lets the fixed light source supply the downward offset that every
//! real-world use of `shadow` (cards, menus, popovers) asks for anyway.
//!
//! Darkness needs a correction. The shadow colors set on the view
//! (`setOutlineAmbientShadowColor` / `setOutlineSpotShadowColor`, API 28+)
//! are multiplied by the theme's `ambientShadowAlpha` / `spotShadowAlpha`
//! before drawing, so passing a style color like `rgba(0,0,0,0.15)` through
//! untouched paints at 0.15 × 0.19 — invisible. The spot alpha is divided
//! back out so the darkest part of the shadow lands near the alpha the
//! author wrote, which is what web's `box-shadow` and iOS's `shadowOpacity`
//! show.
//!
//! Un-gated so the tests run on the host; the JNI half is
//! `style::apply_box_shadow`.

/// The Material theme's `spotShadowAlpha`, which scales every spot shadow
/// color. The app themes the Android wrapper generates inherit it.
pub(crate) const THEME_SPOT_SHADOW_ALPHA: f32 = 0.19;

/// Elevation in dp for a shadow with this offset and blur.
///
/// An elevation shadow's soft edge is roughly twice the elevation wide, so
/// half the blur reproduces the spread. A hard offset shadow (`blur` 0,
/// `y` set) has no soft edge to match; its offset is the only extent it
/// has, so the elevation is at least `|y|`. `x` only moves a shadow
/// sideways, which a fixed light cannot do. Zero means no shadow.
pub(crate) fn elevation_dp(x: f32, y: f32, blur: f32) -> f32 {
    let _ = x;
    (blur.max(0.0) * 0.5).max(y.abs())
}

/// The ARGB to hand the outline shadow color setters for a style color of
/// `argb`: the same RGB, with alpha scaled so the theme's spot multiplier
/// brings it back to the author's alpha (capped at opaque).
pub(crate) fn outline_shadow_color(argb: u32) -> u32 {
    let alpha = (argb >> 24) as f32 / 255.0;
    let scaled = (alpha / THEME_SPOT_SHADOW_ALPHA).min(1.0);
    let a = (scaled * 255.0).round() as u32;
    (a << 24) | (argb & 0x00FF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_android_shadow_is_drawn_card_and_menu_shadows_get_an_elevation() {
        // The report: `shadow` was ignored, so Menu, Select, popovers and
        // raised plates drew flat on Android. Typical shadows must map to a
        // visible elevation.
        assert_eq!(elevation_dp(0.0, 2.0, 8.0), 4.0);
        assert_eq!(elevation_dp(0.0, 8.0, 24.0), 12.0);
    }

    #[test]
    fn a_hard_offset_shadow_still_lifts_the_view() {
        assert_eq!(elevation_dp(0.0, 4.0, 0.0), 4.0);
        assert_eq!(elevation_dp(0.0, -4.0, 0.0), 4.0);
    }

    #[test]
    fn an_empty_shadow_has_no_elevation() {
        assert_eq!(elevation_dp(0.0, 0.0, 0.0), 0.0);
        assert_eq!(elevation_dp(6.0, 0.0, 0.0), 0.0, "sideways only");
    }

    #[test]
    fn regression_android_shadow_is_visible_alpha_cancels_the_theme_multiplier() {
        // rgba(0,0,0,0.15): passed through untouched it draws at
        // 0.15 × 0.19 ≈ 0.03 — no visible shadow.
        let author = 0x2600_0000; // alpha 38/255 ≈ 0.15
        let set = outline_shadow_color(author);
        let drawn = (set >> 24) as f32 / 255.0 * THEME_SPOT_SHADOW_ALPHA;
        assert!((drawn - 0.15).abs() < 0.01, "drawn alpha {drawn}");
        assert_eq!(set & 0x00FF_FFFF, 0, "the color itself is kept");
    }

    #[test]
    fn a_dark_shadow_caps_at_opaque() {
        assert_eq!(outline_shadow_color(0x80FF_0000), 0xFFFF_0000);
        assert_eq!(outline_shadow_color(0x0012_3456), 0x0012_3456, "transparent stays transparent");
    }
}
