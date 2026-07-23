//! Linux (GTK4) handler for the `markdown` external.
//!
//! Renders the WHOLE document as ONE `gtk::Label` whose text is the
//! concatenation of the shared [`segments::lower`] flattening and whose
//! per-segment styling is carried as `pango::Attr*` ranges on a single
//! `pango::AttrList`. This is the GTK analogue of the iOS/macOS
//! `NSAttributedString` path and the Android `SpannableStringBuilder`
//! path: one native text widget, one attribute range per uniform run,
//! zero per-run widgets (CLAUDE.md rule 7 — same observable output as the
//! other backends, different mechanism). The `AttrList` mechanism mirrors
//! `crates/backend/linux/src/text.rs`, which styles a real `text` leaf the
//! same way.
//!
//! Each [`segments::Seg`] maps to a byte range carrying:
//! - `AttrFontDesc` — family (monospace for code), absolute size (heading
//!   scaling), weight (bold), style (italic);
//! - `AttrColor` foreground + `AttrInt` foreground-alpha;
//! - optional `AttrColor` background + background-alpha (inline code / code
//!   block tint);
//! - `AttrInt` underline (links) / strikethrough (GFM `~~`).
//!
//! # Pango ranges are BYTE indices
//!
//! Attribute `start_index`/`end_index` are byte offsets into the UTF-8
//! label text, not char counts. We advance a running byte cursor as we
//! concatenate segments so multi-byte runs (`│`, `─`, `•`, emoji) color
//! the correct range.
//!
//! # No measure fn (backend limitation)
//!
//! `register_external_view` wraps the label as a plain leaf without a
//! Taffy measure function (the backend's `layout` handle is `pub(crate)`
//! and this crate must not edit `backend-linux`). The label sizes from the
//! author's `.with_style` dimensions on the markdown node. A width-aware
//! wrapping measure is a backend-side follow-up, the shape of iOS's
//! `install_external_wrapping_measure`.

use crate::ir::MarkdownDoc;
use crate::segments::{self, Seg};
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::pango;
use gtk4::prelude::*;
use runtime_core::color::{parse_or, Rgba};
use std::rc::Rc;

/// Set an attribute's byte range and insert it into the list. Monomorphic
/// per call site (each `pango::Attr*` is a distinct type sharing the
/// `set_start_index`/`set_end_index` surface), so no generic bound is
/// needed.
macro_rules! push_ranged {
    ($attrs:expr, $attr:expr, $start:expr, $end:expr) => {{
        let mut a = $attr;
        a.set_start_index($start as u32);
        a.set_end_index($end as u32);
        $attrs.insert(a);
    }};
}

/// Register the Linux `markdown` external handler on `backend`.
pub fn register(backend: &mut LinuxBackend) {
    backend.register_external::<MarkdownDoc, _>(|doc, b| build(doc, b));
}

/// `Element::External` handler: one `gtk::Label` carrying the whole doc.
fn build(doc: &Rc<MarkdownDoc>, b: &mut LinuxBackend) -> LinuxNode {
    let label = gtk4::Label::new(None);
    // Multi-line, word-wrapped, top-left aligned — matches the framework
    // `text` leaf defaults and the mobile handlers' wrapping labels.
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_yalign(0.0);

    let segs = segments::lower(doc);
    let (text, attrs) = build_text_and_attrs(&segs);
    label.set_text(&text);
    label.set_attributes(Some(&attrs));

    b.register_external_view(label.upcast())
}

/// Concatenate the segment texts into one string and build the matching
/// `AttrList`, one set of byte-ranged attributes per segment.
fn build_text_and_attrs(segs: &[Seg]) -> (String, pango::AttrList) {
    let attrs = pango::AttrList::new();
    let mut text = String::new();
    let mut byte_cursor: usize = 0;

    for seg in segs {
        let start = byte_cursor;
        text.push_str(&seg.text);
        byte_cursor += seg.text.len();
        let end = byte_cursor;
        let s = &seg.style;

        // Font: family (mono → generic "Monospace", which Pango maps to
        // the system fixed-width face), absolute size (px × PANGO_SCALE),
        // weight, style.
        let mut fd = pango::FontDescription::new();
        if s.mono {
            fd.set_family("Monospace");
        }
        fd.set_absolute_size(s.size as f64 * pango::SCALE as f64);
        fd.set_weight(if s.bold {
            pango::Weight::Bold
        } else {
            pango::Weight::Normal
        });
        if s.italic {
            fd.set_style(pango::Style::Italic);
        }
        push_ranged!(attrs, pango::AttrFontDesc::new(&fd), start, end);

        // Foreground color + alpha.
        let [r, g, b, a] = parse_or(&s.color, Rgba::BLACK).to_srgb_f32();
        push_ranged!(
            attrs,
            pango::AttrColor::new_foreground(
                channel_to_u16(r),
                channel_to_u16(g),
                channel_to_u16(b),
            ),
            start,
            end
        );
        push_ranged!(
            attrs,
            pango::AttrInt::new_foreground_alpha(channel_to_u16(a)),
            start,
            end
        );

        // Optional background tint (inline code / code block).
        if let Some(bg) = &s.bg {
            let [br, bg_, bb, ba] = parse_or(bg, Rgba::TRANSPARENT).to_srgb_f32();
            push_ranged!(
                attrs,
                pango::AttrColor::new_background(
                    channel_to_u16(br),
                    channel_to_u16(bg_),
                    channel_to_u16(bb),
                ),
                start,
                end
            );
            push_ranged!(
                attrs,
                pango::AttrInt::new_background_alpha(channel_to_u16(ba)),
                start,
                end
            );
        }

        if s.underline {
            push_ranged!(
                attrs,
                pango::AttrInt::new_underline(pango::Underline::Single),
                start,
                end
            );
        }
        if s.strike {
            push_ranged!(attrs, pango::AttrInt::new_strikethrough(true), start, end);
        }
    }

    (text, attrs)
}

/// A single sRGB channel (`0..=1`) → Pango's 16-bit-per-channel form
/// (`0..=65535`). Mirrors `backend_linux`'s private `channel_to_u16`,
/// reimplemented here because the backend exposes no public color helper.
fn channel_to_u16(c: f32) -> u16 {
    (c.clamp(0.0, 1.0) * 65535.0).round() as u16
}
