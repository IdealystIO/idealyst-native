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
/// Long-content authors should wrap the result in a sized parent
/// (or pass a style with explicit `height` / `width`); the `<textarea>`
/// otherwise sizes to its default `rows`/`cols` which doesn't fit the
/// rest of the framework's flex-based layout.
pub fn text_area<F: Fn(String) + 'static>(
    value: Signal<String>,
    on_change: F,
) -> Bound<TextAreaHandle> {
    Bound::new(Element::TextArea {
        value,
        on_change: Rc::new(on_change),
        on_key_down: None,
        placeholder: None,
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
            *on_key_down = Some(Rc::new(handler));
        }
        self
    }
}
