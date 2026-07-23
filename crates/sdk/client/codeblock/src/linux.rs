//! Linux (GTK4) handler for the `code_block` external.
//!
//! Renders the whole span list as ONE `gtk::Label` whose text is the
//! concatenation of every run and whose per-token foreground colors are
//! carried as `pango::AttrColor` ranges on a single `pango::AttrList` —
//! the GTK analogue of the iOS/macOS `NSAttributedString` and Android
//! `SpannableString` paths (one native text widget, one attribute range
//! per run, zero per-token widgets). Mirrors how
//! `crates/backend/linux/src/text.rs` styles a real `text` node: a Pango
//! `AttrList`, not a CSS provider, so restyles are a cheap attribute
//! rebuild.
//!
//! # Monospace + selection
//!
//! The whole label gets a monospace `FontDescription` (matching the
//! macOS `monospacedSystemFontOfSize:` / iOS path). `set_selectable(true)`
//! lets the reader copy code; `set_wrap(true)` + left x-align matches the
//! framework's own `text` leaf defaults.
//!
//! # Pango ranges are BYTE indices
//!
//! `AttrColor::set_start_index` / `set_end_index` are byte offsets into
//! the UTF-8 label text, NOT char or grapheme counts. We accumulate a
//! running byte cursor as we concatenate runs so a run containing
//! multi-byte characters still colors the correct range.
//!
//! # No measure fn (backend limitation)
//!
//! `register_external_view` wraps the label as a plain leaf without a
//! Taffy measure function (the `layout` handle it would need is
//! `pub(crate)` on `LinuxBackend`, and this crate must not edit the
//! backend). The label therefore sizes from the author's `.with_style`
//! dimensions on the `code_block` node. A width-aware external measure is
//! a backend-side follow-up, same shape as macOS's
//! `install_external_content_measure`.

use crate::CodeBlockProps;
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::pango;
use gtk4::prelude::*;
use runtime_core::color::{parse_or, Rgba};
use runtime_core::Color;
use std::rc::Rc;

/// Monospace point size for the code label. Matches the macOS/iOS
/// handlers' 13pt.
const MONO_SIZE_PT: f64 = 13.0;

/// Register the Linux `code_block` external handler on `backend`.
pub fn register(backend: &mut LinuxBackend) {
    backend.register_external::<CodeBlockProps, _>(|props, b| build(props, b));
}

/// `Element::External` handler for the Linux codeblock kind: one
/// `gtk::Label` carrying the concatenated spans + a per-run color
/// `AttrList`.
fn build(props: &Rc<CodeBlockProps>, b: &mut LinuxBackend) -> LinuxNode {
    let label = gtk4::Label::new(None);
    // Match the framework `text` leaf defaults + code-panel expectations.
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_xalign(0.0);
    label.set_yalign(0.0);

    let (text, attrs) = build_text_and_attrs(&props.spans);
    label.set_text(&text);
    label.set_attributes(Some(&attrs));

    b.register_external_view(label.upcast())
}

/// Concatenate the span texts into one string and build the matching
/// `AttrList`: a whole-range monospace font plus one foreground-color
/// attribute per run, keyed on byte ranges into the concatenated text.
fn build_text_and_attrs(spans: &[(String, Color)]) -> (String, pango::AttrList) {
    let attrs = pango::AttrList::new();

    // Whole-range monospace font. `pango::Family::Monospace` isn't a
    // constant, so we name the generic "Monospace" family Pango maps to
    // the system fixed-width face (the same lookup GTK's CSS
    // `font-family: monospace` resolves).
    let mut fd = pango::FontDescription::new();
    fd.set_family("Monospace");
    fd.set_absolute_size(MONO_SIZE_PT * pango::SCALE as f64);
    attrs.insert(pango::AttrFontDesc::new(&fd));

    let mut text = String::new();
    let mut byte_cursor: usize = 0;
    for (run_text, color) in spans {
        let start = byte_cursor;
        text.push_str(run_text);
        byte_cursor += run_text.len();

        let [r, g, b, a] = parse_or(&color.0, Rgba::BLACK).to_srgb_f32();
        let mut fg = pango::AttrColor::new_foreground(
            channel_to_u16(r),
            channel_to_u16(g),
            channel_to_u16(b),
        );
        fg.set_start_index(start as u32);
        fg.set_end_index(byte_cursor as u32);
        attrs.insert(fg);

        // Preserve author alpha (rare in highlighter output, but honor
        // it): a second attribute over the same range.
        let mut alpha = pango::AttrInt::new_foreground_alpha(channel_to_u16(a));
        alpha.set_start_index(start as u32);
        alpha.set_end_index(byte_cursor as u32);
        attrs.insert(alpha);
    }

    (text, attrs)
}

/// A single sRGB channel (`0..=1`) → Pango's 16-bit-per-channel form
/// (`0..=65535`). Mirrors `backend_linux`'s private `channel_to_u16`,
/// reimplemented here because the backend exposes no public color helper.
fn channel_to_u16(c: f32) -> u16 {
    (c.clamp(0.0, 1.0) * 65535.0).round() as u16
}
