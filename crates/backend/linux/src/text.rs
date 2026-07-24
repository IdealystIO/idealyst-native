//! Text styling for `text` nodes (real `GtkLabel`s).
//!
//! Text stays a genuine native `GtkLabel` — it gets platform text
//! rendering, selection, and accessibility for free. Style is applied
//! via a Pango [`AttrList`](gtk4::pango::AttrList) rather than a CSS
//! provider so per-frame color animation (welcome's Act 2 headline
//! color tween) is a cheap attribute rebuild, not a CSS reparse.
//!
//! [`TextPaint`] is the resolved, cached style the backend keeps on a
//! text node so `set_animated_color(ForegroundColor)` can swap just the
//! color and rebuild the `AttrList` without re-reading `StyleRules`.

use gtk4::pango;
use gtk4::prelude::WidgetExt;
use runtime_core::{FontFamily, FontStyle, FontWeight, Length, StyleRules, TextAlign};
use runtime_layout::{AvailableSpace, Size};

use crate::color;

/// Resolved text style, cached per text node.
#[derive(Clone, Debug)]
pub struct TextPaint {
    pub family: Option<String>,
    pub size_px: f32,
    pub weight: FontWeight,
    pub italic: bool,
    /// Author letter-spacing in px (may be negative).
    pub letter_spacing_px: f32,
    pub color: [f32; 4],
    pub align: TextAlign,
}

impl Default for TextPaint {
    fn default() -> Self {
        Self {
            family: None,
            size_px: 16.0,
            weight: FontWeight::Normal,
            italic: false,
            letter_spacing_px: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            align: TextAlign::Left,
        }
    }
}

fn family_name(f: &FontFamily) -> Option<String> {
    match f {
        FontFamily::System(name) => Some(name.clone()),
        FontFamily::Typeface(t) => Some(t.family_name.to_string()),
    }
}

fn map_weight(w: FontWeight) -> pango::Weight {
    match w {
        FontWeight::Thin => pango::Weight::Thin,
        FontWeight::ExtraLight => pango::Weight::Ultralight,
        FontWeight::Light => pango::Weight::Light,
        FontWeight::Normal => pango::Weight::Normal,
        FontWeight::Medium => pango::Weight::Medium,
        FontWeight::SemiBold => pango::Weight::Semibold,
        FontWeight::Bold => pango::Weight::Bold,
        FontWeight::ExtraBold => pango::Weight::Ultrabold,
        FontWeight::Black => pango::Weight::Heavy,
    }
}

/// Read the text-relevant fields out of a node's [`StyleRules`],
/// layering them over `prev` so a partial restyle (a stylesheet that
/// only sets `color`) keeps the previously-resolved font. `None` for a
/// field leaves the prior value untouched.
pub fn resolve(style: &StyleRules, prev: &TextPaint) -> TextPaint {
    let mut tp = prev.clone();
    if let Some(fam) = &style.font_family {
        tp.family = family_name(fam);
    }
    if let Some(size) = &style.font_size {
        if let Length::Px(v) = size.resolve() {
            tp.size_px = v;
        }
    }
    if let Some(w) = style.font_weight {
        tp.weight = w;
    }
    if let Some(s) = style.font_style {
        tp.italic = matches!(s, FontStyle::Italic);
    }
    if let Some(ls) = &style.letter_spacing {
        tp.letter_spacing_px = ls.resolve();
    }
    if let Some(c) = &style.color {
        tp.color = color::to_srgb(&c.resolve());
    }
    if let Some(a) = style.text_align {
        tp.align = a;
    }
    tp
}

fn build_attrs(tp: &TextPaint) -> pango::AttrList {
    let attrs = pango::AttrList::new();

    let mut fd = pango::FontDescription::new();
    if let Some(fam) = &tp.family {
        fd.set_family(fam);
    }
    fd.set_weight(map_weight(tp.weight));
    if tp.italic {
        fd.set_style(pango::Style::Italic);
    }
    // Absolute size: px × PANGO_SCALE (device units, DPI-independent).
    fd.set_absolute_size(tp.size_px as f64 * pango::SCALE as f64);
    attrs.insert(pango::AttrFontDesc::new(&fd));

    if tp.letter_spacing_px != 0.0 {
        attrs.insert(pango::AttrInt::new_letter_spacing(
            (tp.letter_spacing_px * pango::SCALE as f32).round() as i32,
        ));
    }

    // Foreground color + alpha (Pango takes 16-bit-per-channel).
    let [r, g, b, a] = tp.color;
    attrs.insert(pango::AttrColor::new_foreground(
        color::channel_to_u16(r),
        color::channel_to_u16(g),
        color::channel_to_u16(b),
    ));
    attrs.insert(pango::AttrInt::new_foreground_alpha(color::channel_to_u16(a)));

    attrs
}

