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
use gtk4::prelude::{Cast, WidgetExt};
use runtime_shared::{FontFamily, FontStyle, FontWeight, Length, StyleRules, TextAlign};
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
    /// Author `line_height` in px. `None` = use the font's own line spacing.
    ///
    /// Must reach both painting and measurement: dropping it makes every text
    /// box shorter than on web/macOS, and the error accumulates up every
    /// column — invisible except in a geometry diff.
    pub line_height_px: Option<f32>,
    pub color: [f32; 4],
    pub align: TextAlign,
    /// Author `max_lines`, normalised: `None` = no limit (`Some(0)` folds to
    /// `None` in [`resolve`], so `apply` only ever sees a real limit). Lives on
    /// the paint, not just the style, because `apply` re-runs on every
    /// animated-colour frame and on every inherited-font refresh — the limit
    /// has to be re-asserted from the same cached value each time.
    pub max_lines: Option<u32>,
}

impl Default for TextPaint {
    fn default() -> Self {
        Self {
            family: None,
            size_px: 16.0,
            weight: FontWeight::Normal,
            italic: false,
            letter_spacing_px: 0.0,
            line_height_px: None,
            color: [0.0, 0.0, 0.0, 1.0],
            align: TextAlign::Left,
            max_lines: None,
        }
    }
}

pub fn family_name(f: &FontFamily) -> Option<String> {
    match f {
        FontFamily::System(name) => Some(name.clone()),
        FontFamily::Typeface(t) => Some(t.family_name.to_string()),
    }
}

pub(crate) fn map_weight(w: FontWeight) -> pango::Weight {
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

/// The inheritable text properties, resolved from the ancestor chain.
///
/// CSS inherits `color` and the font shorthand independently down the tree;
/// native backends have no cascade, so the backend resolves them (see
/// `LinuxBackend::ambient_text`) and hands the result here.
#[derive(Clone, Debug, PartialEq)]
pub struct Inherited {
    pub color: [f32; 4],
    pub family: Option<String>,
    pub size_px: f32,
    pub weight: FontWeight,
    pub italic: bool,
}

impl Default for Inherited {
    fn default() -> Self {
        let d = TextPaint::default();
        Self {
            color: d.color,
            family: d.family,
            size_px: d.size_px,
            weight: d.weight,
            italic: d.italic,
        }
    }
}

/// Read the text-relevant fields out of a node's [`StyleRules`] into a
/// paint, with `inherited` standing in for the CSS cascade.
///
/// Every text field is re-resolved from `style` + `inherited`: a field the
/// style does not set is "not set" (inherit or default), never "keep `prev`'s
/// value" — otherwise every text property becomes one-way, settable but never
/// unsettable.
///
/// The web backend gets inheritance for free: a `<span>` with no `color` picks
/// up its ancestor's, and an icon's `currentColor` resolves the same way. Native
/// backends have no cascade — an unset colour falling back to opaque black is
/// invisible under a dark theme — so the caller resolves the ancestor chain
/// (see `LinuxBackend::ambient_text`) and passes it here.
pub fn resolve(style: &StyleRules, prev: &TextPaint, inherited: &Inherited) -> TextPaint {
    let mut tp = prev.clone();

    // Typography: start from what this node INHERITS, then overlay what it
    // declares. Each property independently, exactly as CSS cascades them.
    //
    // Not a "does the style mention typography" gate over `prev` or
    // `TextPaint::default()`: that discards an inherited size/family for a
    // style setting only `font_weight` (most labels), and makes properties
    // one-way (a TOC entry's Bold never reverts).
    //
    // Inheriting is correct by construction: `apply_style` always
    // receives the node's COMPLETE resolved rules (see
    // `runtime_vocabulary::style_attach`), so a property the rules do not
    // mention is genuinely unset and must come from the ancestor chain — never
    // from this node's own previous state, which is what made it sticky.
    tp.family = style
        .font_family
        .as_ref()
        .and_then(family_name)
        .or_else(|| inherited.family.clone());
    tp.size_px = match style.font_size.as_ref().map(|t| t.resolve()) {
        Some(Length::Px(v)) => v,
        _ => inherited.size_px,
    };
    tp.weight = style.font_weight.unwrap_or(inherited.weight);
    tp.italic = match style.font_style {
        Some(FontStyle::Italic) => true,
        Some(_) => false,
        None => inherited.italic,
    };

    // These are independent, and each reverts to its default when
    // absent, for the same reason: absent means "not set".
    tp.letter_spacing_px = style
        .letter_spacing
        .as_ref()
        .map(|ls| ls.resolve())
        .unwrap_or(0.0);
    // Independent, and absent means "use the font's own spacing" — not "keep
    // whatever the previous style said".
    tp.line_height_px = style
        .line_height
        .as_ref()
        .map(|lh| lh.resolve())
        .filter(|px| *px > 0.0);
    // Own colour wins; otherwise inherit. Absent must NOT mean "keep whatever
    // the previous style said" — a node whose ancestor re-themes has to follow.
    tp.color = match &style.color {
        Some(c) => color::to_srgb(&c.resolve()),
        None => inherited.color,
    };
    tp.align = style.text_align.unwrap_or(TextAlign::Left);
    // Independent like the rest: absent (or 0, the documented "no limit")
    // means unlimited, never "keep the previous limit".
    tp.max_lines = style.max_lines.filter(|n| *n > 0);
    tp
}

/// How a `GtkLabel` is configured to honour a paint's `max_lines`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineLimit {
    /// `GtkLabel:wrap`.
    pub wrap: bool,
    /// `GtkLabel:lines` (`-1` = unlimited).
    pub lines: i32,
    /// `GtkLabel:ellipsize`.
    pub ellipsize: pango::EllipsizeMode,
}

