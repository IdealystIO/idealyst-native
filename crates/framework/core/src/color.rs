//! Canonical color parsing and conversions.
//!
//! Authors write colors as strings inside [`Color`](crate::Color)
//! (`"#1a2b3c"`, `"rgba(255, 0, 0, 0.5)"`, `"transparent"`, …). Each
//! backend ultimately needs the parsed bytes in a different shape —
//! sRGB floats for the web's animation path, a `UIColor` on iOS, an
//! `0xAARRGGBB` int on Android, linear floats for the wgpu renderer.
//! This module owns the parse and the byte intermediate; backends
//! convert from [`Rgba`] to their native form via the small per-backend
//! wrappers.
//!
//! Centralizing this fixes the drift from six near-identical parsers:
//! - Some backends supported `transparent`, others didn't.
//! - Some supported `#rgba` (4-char hex), others didn't.
//! - The Android icon path interpreted `#rrggbbaa` as `aarrggbb`,
//!   producing dark squares where the rest of the framework rendered
//!   the expected CSS alpha.
//!
//! Not supported by design (callers can layer atop): named CSS colors
//! ("red", "white"), `hsl()` / `hsv()`. No real users; adding either
//! is additive when one shows up.

use core::fmt;

/// Canonical RGBA byte intermediate. Channel order matches CSS:
/// red, green, blue, then alpha. Repacking to platform-native forms
/// (Android's `0xAARRGGBB` int, etc.) goes through the methods below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba { r: 0, g: 0, b: 0, a: 255 };
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Convert to sRGB `[r, g, b, a]` floats in `0..=1`. The
    /// representation most backends use internally for animation
    /// state and gradient stop caches.
    pub fn to_srgb_f32(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    /// Build from sRGB `[r, g, b, a]` floats in `0..=1`. Out-of-range
    /// values are clamped before quantization.
    pub fn from_srgb_f32(c: [f32; 4]) -> Self {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self {
            r: q(c[0]),
            g: q(c[1]),
            b: q(c[2]),
            a: q(c[3]),
        }
    }

    /// Pack as `0xAARRGGBB` — Android's `android.graphics.Color`
    /// representation. The high byte is alpha, then R, G, B.
    pub fn to_argb_u32(self) -> u32 {
        ((self.a as u32) << 24)
            | ((self.r as u32) << 16)
            | ((self.g as u32) << 8)
            | (self.b as u32)
    }

    /// Unpack from `0xAARRGGBB`.
    pub fn from_argb_u32(v: u32) -> Self {
        Self {
            a: ((v >> 24) & 0xff) as u8,
            r: ((v >> 16) & 0xff) as u8,
            g: ((v >> 8) & 0xff) as u8,
            b: (v & 0xff) as u8,
        }
    }
}

/// Why a string failed to parse. Backends typically don't branch on
/// the variant — they swap a fallback color in — but the variant is
/// here for diagnostic logging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorParseError {
    /// String was empty after trimming.
    Empty,
    /// `rgb()` / `rgba()` shape had wrong component count or
    /// unparseable numbers.
    InvalidComponents,
    /// `#hex` had a non-hex digit, or its length wasn't 3, 4, 6, or 8.
    InvalidHex,
    /// String didn't match any supported shape — likely a CSS named
    /// color, an `hsl(...)`, or a malformed value.
    UnknownFormat,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColorParseError::Empty => f.write_str("empty color string"),
            ColorParseError::InvalidComponents => {
                f.write_str("rgb()/rgba() components invalid")
            }
            ColorParseError::InvalidHex => f.write_str("hex color invalid"),
            ColorParseError::UnknownFormat => f.write_str("unknown color format"),
        }
    }
}

