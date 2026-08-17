//! Linux (GTK4) handler for the `markdown` external.
//!
//! # STATUS: not wired — this module is not compiled
//!
//! `lib.rs` declares only `mod ir;` / `mod parse;`, so nothing pulls this
//! file into the crate. It also still speaks the pre-scene-registry
//! `Element::External` model (`backend.register_external::<MarkdownDoc,
//! _>`), which CLAUDE.md §3 records as removed. On Linux today markdown
//! renders through the ONE caps-generic handler `register` installs
//! (`mount_markdown`), the same path web/SSR/terminal take — correct
//! output, without this file's one-label optimization.
//!
//! Kept (rather than deleted) because it is the written half of a
//! single-node GTK leaf; wiring it up means declaring the module behind
//! `cfg(all(target_os = "linux", not(target_arch = "wasm32")))`, adding a
//! `Registry<LinuxBackend>` type-dispatch arm to `register` the way
//! `codeblock::register` does, and typechecking it for the first time —
//! it has never been compiled. Until then, treat every claim below as
//! unverified.
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
//! # Not selectable — and therefore not a focus stop
//!
//! `gtk_label_set_selectable()` also turns the widget's `focusable`
//! property on, and GTK hands keyboard focus to the first focusable
//! widget in a freshly mapped subtree. A selectable document label
//! therefore captured focus the moment its screen mounted: it drew a
//! focus caret (reading as a highlight over the first block) and ate the
//! arrow keys that should have scrolled the page. Text rendered from a
//! `text`/markdown leaf is not a focus stop on any other backend, so the
//! label stays non-selectable here too (CLAUDE.md §7 — converge on the
//! observable behavior). Adding text selection is an all-backends change,
//! not a GTK-local one.
//!
//! # Pango ranges are BYTE indices
//!
//! Attribute `start_index`/`end_index` are byte offsets into the UTF-8
//! label text, not char counts. We advance a running byte cursor as we
//! concatenate segments so multi-byte runs (`│`, `─`, `•`, emoji) color
//! the correct range.
//!
//! # Measurement
//!
//! `register_external_view` installs a Taffy measure fn over the widget
//! (`backend_linux::widget_measure`), so the label sizes to its intrinsic
//! content when the author pins no explicit width/height. Author
//! `width`/`height` from `.with_style` still win — they land in Taffy's
//! `size` via `apply_style`, which overrides the measured intrinsic.

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
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    // See "Not selectable — and therefore not a focus stop" above.
    label.set_focusable(false);

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

