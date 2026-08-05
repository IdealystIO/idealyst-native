//! Color conversion helpers shared by the paint + text modules.
//!
//! Every author-facing color is a [`runtime_shared::Color`] — a CSS-ish
//! string (`#rrggbb`, `#rrggbbaa`, `rgb(...)`, `rgba(...)`,
//! `transparent`). The framework already owns the canonical parser
//! ([`runtime_shared::color::parse`]); we reuse it rather than
//! re-implementing hex/rgba parsing (and its clamping/edge-case
//! behavior) a second time in the backend. The only backend-specific
//! job here is turning the parsed sRGB `[f32; 4]` into the two shapes
//! GTK wants: a [`gdk::RGBA`] (for GSK color / gradient nodes) and the
//! `u16`-per-channel form Pango attributes take.

use gtk4::gdk;
use runtime_shared::color::Rgba;
use runtime_shared::Color;

/// Parse a [`Color`] to straight sRGB `[r, g, b, a]` in `0..=1`.
///
/// Unparseable / empty colors resolve to fully-transparent rather
/// than a debug magenta — an unset gradient stop or background in
/// this framework legitimately animates *through* transparent (the
/// welcome vignette starts every stop at alpha 0), so a loud
/// fallback would paint garbage over a valid transparent state.
pub fn to_srgb(color: &Color) -> [f32; 4] {
    runtime_shared::color::parse_or(&color.0, Rgba::TRANSPARENT).to_srgb_f32()
}

/// sRGB `[f32; 4]` → [`gdk::RGBA`]. GDK's channels are the same
/// straight-alpha sRGB floats, so this is a field copy.
pub fn to_gdk(c: [f32; 4]) -> gdk::RGBA {
    gdk::RGBA::new(c[0], c[1], c[2], c[3])
}

/// A single sRGB channel (`0..=1`) → Pango's `u16` (`0..=65535`).
/// Pango color attributes (`AttrColor::new_foreground`) take
/// 16-bit-per-channel values, so a mid-gray `0.5` becomes `32767`.
pub fn channel_to_u16(c: f32) -> u16 {
    (c.clamp(0.0, 1.0) * 65535.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex6_opaque() {
        let c = to_srgb(&Color("#ff0000".into()));
        assert!((c[0] - 1.0).abs() < 1e-3);
        assert!(c[1].abs() < 1e-3 && c[2].abs() < 1e-3);
        assert!((c[3] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn hex8_alpha_suffix_matches_welcome_falloff() {
        // welcome/planet.rs synthesizes soft edges as `"{color}00"`
        // (fully transparent) and `"{color}cc"` (0.8 alpha). Both must
        // parse, and the alpha must land where the falloff expects.
        let edge = to_srgb(&Color("#c8958000".into()));
        assert!(edge[3].abs() < 1e-3, "…00 suffix must be transparent");
        let mid = to_srgb(&Color("#c89580cc".into()));
        assert!((mid[3] - 0.8).abs() < 0.01, "…cc suffix ≈ 0.8 alpha");
    }

    #[test]
    fn rgba_functional() {
        // welcome/sun_glare.rs uses `"rgba(255, 210, 110, 0.70)"`.
        let c = to_srgb(&Color("rgba(255, 210, 110, 0.70)".into()));
        assert!((c[0] - 1.0).abs() < 1e-3);
        assert!((c[1] - 210.0 / 255.0).abs() < 1e-3);
        // Alpha round-trips through a u8 byte (0.70 → 179/255 ≈ 0.702),
        // so the tolerance must exceed the ~1/510 quantization step.
        assert!((c[3] - 0.70).abs() < 0.01);
    }

    #[test]
    fn unparseable_is_transparent_not_loud() {
        let c = to_srgb(&Color("not-a-color".into()));
        assert_eq!(c, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn channel_u16_endpoints() {
        assert_eq!(channel_to_u16(0.0), 0);
        assert_eq!(channel_to_u16(1.0), 65535);
        assert_eq!(channel_to_u16(0.5), 32768); // round-half-up of 32767.5
    }
}
