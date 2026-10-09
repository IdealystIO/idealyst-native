//! Colours as authors write them: sRGB-encoded, straight alpha, `0.0..=1.0`.

/// An sRGB colour with straight alpha — the same space as CSS and the 2D
/// canvas. Renderers convert to linear for lighting ([`to_linear`]).
///
/// [`to_linear`]: Color::to_linear
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    pub const GRAY: Color = Color::rgb(0.5, 0.5, 0.5);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// From 8-bit sRGB channels.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color::rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    /// The same colour with alpha `a`.
    pub const fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Linear-light RGB (sRGB transfer removed) with alpha unchanged — what
    /// lighting math operates on.
    pub fn to_linear(self) -> [f32; 4] {
        [srgb_to_linear(self.r), srgb_to_linear(self.g), srgb_to_linear(self.b), self.a]
    }
}

/// The sRGB electro-optical transfer function (IEC 61966-2-1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`srgb_to_linear`].
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_round_trips_and_hits_the_reference_points() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // sRGB 0.5 is ~21.4% linear light.
        assert!((srgb_to_linear(0.5) - 0.2140).abs() < 1e-3);
        for i in 0..=20 {
            let c = i as f32 / 20.0;
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-5, "{c}");
        }
    }
}