/// Taffy measure function for a `GtkLabel`: report the label's intrinsic
/// size (from its current Pango attributes) under Taffy's width
/// constraint. Registered per text node so flex layout can size + center
/// text (welcome's centered headline / subtitle). Reads the label live,
/// so it reflects whatever font `apply_style` set before the layout pass.
pub fn measure(
    label: &gtk4::Label,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
) -> Size<f32> {
    // Minimum (longest unbreakable word — the label's min-content width)
    // and natural (whole text on one line) widths, both unconstrained.
    let (wmin, wnat, _, _) = label.measure(gtk4::Orientation::Horizontal, -1);
    let width = known.width.unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(aw) => (wnat as f32).min(aw),
        _ => wnat as f32,
    });
    // Height at that width — but height MUST be measured at a width the
    // label can actually take. A `GtkLabel` cannot render narrower than
    // its longest word, and GTK loudly refuses to try:
    //   "Trying to measure GtkLabel for width of 0, but it needs at
    //    least 81"
    // Taffy legitimately probes with a 0/near-0 available width while
    // resolving flex minimums, so this fired constantly and flooded the
    // log. Clamping to `wmin` asks the question GTK can answer (the
    // height when wrapped as tight as possible) instead of an impossible
    // one; `-1` for a non-positive result keeps "unconstrained" meaning
    // unconstrained rather than "zero".
    let for_size = if width >= 1.0 {
        (width.round() as i32).max(wmin)
    } else {
        -1
    };
    let (_hmin, hnat, _, _) = label.measure(gtk4::Orientation::Vertical, for_size);
    Size {
        width,
        height: known.height.unwrap_or(hnat as f32),
    }
}

/// Apply the resolved paint to a `GtkLabel`: attributes (font, color,
/// spacing) + alignment.
pub fn apply(label: &gtk4::Label, tp: &TextPaint) {
    label.set_attributes(Some(&build_attrs(tp)));
    let (xalign, justify) = match tp.align {
        TextAlign::Left => (0.0, gtk4::Justification::Left),
        TextAlign::Right => (1.0, gtk4::Justification::Right),
        TextAlign::Center => (0.5, gtk4::Justification::Center),
        TextAlign::Justify => (0.0, gtk4::Justification::Fill),
    };
    label.set_xalign(xalign);
    label.set_justify(justify);
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{Color, Tokenized};

    #[test]
    fn resolve_layers_over_prev_keeping_font_when_only_color_changes() {
        let base = TextPaint {
            family: Some("Inter".into()),
            size_px: 56.0,
            weight: FontWeight::Bold,
            ..Default::default()
        };
        let mut style = StyleRules::default();
        style.color = Some(Tokenized::Literal(Color("#ff0000".into())));
        let out = resolve(&style, &base);
        // Color changed…
        assert!((out.color[0] - 1.0).abs() < 1e-3);
        // …font preserved from prev.
        assert_eq!(out.family.as_deref(), Some("Inter"));
        assert_eq!(out.size_px, 56.0);
        assert!(matches!(out.weight, FontWeight::Bold));
    }

    #[test]
    fn resolve_reads_size_and_weight() {
        let mut style = StyleRules::default();
        style.font_size = Some(Tokenized::Literal(Length::Px(18.0)));
        style.font_weight = Some(FontWeight::SemiBold);
        style.letter_spacing = Some(Tokenized::Literal(0.6));
        let out = resolve(&style, &TextPaint::default());
        assert_eq!(out.size_px, 18.0);
        assert!(matches!(out.weight, FontWeight::SemiBold));
        assert!((out.letter_spacing_px - 0.6).abs() < 1e-4);
    }

    #[test]
    fn weight_maps_black_to_heavy() {
        assert_eq!(map_weight(FontWeight::Black), pango::Weight::Heavy);
        assert_eq!(map_weight(FontWeight::ExtraLight), pango::Weight::Ultralight);
    }
}