/// Parse a CSS-style color string into canonical [`Rgba`] bytes.
///
/// Accepts:
/// - `transparent` (case-insensitive) → fully transparent black
/// - `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` hex (CSS byte order —
///   alpha last; for 4/8-digit forms)
/// - `rgb(r, g, b)` with channels `0..=255`
/// - `rgba(r, g, b, a)` with channels `0..=255` and alpha as a
///   `0..=1` float (lenient: a value `> 1.0` is treated as a
///   `0..=255` byte, so `rgba(255, 0, 0, 255)` means fully opaque)
///
/// Out-of-range numeric values are clamped, not rejected.
pub fn parse(input: &str) -> Result<Rgba, ColorParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ColorParseError::Empty);
    }
    if s.eq_ignore_ascii_case("transparent") {
        return Ok(Rgba::TRANSPARENT);
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    // Match the function name case-insensitively; numeric body
    // is parsed as-is below.
    let lower_head: String = s.chars().take(5).collect::<String>().to_ascii_lowercase();
    if lower_head.starts_with("rgba(") {
        return parse_rgb_body(s, true);
    }
    if lower_head.starts_with("rgb(") {
        return parse_rgb_body(s, false);
    }
    Err(ColorParseError::UnknownFormat)
}

/// Soft variant: parse, returning `fallback` on any error. Backends
/// that want a specific "missing color" visual (opaque black, debug
/// magenta) call this instead of branching on the Result.
pub fn parse_or(input: &str, fallback: Rgba) -> Rgba {
    parse(input).unwrap_or(fallback)
}

fn parse_hex(hex: &str) -> Result<Rgba, ColorParseError> {
    let bytes = hex.as_bytes();
    // CSS spec: 3-digit `#rgb`, 4-digit `#rgba` (alpha last), 6-digit
    // `#rrggbb`, 8-digit `#rrggbbaa` (alpha last). Android's native
    // `0xAARRGGBB` int is reachable via `Rgba::to_argb_u32()`; the
    // parser does NOT support the legacy `#aarrggbb` text form — it
    // would be ambiguous with the CSS form and produces the dark-
    // square-on-fade bug the previous Android code went out of its
    // way to work around.
    let (r, g, b, a) = match bytes.len() {
        3 => (
            expand_nibble(hex_digit(bytes[0])?),
            expand_nibble(hex_digit(bytes[1])?),
            expand_nibble(hex_digit(bytes[2])?),
            0xff,
        ),
        4 => (
            expand_nibble(hex_digit(bytes[0])?),
            expand_nibble(hex_digit(bytes[1])?),
            expand_nibble(hex_digit(bytes[2])?),
            expand_nibble(hex_digit(bytes[3])?),
        ),
        6 => (
            hex_byte(&bytes[0..2])?,
            hex_byte(&bytes[2..4])?,
            hex_byte(&bytes[4..6])?,
            0xff,
        ),
        8 => (
            hex_byte(&bytes[0..2])?,
            hex_byte(&bytes[2..4])?,
            hex_byte(&bytes[4..6])?,
            hex_byte(&bytes[6..8])?,
        ),
        _ => return Err(ColorParseError::InvalidHex),
    };
    Ok(Rgba { r, g, b, a })
}

fn hex_byte(pair: &[u8]) -> Result<u8, ColorParseError> {
    Ok(hex_digit(pair[0])? * 16 + hex_digit(pair[1])?)
}

fn hex_digit(c: u8) -> Result<u8, ColorParseError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(10 + c - b'a'),
        b'A'..=b'F' => Ok(10 + c - b'A'),
        _ => Err(ColorParseError::InvalidHex),
    }
}

fn expand_nibble(v: u8) -> u8 {
    // `f` → `ff`, `3` → `33` — the standard CSS short-form expansion.
    (v << 4) | v
}

