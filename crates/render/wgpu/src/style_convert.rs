//! Pre-resolve `StyleRules` into render-friendly values.
//!
//! `StyleRules` is shaped for the framework's needs — `Tokenized<T>`,
//! `Length` enums, `Color` as a string. The renderer wants concrete
//! f32 px sizes and `[f32; 4]` RGBA. We cache that projection on each
//! node so the per-frame walk is cheap (just read fields).

use framework_core::{Color, Length, StyleRules, Tokenized};

/// Render-time projection of a node's style. Default = "no painted
/// background, no border, fully opaque, no rounding."
#[derive(Clone, Debug)]
pub struct RenderStyle {
    pub background: Option<[f32; 4]>,
    pub color: [f32; 4], // text color; default is black

    /// Per-corner radius in px. `[tl, tr, br, bl]`.
    pub corner_radius: [f32; 4],
    /// Per-side border width in px. `[top, right, bottom, left]`.
    pub border_width: [f32; 4],
    /// Per-side border color. Defaults to transparent if unset.
    pub border_color: [[f32; 4]; 4],

    pub font_size: f32,
    pub opacity: f32,

    /// Resolved drop shadow, if the author set `shadow: ...` on the
    /// node. The renderer emits a shadow rect instance underneath
    /// the node's main rect via the `shadow_blur > 0` path on the
    /// rounded-rect pipeline. `offset` is `(x, y)`; `blur` controls
    /// the falloff width; `color` is the shadow's RGBA in sRGB.
    pub shadow: Option<ResolvedShadow>,
}

/// Backend-resolved counterpart of `framework_core::Shadow` —
/// hex strings parsed to RGBA, no `Tokenized` indirection so the
/// renderer can read it on the hot path.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ResolvedShadow {
    pub offset: [f32; 2],
    pub blur: f32,
    pub color: [f32; 4],
}

impl Default for RenderStyle {
    fn default() -> Self {
        Self {
            background: None,
            color: [0.0, 0.0, 0.0, 1.0],
            corner_radius: [0.0; 4],
            border_width: [0.0; 4],
            border_color: [[0.0, 0.0, 0.0, 0.0]; 4],
            font_size: 14.0,
            opacity: 1.0,
            shadow: None,
        }
    }
}

