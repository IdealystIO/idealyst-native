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
//! # Monospace, and why the label is NOT selectable
//!
//! The whole label gets a monospace `FontDescription` (matching the
//! macOS `monospacedSystemFontOfSize:` / iOS path); `set_wrap(true)` +
//! left x-align matches the framework's own `text` leaf defaults.
//!
//! The label is deliberately left NON-selectable, which also keeps it
//! non-focusable. `gtk_label_set_selectable()` flips the widget's
//! `focusable` property on, and GTK hands keyboard focus to the first
//! focusable widget in a freshly mapped subtree — so with selection on,
//! every navigation to a docs page parked the caret in that page's FIRST
//! code panel. The panel rendered with a focus caret/ring (it "looked
//! highlighted") and swallowed the arrow keys that should have scrolled
//! the page.
//!
//! Non-selectable is also the CONVERGENT behavior (CLAUDE.md §7): the
//! macOS handler builds its `NSTextField` with `labelWithString:`
//! ("non-editable, non-selectable" — see `macos.rs`), the iOS/Android
//! handlers use a plain `UILabel`/`TextView`, and the portable handler's
//! `<pre>` carries no `tabindex`. GTK was the lone backend making a code
//! panel a focus stop. If code selection is ever wanted, it has to be
//! added to every backend at once, not to this one.
//!
//! # Pango ranges are BYTE indices
//!
//! `AttrColor::set_start_index` / `set_end_index` are byte offsets into
//! the UTF-8 label text, NOT char or grapheme counts. We accumulate a
//! running byte cursor as we concatenate runs so a run containing
//! multi-byte characters still colors the correct range.
//!
//! # Measurement
//!
//! `register_external_view` installs a Taffy measure fn over the widget
//! (`backend_linux::widget_measure`), so the label sizes to its intrinsic
//! content when the author pins no explicit width/height; author
//! `width`/`height` from `.with_style` still win, because they land in
//! Taffy's `size` via `apply_style`.


use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::pango;
use gtk4::prelude::*;
use runtime_shared::color::{parse_or, Rgba};
use runtime_shared::Color;

/// Monospace point size for the code label. Matches the macOS/iOS
/// handlers' 13pt.
const MONO_SIZE_PT: f64 = 13.0;

/// Register the Linux `code_block` external handler on `backend`.
/// `Element::External` handler for the Linux codeblock kind: one
/// `gtk::Label` carrying the concatenated spans + a per-run color
/// `AttrList`.
pub(crate) fn build(spans: &[(String, Color)], b: &mut LinuxBackend) -> LinuxNode {
    let label = gtk4::Label::new(None);
    // Match the framework `text` leaf defaults + code-panel expectations.
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    // A plain `GtkLabel` is already non-focusable; stated explicitly so a
    // future edit that turns selection back on cannot silently
    // reintroduce the focus stop described in the module doc.
    label.set_focusable(false);

    let (text, attrs) = build_text_and_attrs(&spans);
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


// ===========================================================================
// GTK-dependent tests
//
// ONE `#[test]` on purpose: GTK4 requires every call to happen on the
// thread that ran `gtk::init`, and cargo hands each test its own thread,
// so a second GTK test in this binary segfaults. Skips when `gtk4::init()`
// fails (headless CI) — a display IS available in the dev environment, so
// the assertions really run there.
// ===========================================================================
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn spans() -> Vec<(String, Color)> {
        vec![
            ("fn main() {\n".to_string(), Color("#c586c0".into())),
            ("    println!(\"hi\");\n".to_string(), Color("#ce9178".into())),
            ("}".to_string(), Color("#d4d4d4".into())),
        ]
    }

    /// A code panel must not be a keyboard focus stop.
    ///
    /// The bug: `build` used to call `label.set_selectable(true)`, and
    /// `gtk_label_set_selectable()` turns the widget's `focusable`
    /// property on. GTK gives keyboard focus to the first focusable
    /// widget in a freshly mapped subtree, so every navigation to a docs
    /// page parked focus in that page's FIRST code panel: it drew a focus
    /// caret (reported as "the first codeblock seems highlighted") and
    /// swallowed the arrow keys that should have scrolled the page. No
    /// other backend's code panel takes focus (see the module doc).
    #[test]
    fn regression_code_panel_is_not_a_keyboard_focus_stop() {
        if gtk4::init().is_err() {
            eprintln!("SKIP: no display");
            return;
        }
        let window = gtk4::Window::new();
        window.set_default_size(600, 400);
        let mut backend = LinuxBackend::new(window.clone().upcast());
        let node = build(&spans(), &mut backend);
        let widget = node.widget().clone();
        let label = widget
            .downcast_ref::<gtk4::Label>()
            .expect("code panel leaf is a GtkLabel")
            .clone();

        // The label is the window's ONLY child, so if it is focusable at
        // all it is guaranteed to receive the window's initial focus —
        // which is exactly what made the docs app's first code panel
        // light up.
        window.set_child(Some(&widget));
        window.present();
        let ctx = gtk4::glib::MainContext::default();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(1500) {
            ctx.iteration(false);
            if window.is_mapped() && start.elapsed() > std::time::Duration::from_millis(300) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if !window.is_mapped() {
            eprintln!("SKIP: window never mapped");
            return;
        }

        assert!(
            !label.is_focusable(),
            "code panel label must not be focusable — a focusable label \
             becomes the mapped subtree's initial focus target and renders \
             with a focus caret"
        );
        assert!(
            !label.is_selectable(),
            "code panel label must not be selectable — selection flips \
             `focusable` back on, and no other backend's code panel is \
             selectable (CLAUDE.md §7)"
        );
        assert!(
            !label.has_focus(),
            "code panel label took keyboard focus on map"
        );
        assert!(
            label.selection_bounds().is_none(),
            "code panel label rendered with a text selection"
        );
        let focused = gtk4::prelude::GtkWindowExt::focus(&window);
        assert!(
            focused.as_ref() != Some(&widget),
            "window's initial focus landed on the code panel"
        );
    }
}