fn parse_rgb_body(s: &str, has_alpha: bool) -> Result<Rgba, ColorParseError> {
    // Slice between the first `(` and the trailing `)`. We've already
    // matched on the function name, so the open paren is guaranteed.
    let open = s.find('(').ok_or(ColorParseError::InvalidComponents)?;
    let inner = s[open + 1..]
        .strip_suffix(')')
        .ok_or(ColorParseError::InvalidComponents)?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let need = if has_alpha { 4 } else { 3 };
    if parts.len() < need {
        return Err(ColorParseError::InvalidComponents);
    }
    let r = parts[0].parse::<f32>().map_err(|_| ColorParseError::InvalidComponents)?;
    let g = parts[1].parse::<f32>().map_err(|_| ColorParseError::InvalidComponents)?;
    let b = parts[2].parse::<f32>().map_err(|_| ColorParseError::InvalidComponents)?;
    let a_raw = if has_alpha {
        parts[3].parse::<f32>().map_err(|_| ColorParseError::InvalidComponents)?
    } else {
        1.0
    };
    // Lenient alpha: `rgba(r, g, b, 255)` is in the wild, even though
    // CSS spec is `0..=1`. Treat anything `> 1.0` as a `0..=255` byte.
    let a_byte = if a_raw > 1.0 {
        clamp_byte(a_raw)
    } else {
        (a_raw.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Ok(Rgba {
        r: clamp_byte(r),
        g: clamp_byte(g),
        b: clamp_byte(b),
        a: a_byte,
    })
}

fn clamp_byte(v: f32) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

/// sRGB → linear conversion for a single 0..=1 channel. The wgpu
/// surface format we select is sRGB-encoded, so the hardware
/// gamma-encodes whatever the fragment shader outputs. To get the
/// CSS-style sRGB color the author wrote, we ship a linear value
/// that the hardware re-encodes back to sRGB on write. Alpha is not
/// gamma-encoded — only RGB.
pub fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Apply [`srgb_channel_to_linear`] to RGB, leave alpha untouched.
pub fn srgb_rgba_to_linear(c: [f32; 4]) -> [f32; 4] {
    [
        srgb_channel_to_linear(c[0]),
        srgb_channel_to_linear(c[1]),
        srgb_channel_to_linear(c[2]),
        c[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba { r, g, b, a }
    }

    // ---- hex ----

    #[test]
    fn hex_3() {
        assert_eq!(parse("#abc").unwrap(), rgba(0xaa, 0xbb, 0xcc, 0xff));
        assert_eq!(parse("#000").unwrap(), rgba(0, 0, 0, 0xff));
        assert_eq!(parse("#fff").unwrap(), rgba(0xff, 0xff, 0xff, 0xff));
    }

    #[test]
    fn hex_4_with_alpha() {
        // `#rgba` — alpha last, expanded same as RGB.
        assert_eq!(parse("#abcd").unwrap(), rgba(0xaa, 0xbb, 0xcc, 0xdd));
        assert_eq!(parse("#0000").unwrap(), rgba(0, 0, 0, 0));
    }

    #[test]
    fn hex_6() {
        assert_eq!(parse("#1a2b3c").unwrap(), rgba(0x1a, 0x2b, 0x3c, 0xff));
        assert_eq!(parse("#FF0000").unwrap(), rgba(0xff, 0, 0, 0xff));
    }

    #[test]
    fn hex_8_css_order() {
        // CSS spec: `#rrggbbaa`. The legacy `#aarrggbb` interpretation
        // would yield (0xc8, 0x95, 0x80, 0x00) — verifying we DON'T
        // do that prevents the dark-square regression.
        assert_eq!(parse("#c8958000").unwrap(), rgba(0xc8, 0x95, 0x80, 0x00));
        assert_eq!(parse("#11223344").unwrap(), rgba(0x11, 0x22, 0x33, 0x44));
    }

    #[test]
    fn hex_case_insensitive() {
        assert_eq!(parse("#AbCdEf").unwrap(), rgba(0xab, 0xcd, 0xef, 0xff));
    }

    #[test]
    fn hex_invalid_length() {
        assert_eq!(parse("#12345"), Err(ColorParseError::InvalidHex));
        assert_eq!(parse("#1234567"), Err(ColorParseError::InvalidHex));
        assert_eq!(parse("#"), Err(ColorParseError::InvalidHex));
    }

    #[test]
    fn hex_invalid_digit() {
        assert_eq!(parse("#xyz"), Err(ColorParseError::InvalidHex));
        assert_eq!(parse("#12345g"), Err(ColorParseError::InvalidHex));
    }

    // ---- rgb / rgba ----

    #[test]
    fn rgb_basic() {
        assert_eq!(parse("rgb(255, 0, 128)").unwrap(), rgba(255, 0, 128, 255));
        assert_eq!(parse("RGB(0,0,0)").unwrap(), rgba(0, 0, 0, 255));
    }

    #[test]
    fn rgba_alpha_0_to_1() {
        let v = parse("rgba(255, 0, 0, 0.5)").unwrap();
        assert_eq!(v.r, 255);
        assert_eq!(v.g, 0);
        assert_eq!(v.b, 0);
        // 0.5 * 255 → 127.5 → rounds to 128 (banker's? no — half-away-from-zero).
        assert_eq!(v.a, 128);
    }

    #[test]
    fn rgba_lenient_alpha_as_byte() {
        // Authors in the wild write `rgba(r, g, b, 255)`. Be lenient.
        assert_eq!(parse("rgba(10, 20, 30, 255)").unwrap(), rgba(10, 20, 30, 255));
    }

    #[test]
    fn rgba_zero_alpha() {
        assert_eq!(parse("rgba(255, 255, 255, 0)").unwrap(), rgba(255, 255, 255, 0));
        // CSS-style transparent-with-color: gradient stops use this
        // shape (`rgba(r, g, b, 0)`) to fade to invisible while
        // preserving the color band.
        assert_eq!(parse("rgba(0, 122, 255, 0)").unwrap(), rgba(0, 122, 255, 0));
    }

    #[test]
    fn rgb_with_spaces() {
        assert_eq!(parse("rgb( 10 , 20 , 30 )").unwrap(), rgba(10, 20, 30, 255));
    }

    #[test]
    fn rgb_out_of_range_clamped() {
        assert_eq!(parse("rgb(300, -50, 128)").unwrap(), rgba(255, 0, 128, 255));
    }

    #[test]
    fn rgb_invalid_count() {
        assert_eq!(
            parse("rgb(255, 0)"),
            Err(ColorParseError::InvalidComponents),
        );
    }

    #[test]
    fn rgb_invalid_number() {
        assert_eq!(
            parse("rgb(255, abc, 0)"),
            Err(ColorParseError::InvalidComponents),
        );
    }

    // ---- transparent / unknown / empty ----

    #[test]
    fn transparent_keyword() {
        assert_eq!(parse("transparent").unwrap(), Rgba::TRANSPARENT);
        assert_eq!(parse("TRANSPARENT").unwrap(), Rgba::TRANSPARENT);
        assert_eq!(parse("  Transparent  ").unwrap(), Rgba::TRANSPARENT);
    }

    #[test]
    fn unknown_format() {
        assert_eq!(parse("red"), Err(ColorParseError::UnknownFormat));
        assert_eq!(parse("hsl(120, 50%, 50%)"), Err(ColorParseError::UnknownFormat));
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse(""), Err(ColorParseError::Empty));
        assert_eq!(parse("   "), Err(ColorParseError::Empty));
    }

    #[test]
    fn parse_or_uses_fallback() {
        assert_eq!(parse_or("not a color", Rgba::BLACK), Rgba::BLACK);
        assert_eq!(parse_or("#ff0000", Rgba::BLACK), rgba(255, 0, 0, 255));
    }

    // ---- byte intermediate conversions ----

    #[test]
    fn srgb_f32_round_trip() {
        let cases = [
            rgba(0, 0, 0, 0),
            rgba(255, 255, 255, 255),
            rgba(128, 64, 192, 50),
            rgba(1, 2, 3, 4),
        ];
        for c in cases {
            let back = Rgba::from_srgb_f32(c.to_srgb_f32());
            assert_eq!(back, c, "round-trip {:?}", c);
        }
    }

    #[test]
    fn argb_u32_round_trip() {
        let c = rgba(0x1a, 0x2b, 0x3c, 0x4d);
        let packed = c.to_argb_u32();
        assert_eq!(packed, 0x4d1a2b3c);
        assert_eq!(Rgba::from_argb_u32(packed), c);
    }

    #[test]
    fn from_srgb_f32_clamps() {
        let c = Rgba::from_srgb_f32([-0.5, 0.5, 1.5, 2.0]);
        assert_eq!(c, rgba(0, 128, 255, 255));
    }

    // ---- sRGB → linear ----

    #[test]
    fn srgb_linear_anchors() {
        assert_eq!(srgb_channel_to_linear(0.0), 0.0);
        // 1.0 maps to ~1.0 (within float precision).
        assert!((srgb_channel_to_linear(1.0) - 1.0).abs() < 1e-5);
        // The piecewise breakpoint at 0.04045 → 0.04045 / 12.92.
        assert!((srgb_channel_to_linear(0.04045) - 0.04045 / 12.92).abs() < 1e-7);
    }

    #[test]
    fn srgb_linear_preserves_alpha() {
        let [_, _, _, a] = srgb_rgba_to_linear([0.5, 0.5, 0.5, 0.75]);
        assert_eq!(a, 0.75);
    }
}
