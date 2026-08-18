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
    /// Absent from this struct entirely until the cross-platform parity sweep
    /// found it: web resolves CSS `line-height` and macOS feeds
    /// `style.line_height` into its layout policy, while GTK simply dropped the
    /// property. Every text box was therefore SHORTER on Linux than on web —
    /// measured on the docs app's Color page, a wrapped paragraph came out
    /// 39px tall against web's 50, a badge row 23 against 32 — and because a
    /// column's height is the sum of its children's, the error accumulated all
    /// the way up (that page's content column: 917px vs 991px). It moved no
    /// visual prop, so only a geometry diff could see it.
    pub line_height_px: Option<f32>,
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
            line_height_px: None,
            color: [0.0, 0.0, 0.0, 1.0],
            align: TextAlign::Left,
        }
    }
}

pub fn family_name(f: &FontFamily) -> Option<String> {
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

/// Read the text-relevant fields out of a node's [`StyleRules`] into a
/// fresh paint.
///
/// `prev` is carried ONLY for the properties this style says nothing
/// about at all (see the typography group below) — a `None` field is
/// otherwise "not set", i.e. the default, never "keep the old value".
/// Getting that backwards makes every text property one-way: settable
/// but never unsettable.
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

/// Resolve a text paint, with `inherited_color` standing in for the CSS
/// cascade's `color`.
///
/// The web backend gets inheritance for free: a `<span>` with no `color` picks
/// up its ancestor's, and an icon's `currentColor` resolves the same way. Native
/// backends have no cascade, so an unset colour used to fall back to
/// `TextPaint::default()`'s opaque black — right-looking under a light theme and
/// invisible under a dark one. The caller resolves the nearest ancestor that
/// declares a colour (see `LinuxBackend::ambient_color`) and passes it here.
pub fn resolve(style: &StyleRules, prev: &TextPaint, inherited: &Inherited) -> TextPaint {
    let mut tp = prev.clone();

    // Typography: start from what this node INHERITS, then overlay what it
    // declares. Each property independently, exactly as CSS cascades them.
    //
    // This used to layer over `prev` behind a "does the style mention any
    // typography" gate, rebuilding the whole group from `TextPaint::default()`
    // when it did. Two problems. The gate's default base meant a style setting
    // only `font_weight` — which is most labels — silently discarded an
    // inherited 14px for the hardcoded 16px and blanked the family: 167 wrong
    // font sizes and 394 missing families across the docs catalogue, and wrong
    // text metrics feed wrong box heights all the way up the tree. And layering
    // over `prev` at all made properties one-way: the website's table of
    // contents bolded its active entry and every visited entry stayed bold,
    // because the inactive style simply omitted `font_weight`.
    //
    // Inheriting instead fixes both by construction: `apply_style` always
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
    // whatever the previous style said" (see the group-vs-independent note
    // above).
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
/// `TagSheetBuilder::build_text` gives every Tag), sweeping font sizes:
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
/// Trusting the short report made Taffy size the docs app's `Tag(label = "✓
/// Verified")` 67px wide when GTK needed 68, so the label wrapped INSIDE its own
/// box and the glyph rendered ABOVE the word while web drew one line. Reported by
/// a user as "Tag with icon positions the icon above the text"; a real
/// cross-platform layout divergence produced entirely by believing a
/// one-pixel-short measurement.
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
    // margins have to come back off here. Skipping this double-counted
    // every padded leaf: Taffy sized the box to content+2×padding, so a
    // codeblock's padding rendered twice as large on Linux as everywhere
    // else.
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
    // resolving flex minimums, so this fired constantly and flooded the
    // log. Clamping to `wmin` asks the question GTK can answer (the
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::{Color, Tokenized};

    // Reported live: the website's TOC bolds its active entry, and every
    // entry stayed bold forever after. `resolve` layered each field over
    // the previous paint, so dropping `font_weight` left the old Bold in
    // place — a property could be turned on but never off.
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
    /// ancestor chain, which is where it lives on web too.
    ///
    /// This used to assert that the font survived from the node's OWN previous
    /// paint, behind a "does this style mention typography" gate. That gate is
    /// what made a style setting only `font_weight` discard an inherited size
    /// and family for `TextPaint::default()`'s 16px system font — 167 wrong
    /// sizes and 394 missing families across the docs catalogue. Resolving from
    /// the INHERITED context instead keeps a colour-only restyle harmless
    /// (below) without ever reading this node's stale state.
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
    /// This is the shape most labels have (`font_weight` only), and the reason
    /// 167 nodes rendered at 16px where web rendered 14px: wrong metrics, and
    /// every ancestor's box wrong with them.
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
    fn weight_maps_black_to_heavy() {
        assert_eq!(map_weight(FontWeight::Black), pango::Weight::Heavy);
        assert_eq!(map_weight(FontWeight::ExtraLight), pango::Weight::Ultralight);
    }
}