/// The label configuration for a line limit.
///
/// - **No limit** is exactly what `create_text` sets up: wrapping, unlimited
///   lines, no ellipsis. A restyle that drops `max_lines` must land back here,
///   or the limit would be one-way (the same bug class as the sticky
///   `font_weight` above).
/// - **One line** is ellipsize-End WITHOUT wrap. `GtkLabel:lines` (which
///   becomes `pango_layout_set_height(-n)`) only does anything for a wrapping
///   label, and a non-wrapping one never breaks a paragraph, so plain
///   ellipsize is the whole of it — the CSS `white-space: nowrap` +
///   `text-overflow: ellipsis` shape. It also makes GtkLabel's request mode
///   constant-size, so the height measured at any width is one line.
/// - **n > 1 lines** is wrap + `lines = n` + ellipsize-End: Pango wraps, stops
///   after the n-th line and ellipsizes it at the tail.
///
/// Known divergence: Pango's negative layout height is a limit *per
/// paragraph*, so a text containing hard `\n` breaks can show more than n
/// lines in total (each paragraph gets up to n). `GtkLabel` exposes no
/// whole-layout line cap (only `lines`), so this cannot be matched without
/// dropping the native label. Single-paragraph text — the case `max_lines`
/// exists for (names in table cells, previews) — matches web exactly.
pub fn line_limit(max_lines: Option<u32>) -> LineLimit {
    match max_lines.filter(|n| *n > 0) {
        None => LineLimit {
            wrap: true,
            lines: -1,
            ellipsize: pango::EllipsizeMode::None,
        },
        Some(1) => LineLimit {
            wrap: false,
            lines: -1,
            ellipsize: pango::EllipsizeMode::End,
        },
        Some(n) => LineLimit {
            wrap: true,
            lines: i32::try_from(n).unwrap_or(i32::MAX),
            ellipsize: pango::EllipsizeMode::End,
        },
    }
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

    // Author `line_height`, in px, as Pango's ABSOLUTE line height (the
    // factor variant would multiply the font's own spacing and so would not
    // match a CSS `line-height: 20px`, which web resolves literally). Applying
    // it as an attribute — rather than `Layout::set_line_spacing` — keeps the
    // whole paint in one `AttrList`, so a restyle stays a single attribute
    // rebuild and the Taffy measure fn (which reads the label live) picks the
    // new height up with no extra plumbing.
    if let Some(px) = tp.line_height_px {
        attrs.insert(pango::AttrInt::new_line_height_absolute(
            (px * pango::SCALE as f32).round() as i32,
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
/// How far above GTK's reported natural width to look for a width the label can
/// actually render on one line. The discrepancy observed is 1px; the bound keeps
/// a pathological font from turning this into an unbounded scan, and 4px is far
/// below the size of any real layout difference.
const NAT_WIDTH_CORRECTION_MAX_PX: i32 = 4;

/// GTK's reported natural width, corrected to a width its own layout agrees the
/// text fits in on one line.
///
/// # The bug this exists for
///
/// `gtk_widget_measure(Horizontal, -1)` reports the natural width from the
/// UNWRAPPED layout's logical extents, rounded up to whole pixels. The
/// height-for-width path instead calls `pango_layout_set_width(w * PANGO_SCALE)`
/// and re-runs the line breaker. **`letter_spacing` makes the two disagree**: the
/// per-glyph spacing accumulates a fractional total that the width report rounds
/// away and the line breaker does not, so GTK claims a width and then refuses to
/// render in it. Measured on a letter-spaced label (0.3px, the value
/// idea-ui's Tag `label` slot gives every Tag), sweeping font sizes:
///
/// ```text
/// "★ Featured"  short at 10, 13, 16, 22, 23 px
/// "✓ Done"      short at  9, 12, 15, 18, 22 px
/// "→ Next"      short at 10, 15, 18, 23 px
/// ```
///
/// With no letter-spacing, none of those sizes disagree — the spacing is the
/// trigger, not the glyph.
///
/// Trusting the short report sizes e.g. a `Tag(label = "✓ Verified")` 1px too
/// narrow, so the label wraps INSIDE its own box and the glyph renders ABOVE
/// the word while web draws one line.
///
/// The correction asks GTK the question it can answer consistently: the smallest
/// width (from the reported natural width up) whose height-for-width equals the
/// unconstrained height, i.e. the narrowest width that does not force an extra
/// line. Costs two cached `measure` calls in the common case, where the reported
/// width is already correct and the loop exits on its first iteration.
fn honest_natural_width(label: &gtk4::Label, reported: i32) -> i32 {
    if reported <= 0 {
        return reported;
    }
    // The height with no wrapping at all — hard breaks (`\n`) still count.
    let (_, h_unwrapped, _, _) = label.measure(gtk4::Orientation::Vertical, -1);
    for extra in 0..=NAT_WIDTH_CORRECTION_MAX_PX {
        let w = reported + extra;
        let (_, h_at_w, _, _) = label.measure(gtk4::Orientation::Vertical, w);
        if h_at_w <= h_unwrapped {
            return w;
        }
    }
    // Nothing within the bound fit. Report the widest probed rather than the
    // known-short value: an extra pixel of slack is invisible, a wrapped line is
    // not.
    reported + NAT_WIDTH_CORRECTION_MAX_PX
}

pub fn measure(
    label: &gtk4::Label,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
) -> Size<f32> {
    // Padding reaches a leaf as GTK margins (see `apply_style` step 1a),
    // and `gtk_widget_measure` INCLUDES margins in what it reports. Taffy
    // asks for — and adds padding back onto — the CONTENT size, so the
    // margins have to come back off here, or every padded leaf's padding
    // is double-counted (content + 2×padding).
    let (mx, my) = crate::widget_margins(label.upcast_ref());

    // Minimum (longest unbreakable word — the label's min-content width)
    // and natural (whole text on one line) widths, both unconstrained.
    let (wmin, wnat, _, _) = label.measure(gtk4::Orientation::Horizontal, -1);
    let wnat = honest_natural_width(label, wnat);
    let (wmin, wnat) = ((wmin - mx).max(0), (wnat - mx).max(0));
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
    // resolving flex minimums, which would flood the log. Clamping to `wmin` asks the question GTK can answer (the
    // height when wrapped as tight as possible) instead of an impossible
    // one; `-1` for a non-positive result keeps "unconstrained" meaning
    // unconstrained rather than "zero".
    //
    // `for_size` goes back INTO GTK, so it must be margin-inclusive
    // again — the `+ mx` undoes the subtraction above for this one call.
    let for_size = if width >= wnat as f32 {
        // At (or above) the natural width the label is by definition NOT
        // wrapping, so ask for the unconstrained height — the one that honours
        // hard breaks only. Feeding the natural width back in as a constraint
        // re-runs the wrap decision and re-invites the same off-by-one
        // `honest_natural_width` exists to correct.
        -1
    } else if width >= 1.0 {
        (width.round() as i32).max(wmin) + mx
    } else {
        -1
    };
    let (_hmin, hnat, _, _) = label.measure(gtk4::Orientation::Vertical, for_size);
    Size {
        width,
        height: known.height.unwrap_or((hnat - my).max(0) as f32),
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

    // Line limit. All three setters early-return when the value is unchanged,
    // so re-asserting them on every animated-colour frame costs nothing and
    // does not invalidate the layout. The Taffy measure fn (`measure`) reads
    // the label live through `gtk_widget_measure`, whose measuring layout is a
    // copy of the label's own (ellipsize + `lines` → `pango_layout_set_height`
    // included), so the measured height is capped at the same n lines the
    // label draws and the ellipsized label's minimum width drops to the
    // ellipsis — no separate measurement path to keep in sync.
    let limit = line_limit(tp.max_lines);
    label.set_wrap(limit.wrap);
    label.set_lines(limit.lines);
    label.set_ellipsize(limit.ellipsize);
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::{Color, Tokenized};

    // Bug: a TOC bolds its active entry, and every entry stayed bold
    // forever after — dropping `font_weight` must revert the old Bold, not
    // keep the previous paint's value.
    #[test]
    fn regression_unsetting_a_font_prop_reverts_it_to_default() {
        let bolded = TextPaint {
            family: Some("Inter".into()),
            size_px: 20.0,
            weight: FontWeight::Bold,
            italic: true,
            ..Default::default()
        };
        // The "inactive" style still specifies a size (so typography IS
        // being set) but no weight or italic — those must revert.
        let mut style = StyleRules::default();
        style.font_size = Some(Tokenized::Literal(Length::Px(20.0)));
        let out = resolve(&style, &bolded, &Inherited::default());
        assert!(
            matches!(out.weight, FontWeight::Normal),
            "dropping font_weight must un-bold, not keep Bold",
        );
        assert!(!out.italic, "dropping font_style must un-italicize");
        assert_eq!(out.size_px, 20.0);
        assert_eq!(out.family, None, "dropping font_family reverts to system");
    }

    #[test]
    fn regression_unsetting_letter_spacing_and_align_reverts_them() {
        let prev = TextPaint {
            letter_spacing_px: 4.0,
            align: TextAlign::Center,
            ..Default::default()
        };
        let out = resolve(&StyleRules::default(), &prev, &Inherited::default());
        assert_eq!(out.letter_spacing_px, 0.0, "letter spacing must reset");
        assert!(matches!(out.align, TextAlign::Left), "alignment must reset");
    }

    /// A colour-only restyle must not wipe the font — the font comes from the
    /// ancestor chain, which is where it lives on web too — never from the
    /// node's own previous paint.
    #[test]
    fn a_colour_only_restyle_keeps_the_inherited_font() {
        let inherited = Inherited {
            family: Some("Inter".into()),
            size_px: 56.0,
            weight: FontWeight::Bold,
            ..Default::default()
        };
        // Whatever this node resolved to previously is irrelevant: stale state
        // is exactly what made properties sticky.
        let stale = TextPaint {
            family: Some("Comic Sans".into()),
            size_px: 9.0,
            ..Default::default()
        };
        let mut style = StyleRules::default();
        style.color = Some(Tokenized::Literal(Color("#ff0000".into())));
        let out = resolve(&style, &stale, &inherited);
        assert!((out.color[0] - 1.0).abs() < 1e-3, "the declared colour applies");
        assert_eq!(out.family.as_deref(), Some("Inter"), "font inherited, not stale");
        assert_eq!(out.size_px, 56.0);
        assert!(matches!(out.weight, FontWeight::Bold));
    }

    /// A style that sets ONE font property must not reset the others to
    /// hardcoded defaults — it inherits them.
    ///
    /// This is the shape most labels have (`font_weight` only); resetting
    /// gives them 16px where web renders the inherited 14px — wrong metrics,
    /// and every ancestor's box wrong with them.
    #[test]
    fn regression_declaring_one_font_property_inherits_the_rest() {
        let inherited = Inherited {
            family: Some("Inter".into()),
            size_px: 14.0,
            ..Default::default()
        };
        let mut style = StyleRules::default();
        style.font_weight = Some(FontWeight::Bold);
        let out = resolve(&style, &TextPaint::default(), &inherited);
        assert!(matches!(out.weight, FontWeight::Bold), "the declared weight applies");
        assert_eq!(out.size_px, 14.0, "size must inherit, not fall back to 16px");
        assert_eq!(
            out.family.as_deref(),
            Some("Inter"),
            "family must inherit, not blank to the system font"
        );
    }

    #[test]
    fn resolve_reads_size_and_weight() {
        let mut style = StyleRules::default();
        style.font_size = Some(Tokenized::Literal(Length::Px(18.0)));
        style.font_weight = Some(FontWeight::SemiBold);
        style.letter_spacing = Some(Tokenized::Literal(0.6));
        let out = resolve(&style, &TextPaint::default(), &Inherited::default());
        assert_eq!(out.size_px, 18.0);
        assert!(matches!(out.weight, FontWeight::SemiBold));
        assert!((out.letter_spacing_px - 0.6).abs() < 1e-4);
    }

    #[test]
    fn resolve_reads_max_lines_and_folds_zero_to_unlimited() {
        let mut style = StyleRules::default();
        style.max_lines = Some(2);
        let out = resolve(&style, &TextPaint::default(), &Inherited::default());
        assert_eq!(out.max_lines, Some(2));
        style.max_lines = Some(0);
        let out = resolve(&style, &TextPaint::default(), &Inherited::default());
        assert_eq!(out.max_lines, None, "0 is documented as no limit");
    }

    /// A restyle that drops `max_lines` (e.g. a row that expands on press)
    /// must lift the limit, not keep the previous paint's one.
    #[test]
    fn regression_unsetting_max_lines_lifts_the_limit() {
        let limited = TextPaint {
            max_lines: Some(1),
            ..Default::default()
        };
        let out = resolve(&StyleRules::default(), &limited, &Inherited::default());
        assert_eq!(out.max_lines, None, "dropping max_lines must unlimit");
        assert_eq!(
            line_limit(out.max_lines),
            line_limit(TextPaint::default().max_lines),
            "an unlimited label is configured exactly like a fresh one",
        );
    }

    #[test]
    fn line_limit_unlimited_is_the_create_text_default() {
        let l = line_limit(None);
        assert!(l.wrap, "create_text labels wrap");
        assert_eq!(l.lines, -1);
        assert_eq!(l.ellipsize, pango::EllipsizeMode::None);
        assert_eq!(line_limit(Some(0)), l, "Some(0) is no limit");
    }

    /// One line must not wrap: `GtkLabel:lines` is ignored without wrap, and
    /// a wrapping label would still break and grow taller than one line.
    #[test]
    fn line_limit_one_line_ellipsizes_without_wrapping() {
        let l = line_limit(Some(1));
        assert!(!l.wrap);
        assert_eq!(l.ellipsize, pango::EllipsizeMode::End);
    }

    #[test]
    fn line_limit_many_lines_wraps_and_caps() {
        let l = line_limit(Some(3));
        assert!(l.wrap, "lines only applies to a wrapping label");
        assert_eq!(l.lines, 3);
        assert_eq!(l.ellipsize, pango::EllipsizeMode::End);
        assert_eq!(line_limit(Some(u32::MAX)).lines, i32::MAX, "no overflow");
    }

    #[test]
    fn weight_maps_black_to_heavy() {
        assert_eq!(map_weight(FontWeight::Black), pango::Weight::Heavy);
        assert_eq!(map_weight(FontWeight::ExtraLight), pango::Weight::Ultralight);
    }
}
