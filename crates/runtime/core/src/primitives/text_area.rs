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
use std::any::Any;
use std::rc::Rc;

/// Handle exposed to a parent via `Ref<TextAreaHandle>`. Backends
/// implement the ops trait below to make `focus()`, `blur()`,
/// `select_all()`, and `insert_text()` work — same surface as
/// `TextInputHandle`, just on the multi-line widget.
#[derive(Clone)]
pub struct TextAreaHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn TextAreaOps,
}

impl TextAreaHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextAreaOps) -> Self {
        Self { node, ops }
    }

    pub fn focus(&self) {
        self.ops.focus(&*self.node);
    }

    pub fn blur(&self) {
        self.ops.blur(&*self.node);
    }

    pub fn select_all(&self) {
        self.ops.select_all(&*self.node);
    }

    /// Replace the current selection (or insert at the caret if no
    /// selection) with `text`, then place the caret immediately
    /// after the inserted text. Fires the same on-change signal
    /// path a real keystroke would, so the controlling `Signal`
    /// stays in sync. See
    /// [`TextInputHandle::insert_text`](crate::TextInputHandle::insert_text)
    /// for the canonical use-case.
    pub fn insert_text(&self, text: &str) {
        self.ops.insert_text(&*self.node, text);
    }
}

pub trait TextAreaOps {
    fn focus(&self, node: &dyn Any);
    fn blur(&self, node: &dyn Any);
    fn select_all(&self, node: &dyn Any);
    /// See [`TextAreaHandle::insert_text`].
    fn insert_text(&self, node: &dyn Any, text: &str);
}

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

/// Resolve an autogrowing text area's border-box **height** from its measured
/// content — the shared cross-backend autosize math. Every backend computes
/// `content_h` (the glyph-layout height for the current width), `line_h` (the
/// font's single-line height), and `v_pad` (the per-edge vertical padding) from
/// its OWN real metrics, then calls this so the `min_rows`/`max_rows` → pixel
/// conversion is identical and uses each platform's true line height instead of
/// an estimate (the old idea-ui guess).
///
/// - floors at `max(1, min_rows)` lines so an empty box is never shorter than
///   its resting row count;
/// - caps at `max_rows` lines (the native widget then scrolls past it);
/// - adds `v_pad` to BOTH the top and bottom edge (web `box-sizing: border-box`
///   parity).
///
/// An explicit style `min_height`/`max_height` still applies on top (the layout
/// engine clamps the returned height), so authors keep a pixel-precise override.
pub fn resolve_text_area_height(
    content_h: f32,
    line_h: f32,
    v_pad: f32,
    min_rows: Option<u32>,
    max_rows: Option<u32>,
) -> f32 {
    let pad = v_pad.max(0.0) * 2.0;
    let line = line_h.max(0.0);
    let floor_rows = min_rows.unwrap_or(1).max(1) as f32;
    let mut h = content_h.max(0.0) + pad;
    h = h.max(floor_rows * line + pad);
    if let Some(mx) = max_rows {
        let cap = (mx.max(1) as f32) * line + pad;
        h = h.min(cap);
    }
    h
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

#[cfg(test)]
mod resolve_height_tests {
    use super::resolve_text_area_height;

    // line_h = 20, v_pad = 8 → padding contributes 16; one line + pad = 36.

    #[test]
    fn empty_floors_at_one_line_when_no_min_rows() {
        // No content, no floor → exactly one line + padding.
        assert_eq!(resolve_text_area_height(0.0, 20.0, 8.0, None, None), 36.0);
    }

    #[test]
    fn floors_at_min_rows() {
        // Three-row floor with one line of content → 3 lines + padding.
        assert_eq!(resolve_text_area_height(20.0, 20.0, 8.0, Some(3), None), 76.0);
    }

    #[test]
    fn grows_with_content_between_floor_and_cap() {
        // Five lines of content, floor 3, cap 8 → content wins.
        assert_eq!(resolve_text_area_height(100.0, 20.0, 8.0, Some(3), Some(8)), 116.0);
    }

    #[test]
    fn caps_at_max_rows_then_scrolls() {
        // Twelve lines of content but capped at 8 → 8 lines + padding (the
        // native widget scrolls the overflow).
        assert_eq!(resolve_text_area_height(240.0, 20.0, 8.0, Some(3), Some(8)), 176.0);
    }

    #[test]
    fn cap_below_floor_clamps_to_cap() {
        // Degenerate author input (max_rows < min_rows): the cap wins so the
        // box can't exceed its own stated maximum.
        assert_eq!(resolve_text_area_height(200.0, 20.0, 8.0, Some(6), Some(2)), 56.0);
    }

    #[test]
    fn zero_rows_treated_as_one_line() {
        // `min_rows(0)` / `max_rows(0)` are nonsense → clamped up to one line.
        assert_eq!(resolve_text_area_height(0.0, 20.0, 0.0, Some(0), Some(0)), 20.0);
    }
}
