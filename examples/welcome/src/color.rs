//! `(r, g, b, a)` tuple helpers. Framework color AVs consume sRGB
//! tuples with all four channels in `0..=1`.

/// CSS-style `0..=255` channels → `(r, g, b, a)`. Alpha stays in
/// `0..=1`, matching the gradient-stop convention.
pub fn srgba_tuple(r: f32, g: f32, b: f32, a: f32) -> (f32, f32, f32, f32) {
    (r / 255.0, g / 255.0, b / 255.0, a)
}

/// Linear interpolate two RGBA tuples at `t` in `0..=1`.
pub fn lerp_color(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
    t: f32,
) -> (f32, f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
        a.3 + (b.3 - a.3) * t,
    )
}

/// `#rrggbb` or `#rgb` → `(r, g, b, 1.0)`.
pub fn srgb_tuple(hex: &str) -> (f32, f32, f32, f32) {
    let h = hex.trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0) as f32 / 255.0;
    let (r, g, b) = if h.len() == 6 {
        (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6]))
    } else if h.len() == 3 {
        let ch = |c: char| {
            u8::from_str_radix(&c.to_string().repeat(2), 16).unwrap_or(0) as f32 / 255.0
        };
        let bytes: Vec<char> = h.chars().collect();
        (ch(bytes[0]), ch(bytes[1]), ch(bytes[2]))
    } else {
        (0.0, 0.0, 0.0)
    };
    (r, g, b, 1.0)
}
