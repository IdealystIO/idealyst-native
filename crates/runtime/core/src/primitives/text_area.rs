//! TextArea primitive (controlled, multi-line).
//!
//! Same shape as [`crate::primitives::text_input::text_input`] but
//! accepts newlines and renders multi-line. Web maps to
//! `<textarea>` (multi-line, undo/redo, scroll, tab — everything the
//! browser already does for editable text). iOS would map to
//! `UITextView`; Android to `EditText` with `inputType="textMultiLine"`.
//! The wgpu render backend currently surfaces it as an "Unsupported"
//! placeholder — a native multi-line editor on the wgpu side is the
//! obvious follow-up but lives outside the v1 surface.
//!
//! Why a separate primitive instead of a "multi-line" flag on
//! `TextInput`? The two have different keyboard semantics (Enter
//! submits vs. inserts newline), different default heights, and
//! different platform widget mappings. Keeping them separate keeps
//! each call site honest about which shape it wants.

use crate::primitives::key::{KeyEvent, KeyOutcome};
use crate::{Bound, Element, Ref, RefFill, Signal};
use std::rc::Rc;

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::text_area::*;

/// Construct a `TextArea`. Controlled — `value` is the source of
/// truth, `on_change` fires per keystroke with the full new text.
///
/// The box is **intrinsically sized to its content** (like `text`):
/// with no height pinned it grows to fit the text and shrinks as text
/// is removed. Constrain it through the normal style fields:
///
/// - a `height` (or a sized / absolutely-positioned parent) pins it to
///   a fixed box that scrolls past its bounds;
/// - a `min_height` sets a resting floor it never shrinks below;
/// - a `max_height` lets it grow to a cap, then scroll.
///
/// The only reshaping knob is wrapping:
/// [`wrap(false)`](Bound::wrap) / [`code_mode()`](Bound::code_mode)
/// keeps lines unwrapped and scrolls horizontally — the code-editor
/// shape (which is fixed-height, not content-grown).
#[cfg(feature = "prim-text-input")]
pub fn text_area<F: Fn(String) + 'static>(
    value: Signal<String>,
    on_change: F,
) -> Bound<TextAreaHandle> {
    Bound::new(Element::TextArea {
        value,
        // Born batched — see `reactive::cycle`.
        on_change: Rc::new(move |s: String| crate::cycle(|| on_change(s))),
        on_key_down: None,
        placeholder: None,
        // Standard textarea default: soft-wrap on. The code-editor
        // shape is the explicit opt-out.
        wrap: true,
        // Unbounded autogrow by default: floor at one line, no cap.
        min_rows: None,
        max_rows: None,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<TextAreaHandle> {
    pub fn placeholder(mut self, text: String) -> Self {
        if let Element::TextArea { placeholder, .. } = &mut self.primitive {
            *placeholder = Some(text);
        }
        self
    }

    pub fn bind(mut self, r: Ref<TextAreaHandle>) -> Self {
        if let Element::TextArea { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::TextArea(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Toggle soft-wrapping. `true` (the default) wraps long lines at
    /// the box edge; `false` keeps them unwrapped and scrolls
    /// horizontally — the code-editor shape. See also
    /// [`code_mode()`](Self::code_mode).
    pub fn wrap(mut self, wrap: bool) -> Self {
        if let Element::TextArea { wrap: w, .. } = &mut self.primitive {
            *w = wrap;
        }
        self
    }

    /// Convenience for the code-editor shape: unwrapped lines that
    /// scroll horizontally. Equivalent to `.wrap(false)`. A code editor
    /// is fixed-height (it scrolls rather than growing to the file
    /// length), so pair it with a pinned height or a sized parent at
    /// the call site (see `examples/fiddle`).
    pub fn code_mode(self) -> Self {
        self.wrap(false)
    }

    /// Resting floor in text lines: the autogrowing box is at least this
    /// many rows tall and never shrinks below it. The backend converts
    /// rows→pixels using its real font line height, so the floor is exact
    /// on every platform. An explicit style `min_height` overrides it.
    pub fn min_rows(mut self, rows: u32) -> Self {
        if let Element::TextArea { min_rows, .. } = &mut self.primitive {
            *min_rows = Some(rows);
        }
        self
    }

    /// Growth cap in text lines: once the content needs more rows than
    /// this the box stops growing and scrolls. Leaves the box uncapped
    /// when never set. An explicit style `max_height` overrides it.
    pub fn max_rows(mut self, rows: u32) -> Self {
        if let Element::TextArea { max_rows, .. } = &mut self.primitive {
            *max_rows = Some(rows);
        }
        self
    }

    /// Attach a keyboard hook that fires on every keydown while the
    /// textarea has focus. Return [`KeyOutcome::PreventDefault`] to
    /// suppress the platform's default behaviour for that key. See
    /// [`primitives::key`](crate::primitives::key) for the
    /// cross-platform contract.
    pub fn on_key_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&KeyEvent) -> KeyOutcome + 'static,
    {
        if let Element::TextArea { on_key_down, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`.
            *on_key_down = Some(Rc::new(move |e: &KeyEvent| crate::cycle(|| handler(e))));
        }
        self
    }
}
