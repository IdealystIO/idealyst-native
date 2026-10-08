//! `text_area` pieces GTK does not provide: the placeholder, the intrinsic
//! (autosize) measure, and the font plumbing both depend on.
//!
//! A `text_area` node is a `GtkScrolledWindow` (the Taffy leaf, carrying the
//! author's padding as GTK margins like every other leaf) around a
//! `GtkTextView`. Everything the author styles — text colour, caret, font,
//! placeholder — has to reach the INNER view; the box (background, border,
//! radius) is painted by the parent at the node's frame, exactly as for a
//! `text_input` (see `LinuxBackend::apply_native_widget_css`).
//!
//! # Placeholder
//!
//! `GtkTextView` has no placeholder property (`GtkEntry`'s is built into its
//! `GtkText`). The placeholder is a `GtkLabel` added with
//! `gtk_text_view_add_overlay` at buffer origin — i.e. where the first glyph
//! of an empty buffer sits (the caret position) — the same overlay approach
//! macOS takes for `NSTextView` (which has no placeholder either). It is:
//! - shown only while the buffer is empty, toggled from the buffer's
//!   `changed` signal, which fires for typing AND programmatic writes;
//! - a widget, never buffer text, so it is not part of the value and never
//!   reaches `on_change`;
//! - hit-transparent and unfocusable, so clicks and focus land on the view and
//!   the caret is unaffected;
//! - coloured by the backend's native-widget CSS identically to a
//!   `text_input`'s placeholder: the field's text colour at
//!   [`PLACEHOLDER_ALPHA`]. Pinning both explicitly keeps the two controls in
//!   step and keeps the desktop theme's dimming out of it.
//!
//! # Measure
//!
//! Soft-wrap text areas autosize like web / macOS / iOS / Android: content
//! height at the box width, floored at `max(1, min_rows)` lines and capped at
//! `max_rows` lines (past which the scrolled window scrolls), via the shared
//! [`resolve_text_area_height`]. `wrap == false` (the code-editor shape) is
//! sized by its style, exactly as on the other backends.
//!
//! `gtk_widget_measure` on the text view can NOT be used: its vertical
//! measure is the height of the layout at the view's CURRENT allocation
//! width (it ignores `for_size`), i.e. one layout pass stale — the same lag
//! that made macOS's `intrinsicContentSize` measure collapse the box after
//! each edit. Instead the text is laid out with Pango at the width Taffy is
//! asking about, one paragraph at a time, rounding each paragraph's height
//! the way `GtkTextLayout` does (`PANGO_PIXELS` of the logical extents), so
//! the box agrees with what the view draws to the pixel
//! (`tests/text_area_parity.rs` checks it against `gtk_text_view_get_line_yrange`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::pango;
use gtk4::prelude::*;
use runtime_layout::{AvailableSpace, LayoutNode, Size};
use runtime_shared::primitives::text_area::resolve_text_area_height;
use runtime_shared::FontWeight;

/// A placeholder is the field's text colour at this alpha — on `text_input`
/// AND `text_area`. 0.55 is the dimming GTK's own theme gives an entry
/// placeholder (`.dim-label`), so a Linux field reads the way users of the
/// platform expect, and it sits inside the 0.25–0.55 band the other
/// platforms' placeholder colours span.
pub(crate) const PLACEHOLDER_ALPHA: f32 = 0.55;

/// CSS class on the placeholder overlay label, targeted by the per-node
/// native-widget stylesheet.
pub(crate) const PLACEHOLDER_CLASS: &str = "idealyst-text-area-placeholder";

/// The font properties a text area actually DECLARES — its own style first,
/// then each property independently from the nearest ancestor that declares
/// it (the CSS cascade, resolved by the backend). `None` means nobody
/// declared it: the property is left to GTK's default font, NOT forced to the
/// framework's 16px text default, so an unstyled text area keeps the size it
/// always had.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DeclaredFont {
    pub family: Option<String>,
    pub size_px: Option<f32>,
    pub weight: Option<FontWeight>,
    pub italic: Option<bool>,
}

impl DeclaredFont {
    /// CSS declarations for the declared properties (empty when none are).
    pub(crate) fn css(&self) -> String {
        let mut out = String::new();
        if let Some(f) = &self.family {
            // Quoted, with `"`/`\` escaped, so any family name parses.
            let esc = f.replace('\\', "\\\\").replace('"', "\\\"");
            out.push_str(&format!("font-family: \"{esc}\"; "));
        }
        if let Some(px) = self.size_px {
            out.push_str(&format!("font-size: {px}px; "));
        }
        if let Some(w) = self.weight {
            out.push_str(&format!("font-weight: {}; ", css_weight(w)));
        }
        if let Some(i) = self.italic {
            out.push_str(if i { "font-style: italic; " } else { "font-style: normal; " });
        }
        out
    }

    /// Overlay the declared properties onto `fd` (the view's current font),
    /// so the measure uses exactly the font the CSS above renders with —
    /// without waiting for GTK to revalidate the style.
    fn overlay(&self, fd: &mut pango::FontDescription) {
        if let Some(f) = &self.family {
            fd.set_family(f);
        }
        if let Some(px) = self.size_px {
            // CSS px == Pango absolute device units at scale 1, which is how
            // GTK's CSS `font-size` reaches Pango too.
            fd.set_absolute_size(px as f64 * pango::SCALE as f64);
        }
        if let Some(w) = self.weight {
            fd.set_weight(crate::text::map_weight(w));
        }
        if let Some(i) = self.italic {
            fd.set_style(if i { pango::Style::Italic } else { pango::Style::Normal });
        }
    }
}