impl RenderStyle {
    /// Project from the framework's `StyleRules`. Properties that
    /// the rules leave unset keep their previous render value — call
    /// sites should start from the existing `RenderStyle`, not from
    /// `default()`, so a state overlay setting only `background`
    /// preserves the base's borders and font size.
    pub fn apply(&mut self, rules: &StyleRules) {
        // `.resolve()` subscribes the enclosing apply-style Effect to
        // the per-token signal for each referenced token. Token swaps
        // re-fire only nodes that touched the changed token.
        if let Some(bg) = rules.background.as_ref() {
            self.background = Some(parse_color(&bg.resolve()));
        }
        if let Some(c) = rules.color.as_ref() {
            self.color = parse_color(&c.resolve());
        }
        if let Some(fs) = rules.font_size.as_ref() {
            if let Length::Px(px) = fs.resolve() {
                self.font_size = px;
            }
        }
        if let Some(o) = rules.opacity.as_ref() {
            self.opacity = o.resolve();
        }

        // Border radius: per-corner. Percent is interpreted at draw
        // time against the rect's min(width, height) — but the MVP
        // shader only handles px, so we collapse percent to 0 for
        // now and revisit when we add percent support.
        self.corner_radius[0] = px(rules.border_top_left_radius.as_ref());
        self.corner_radius[1] = px(rules.border_top_right_radius.as_ref());
        self.corner_radius[2] = px(rules.border_bottom_right_radius.as_ref());
        self.corner_radius[3] = px(rules.border_bottom_left_radius.as_ref());

        // Border widths.
        self.border_width[0] = rules.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[0]);
        self.border_width[1] = rules.border_right_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[1]);
        self.border_width[2] = rules.border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[2]);
        self.border_width[3] = rules.border_left_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[3]);

        if let Some(c) = rules.border_top_color.as_ref() {
            self.border_color[0] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_right_color.as_ref() {
            self.border_color[1] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_bottom_color.as_ref() {
            self.border_color[2] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_left_color.as_ref() {
            self.border_color[3] = parse_color(&c.resolve());
        }

        // Drop shadow — author sets `Shadow { x, y, blur, color }`
        // on the rules; we project to RGBA + concrete f32s so the
        // renderer can stage a shadow rect instance without
        // touching the framework's `Tokenized` types on the hot
        // path. Absence collapses to `None`; once set, fields
        // without an explicit per-frame update keep their resolved
        // values (same merge-into-self pattern the rest of this
        // function uses).
        if let Some(sh) = rules.shadow.as_ref() {
            self.shadow = Some(ResolvedShadow {
                offset: [sh.x, sh.y],
                blur: sh.blur,
                color: parse_color(&sh.color),
            });
        }
    }
}

fn px(t: Option<&Tokenized<Length>>) -> f32 {
    match t.map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

/// Best-effort CSS color parse. Accepts `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)`. Unknown strings → opaque magenta as
/// a visible "you forgot to set a color" signal.
pub fn parse_color(c: &Color) -> [f32; 4] {
    let s = c.0.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgba_components(inner, true);
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        return parse_rgba_components(inner, false);
    }
    // Visible fallback so missing-color bugs are obvious.
    [1.0, 0.0, 1.0, 1.0]
}

fn parse_hex(hex: &str) -> [f32; 4] {
    let bytes = hex.as_bytes();
    let (r, g, b, a) = match bytes.len() {
        3 => (
            dup(bytes[0]),
            dup(bytes[1]),
            dup(bytes[2]),
            0xff,
        ),
        4 => (
            dup(bytes[0]),
            dup(bytes[1]),
            dup(bytes[2]),
            dup(bytes[3]),
        ),
        6 => (
            byte(&bytes[0..2]),
            byte(&bytes[2..4]),
            byte(&bytes[4..6]),
            0xff,
        ),
        8 => (
            byte(&bytes[0..2]),
            byte(&bytes[2..4]),
            byte(&bytes[4..6]),
            byte(&bytes[6..8]),
        ),
        _ => return [1.0, 0.0, 1.0, 1.0],
    };
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
}

fn dup(c: u8) -> u8 {
    let v = hex_digit(c);
    v * 16 + v
}

fn byte(pair: &[u8]) -> u8 {
    hex_digit(pair[0]) * 16 + hex_digit(pair[1])
}

fn hex_digit(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => 10 + c - b'a',
        b'A'..=b'F' => 10 + c - b'A',
        _ => 0,
    }
}

/// sRGB → linear conversion for a single 0..1 channel. The wgpu
/// surface format we select is sRGB-encoded, so the hardware
/// gamma-encodes whatever the fragment shader outputs. To get the
/// CSS-style sRGB color the author wrote, we need to ship a linear
/// value that the hardware will *then* re-encode back to sRGB on
/// write. Alpha is not gamma-encoded — only RGB needs conversion.
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

fn parse_rgba_components(inner: &str, has_alpha: bool) -> [f32; 4] {
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let need = if has_alpha { 4 } else { 3 };
    if parts.len() < need {
        return [1.0, 0.0, 1.0, 1.0];
    }
    let r = parts[0].parse::<f32>().unwrap_or(0.0) / 255.0;
    let g = parts[1].parse::<f32>().unwrap_or(0.0) / 255.0;
    let b = parts[2].parse::<f32>().unwrap_or(0.0) / 255.0;
    let a = if has_alpha { parts[3].parse::<f32>().unwrap_or(1.0) } else { 1.0 };
    [r, g, b, a]
}
