//! `StyleRules::cursor` → the GDK cursor the pointer takes over a widget.
//!
//! GTK4 resolves cursors by NAME through `gdk_cursor_new_from_name`, and
//! the names GDK accepts are the CSS `cursor` keywords — so the mapping
//! is 1:1 with what the web backend emits, and Linux gets the same
//! affordance for the same style with no per-platform fudging (CLAUDE.md
//! §7). Names GDK's theme can't resolve fall back to the theme default,
//! which is the same degradation macOS takes for the values NSCursor
//! lacks.
//!
//! [`Cursor::Auto`] maps to `None`, not to `"default"`: GTK inherits the
//! cursor from the nearest ancestor that sets one, and `"default"` would
//! pin an arrow that OVERRIDES an ancestor's affordance. `Auto` means
//! "don't express an opinion", so it must clear the widget's cursor and
//! let inheritance run — the same thing CSS `cursor: auto` does.

use runtime_shared::Cursor;

/// The GDK/CSS cursor name for `c`, or `None` for "inherit" ([`Cursor::Auto`]).
pub(crate) fn cursor_name(c: Cursor) -> Option<&'static str> {
    Some(match c {
        Cursor::Auto => return None,
        Cursor::Default => "default",
        Cursor::Pointer => "pointer",
        Cursor::Text => "text",
        Cursor::Wait => "wait",
        Cursor::Progress => "progress",
        Cursor::Help => "help",
        Cursor::NotAllowed => "not-allowed",
        Cursor::Move => "move",
        Cursor::Grab => "grab",
        Cursor::Grabbing => "grabbing",
        Cursor::Crosshair => "crosshair",
        Cursor::ColResize => "col-resize",
        Cursor::RowResize => "row-resize",
        Cursor::EwResize => "ew-resize",
        Cursor::NsResize => "ns-resize",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug: `apply_style` dropped `StyleRules::cursor` on the floor,
    /// so every clickable on Linux kept the plain arrow while the same
    /// tree showed a hand on web and macOS. idea-ui opts its clickables
    /// into `Cursor::Pointer` and that is the single source of truth, so
    /// this one value failing to map is the whole user-visible symptom.
    #[test]
    fn regression_pointer_maps_to_the_hand_cursor() {
        assert_eq!(cursor_name(Cursor::Pointer), Some("pointer"));
    }

    /// `Auto` must CLEAR the widget's cursor rather than pin an arrow —
    /// pinning `"default"` would override an ancestor's affordance (e.g.
    /// an unstyled child of a `Pointer` row killing the hand), which is
    /// not what CSS `cursor: auto` means.
    #[test]
    fn auto_inherits_rather_than_pinning_an_arrow() {
        assert_eq!(cursor_name(Cursor::Auto), None);
        assert_eq!(cursor_name(Cursor::Default), Some("default"));
    }

    /// Every variant must produce a name (or a deliberate `None`); a
    /// `_ => None` catch-all would silently swallow a newly-added
    /// variant, which is exactly the class of miss that left the whole
    /// property unimplemented here. The match in `cursor_name` is
    /// exhaustive, so this list failing to compile is the real guard —
    /// the assertions pin the CSS spellings GDK expects.
    #[test]
    fn every_variant_maps_to_its_css_keyword() {
        for (c, want) in [
            (Cursor::Default, Some("default")),
            (Cursor::Pointer, Some("pointer")),
            (Cursor::Text, Some("text")),
            (Cursor::Wait, Some("wait")),
            (Cursor::Progress, Some("progress")),
            (Cursor::Help, Some("help")),
            (Cursor::NotAllowed, Some("not-allowed")),
            (Cursor::Move, Some("move")),
            (Cursor::Grab, Some("grab")),
            (Cursor::Grabbing, Some("grabbing")),
            (Cursor::Crosshair, Some("crosshair")),
            (Cursor::ColResize, Some("col-resize")),
            (Cursor::RowResize, Some("row-resize")),
            (Cursor::EwResize, Some("ew-resize")),
            (Cursor::NsResize, Some("ns-resize")),
            (Cursor::Auto, None),
        ] {
            assert_eq!(cursor_name(c), want, "{c:?} mapped wrong");
        }
    }
}
