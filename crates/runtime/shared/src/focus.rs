//! Keyboard-focus channel for containers — `view` / `pressable` `on_focus` —
//! and the app-wide record of whether a text entry holds focus
//! ([`text_entry_focused`], fed through [`TextEntryFocus`]).
//!
//! A focus handler fires `true` when **keyboard focus** lands on the node
//! or on anything inside it, and `false` when it leaves that subtree (or
//! stops being keyboard focus). It is focus-*within*: a `view` wrapping a
//! button hears the button's focus, which is what a wrapping component
//! (`HoverCard`, a toolbar group) needs — the wrapper itself is never a
//! tab stop.
//!
//! "Keyboard focus" is the focus a focus ring shows: the framework's
//! `StateBits::FOCUSED` rule, CSS `:focus-visible`. Tab / arrow
//! traversal and programmatic focus count; a mouse click that happens to
//! make a button first responder does not (it would otherwise open
//! focus-driven UI on every click). A text field counts on any focus, as
//! `:focus-visible` does — the ring marks where typing goes.
//!
//! The vocabulary forwards only *changes* (a backend that reports `false`
//! twice, or `false` before any `true`, is deduplicated), so a handler can
//! treat every call as a transition.
//!
//! Where a platform has no keyboard focus for an element the handler never
//! fires: touch-only devices without a hardware keyboard, and backends
//! with no focus model (see each backend's `InputOps::install_focus_handler`).

use std::rc::Rc;

/// Installed via the builder's `on_focus`. The `bool` argument is the new
/// state: `true` when keyboard focus enters the node's subtree, `false`
/// when it leaves. Born batched (one reactive cycle per call) like the
/// hover handler.
pub type FocusWithinHandler = Rc<dyn Fn(bool)>;

// ---------------------------------------------------------------------------
// Text entry focus
// ---------------------------------------------------------------------------

use std::cell::Cell;

runtime_tls::thread_local! {
    /// How many text entries say they hold focus. One at most in practice
    /// (a toolkit focuses one thing); a count rather than a flag so a
    /// late blur from one field can't clear another's focus when the two
    /// transitions arrive out of order (blur of the old after focus of the
    /// new, as GTK and AppKit can deliver them).
    static TEXT_ENTRIES_FOCUSED: Cell<usize> = const { Cell::new(0) };
}

/// Whether a field that takes typing — a `text_input` or `text_area`, or
/// an extension's own editor reporting through [`TextEntryFocus`] — holds
/// keyboard focus right now.
///
/// What app-wide shortcuts check: an `on_key` listener sees every key,
/// including the ones typed into a field, so a bare `T` shortcut must
/// stand down while this is `true`:
///
/// ```ignore
/// on_key(move |e| {
///     if !e.is_press() || text_entry_focused() { return KeyOutcome::Default }
///     if e.key == "t" { new_task(); return KeyOutcome::PreventDefault }
///     KeyOutcome::Default
/// });
/// ```
///
/// The vocabulary tracks every editable `text_input` and `text_area` on
/// every backend that reports their focus, so no field has to opt in. A
/// backend with no keyboard-focus model reports nothing and this stays
/// `false`. Not reactive: read it where the key arrives.
pub fn text_entry_focused() -> bool {
    TEXT_ENTRIES_FOCUSED.with(|n| n.get() > 0)
}

/// One text entry's share of [`text_entry_focused`]: `set(true)` while it
/// holds focus, `set(false)` when it loses it. Repeats are ignored, and
/// dropping it releases any focus it still claimed — a field unmounted
/// while focused may never deliver its blur, and must not leave the app
/// believing something is still being typed into.
///
/// The vocabulary keeps one per editable `text_input` / `text_area`. An
/// extension whose editor is not built on those (a canvas-drawn editor, a
/// web view) holds its own and feeds it its focus changes.
#[derive(Debug, Default)]
pub struct TextEntryFocus {
    held: Cell<bool>,
}

impl TextEntryFocus {
    pub fn new() -> Self {
        Self::default()
    }

    /// This entry gained (`true`) or lost (`false`) keyboard focus.
    pub fn set(&self, focused: bool) {
        if self.held.replace(focused) == focused {
            return;
        }
        TEXT_ENTRIES_FOCUSED.with(|n| {
            n.set(if focused { n.get() + 1 } else { n.get().saturating_sub(1) });
        });
    }

    /// Whether this entry holds focus, as last reported.
    pub fn is_focused(&self) -> bool {
        self.held.get()
    }
}

impl Drop for TextEntryFocus {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(test)]
mod text_entry_tests {
    use super::*;

    #[test]
    fn an_entry_counts_while_it_holds_focus() {
        let field = TextEntryFocus::new();
        assert!(!text_entry_focused());
        field.set(true);
        field.set(true);
        assert!(text_entry_focused());
        field.set(false);
        assert!(!text_entry_focused());
        // A blur with no focus before it changes nothing.
        field.set(false);
        assert!(!text_entry_focused());
    }

    #[test]
    fn regression_a_late_blur_does_not_clear_the_next_fields_focus() {
        let (a, b) = (TextEntryFocus::new(), TextEntryFocus::new());
        a.set(true);
        // Focus moves to `b`; the toolkit tells `b` first, then blurs `a`.
        b.set(true);
        a.set(false);
        assert!(text_entry_focused(), "b still holds focus");
        b.set(false);
        assert!(!text_entry_focused());
    }

    #[test]
    fn regression_an_entry_dropped_while_focused_releases_it() {
        let field = TextEntryFocus::new();
        field.set(true);
        drop(field);
        assert!(!text_entry_focused(), "an unmounted field must not keep shortcuts off");
    }
}
