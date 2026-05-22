//! Cross-Apple color parsing. The CSS-style string → sRGB float
//! tuple step is pure (lives in `framework_core::color`); this
//! wrapper just coerces to `CGFloat` so the result drops straight
//! into `UIColor::colorWithRed:...` / `NSColor::colorWithRed:...`
//! without per-call casting at every caller.
//!
//! UIColor / NSColor construction itself stays in the leaf crates
//! because it depends on the UI toolkit; this module only owns the
//! shared parsing shim.

use objc2_foundation::CGFloat;

/// Parse a CSS-style color string into `(r, g, b, a)` in `0.0..=1.0`,
/// coerced to `CGFloat`. Unknown shapes fall back to opaque black
/// (matches the legacy iOS behavior before centralization).
///
/// The parsing logic lives in
/// [`framework_core::color::parse_or`]; this wrapper exists so the
/// leaf crate's `color_to_uicolor` / `color_to_nscolor` adapter
/// stays a one-liner.
pub fn parse_color(s: &str) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let [r, g, b, a] = framework_core::color::parse_or(s, framework_core::color::Rgba::BLACK)
        .to_srgb_f32();
    (r as CGFloat, g as CGFloat, b as CGFloat, a as CGFloat)
}