fn css_weight(w: FontWeight) -> u16 {
    match w {
        FontWeight::Thin => 100,
        FontWeight::ExtraLight => 200,
        FontWeight::Light => 300,
        FontWeight::Normal => 400,
        FontWeight::Medium => 500,
        FontWeight::SemiBold => 600,
        FontWeight::Bold => 700,
        FontWeight::ExtraBold => 800,
        FontWeight::Black => 900,
    }
}

/// Backend-side record for one text area.
pub(crate) struct TextAreaState {
    /// The node's Taffy leaf, re-marked dirty when the content changes.
    pub layout: LayoutNode,
    /// The declared font, shared with the measure closure.
    pub font: Rc<RefCell<DeclaredFont>>,
    /// Set by the buffer's `changed` signal; consumed at the start of the
    /// next layout pass (`LinuxBackend::flush_text_area_dirty`). A flag, not
    /// a direct `mark_dirty`: `changed` also fires inside
    /// `update_text_area_value`, while the backend is already mutably
    /// borrowed, so the signal handler cannot reach the layout tree itself.
    pub dirty: Rc<Cell<bool>>,
}

/// Add the placeholder overlay to `view`. Visibility then follows the buffer
/// (see [`sync_placeholder`]).
pub(crate) fn install_placeholder(view: &gtk4::TextView, text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class(PLACEHOLDER_CLASS);
    label.set_xalign(0.0);
    // Clicks fall through to the text view (so a click on the hint places the
    // caret and focuses the field), and Tab never stops on the hint.
    label.set_can_target(false);
    label.set_focusable(false);
    label.set_can_focus(false);
    // Buffer origin = where the empty buffer's caret / first glyph sits. The
    // text view's own CSS padding is zeroed (see `apply_native_widget_css`)
    // and its margins are 0, so this is also the content-box corner.
    view.add_overlay(&label, 0, 0);
    sync_placeholder(&view.buffer(), &label);
    let weak = label.downgrade();
    view.buffer().connect_changed(move |b| {
        if let Some(l) = weak.upgrade() {
            sync_placeholder(b, &l);
        }
    });
    label
}

fn sync_placeholder(buffer: &gtk4::TextBuffer, label: &gtk4::Label) {
    label.set_visible(buffer.char_count() == 0);
}

/// `PANGO_PIXELS`: round Pango units to the nearest pixel — the rounding
/// `GtkTextLayout` applies to each line's logical height.
fn pango_pixels(units: i32) -> i32 {
    (units + pango::SCALE / 2).div_euclid(pango::SCALE)
}

/// Taffy measure for a soft-wrap text area: its content-box size at the
/// width being resolved, rows-bounded. See the module docs.
pub(crate) fn measure(
    view: &gtk4::TextView,
    font: &DeclaredFont,
    min_rows: Option<u32>,
    max_rows: Option<u32>,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
) -> Size<f32> {
    let layout = view.create_pango_layout(None);
    let mut fd = view
        .pango_context()
        .font_description()
        .unwrap_or_else(pango::FontDescription::new);
    font.overlay(&mut fd);
    layout.set_font_description(Some(&fd));
    layout.set_wrap(pango::WrapMode::WordChar);

    let buffer = view.buffer();
    let (start, end) = buffer.bounds();
    let text = buffer.text(&start, &end, true);

    let h_margins = (view.left_margin() + view.right_margin()) as f32;
    let width = known.width.unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(w) => w,
        // Wrapping breaks anywhere, so a text area can shrink to nothing.
        AvailableSpace::MinContent => 0.0,
        // Widest unwrapped paragraph.
        AvailableSpace::MaxContent => {
            layout.set_width(-1);
            let widest = text
                .split('\n')
                .map(|p| {
                    layout.set_text(p);
                    layout.pixel_size().0
                })
                .max()
                .unwrap_or(0);
            widest as f32 + h_margins
        }
    });

    let text_w = width - h_margins;
    // A sub-pixel probe width would wrap one glyph per line; treat it as
    // unconstrained instead (Taffy only uses that height for flex minimums).
    layout.set_width(if text_w >= 1.0 {
        (text_w * pango::SCALE as f32).round() as i32
    } else {
        -1
    });
    // One paragraph per hard break, each rounded on its own, because that is
    // how the text view lays out and sums them (a single multi-paragraph
    // layout rounds once and drifts by up to a pixel per line).
    let content: i32 = text
        .split('\n')
        .map(|p| {
            layout.set_text(p);
            pango_pixels(layout.size().1)
        })
        .sum();
    layout.set_text("");
    let line = pango_pixels(layout.size().1);

    let v_margins = (view.top_margin() + view.bottom_margin()) as f32;
    let h = resolve_text_area_height(content as f32, line as f32, 0.0, min_rows, max_rows) + v_margins;
    Size {
        width,
        height: known.height.unwrap_or(h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pango_pixels_rounds_to_nearest_like_gtk() {
        let s = pango::SCALE;
        assert_eq!(pango_pixels(0), 0);
        assert_eq!(pango_pixels(18 * s), 18);
        assert_eq!(pango_pixels(18 * s + s / 2 - 1), 18);
        assert_eq!(pango_pixels(18 * s + s / 2), 19);
    }

    #[test]
    fn declared_font_css_only_emits_declared_properties() {
        assert_eq!(DeclaredFont::default().css(), "");
        let f = DeclaredFont {
            family: Some("Inter \"Display\"".into()),
            size_px: Some(20.0),
            weight: Some(FontWeight::SemiBold),
            italic: Some(false),
        };
        assert_eq!(
            f.css(),
            "font-family: \"Inter \\\"Display\\\"\"; font-size: 20px; font-weight: 600; \
             font-style: normal; "
        );
    }
}
