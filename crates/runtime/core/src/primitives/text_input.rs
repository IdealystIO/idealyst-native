//! TextInput primitive (controlled).
//!
//! Backed by `<input type="text">` on web, `UITextField` on iOS,
//! `EditText` on Android. The value is controlled — the parent owns
//! a `Signal<String>` that the framework subscribes to and writes to
//! the native widget; native input events fire `on_change` which the
//! parent uses to update the signal. Cyclic but stable: widgets
//! no-op when set to their current value.
//!
//! Why controlled by default? It matches the rest of the framework's
//! reactive shape — every input has a single source of truth (a
//! signal), and the parent decides how/whether to accept incoming
//! values (e.g. validation, transformation). Uncontrolled variants
//! can be added later if a real need arises.

use crate::primitives::key::{KeyEvent, KeyOutcome};
use crate::{Bound, Element, Ref, RefFill, Signal};
use std::any::Any;
use std::rc::Rc;

/// Handle exposed to a parent via `Ref<TextInputHandle>`. Backends
/// implement the ops trait below to make `focus()`, `blur()`,
/// `select_all()`, and `insert_text()` work.
#[derive(Clone)]
pub struct TextInputHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn TextInputOps,
}

impl TextInputHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextInputOps) -> Self {
        Self { node, ops }
    }

    /// Move keyboard focus to this input.
    pub fn focus(&self) {
        self.ops.focus(&*self.node);
    }

    /// Drop keyboard focus from this input.
    pub fn blur(&self) {
        self.ops.blur(&*self.node);
    }

    /// Select all the current text. Useful for "tap to edit"
    /// patterns where the entire value should be replaced on
    /// focus.
    pub fn select_all(&self) {
        self.ops.select_all(&*self.node);
    }

    /// Replace the current selection (or insert at the caret if no
    /// selection) with `text`, then place the caret immediately
    /// after the inserted text. Fires the same on-change signal
    /// path a real keystroke would, so the controlling `Signal`
    /// stays in sync.
    ///
    /// Typical use: from inside an [`on_key_down`](crate::primitives::key)
    /// handler that returns [`KeyOutcome::PreventDefault`], to
    /// substitute custom text for the suppressed default behaviour
    /// (e.g. inserting four spaces for Tab in a code editor).
    pub fn insert_text(&self, text: &str) {
        self.ops.insert_text(&*self.node, text);
    }
}

pub trait TextInputOps {
    fn focus(&self, node: &dyn Any);
    fn blur(&self, node: &dyn Any);
    fn select_all(&self, node: &dyn Any);
    /// See [`TextInputHandle::insert_text`]. Backends MUST replace
    /// the active selection (if any), advance the caret to the end
    /// of the inserted text, and fire the input's normal on-change
    /// path so the controlling `Signal` observes the new value.
    fn insert_text(&self, node: &dyn Any, text: &str);
}

/// Construct a `TextInput`. The `value` signal is the source of
/// truth — the input reflects whatever the signal currently holds.
/// `on_change` fires for every native input event with the new
/// text; the typical pattern is to call `value.set(new_text)`
/// inside the callback (the framework optimizes away the redundant
/// write-back when the signal already matches).
pub fn text_input<F: Fn(String) + 'static>(
    value: Signal<String>,
    on_change: F,
) -> Bound<TextInputHandle> {
    Bound::new(Element::TextInput {
        value,
        on_change: Rc::new(on_change),
        on_key_down: None,
        placeholder: None,
        secure: false,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<TextInputHandle> {
    /// Placeholder text shown when the input is empty.
    pub fn placeholder(mut self, text: String) -> Self {
        if let Element::TextInput { placeholder, .. } = &mut self.primitive {
            *placeholder = Some(text);
        }
        self
    }

    /// Mask the entered text (password entry). Maps to each backend's native
    /// secure-entry mode; the masked-character behaviour is identical
    /// everywhere. Default `false`.
    pub fn secure(mut self, is_secure: bool) -> Self {
        if let Element::TextInput { secure, .. } = &mut self.primitive {
            *secure = is_secure;
        }
        self
    }

    /// Bind to a `Ref<TextInputHandle>` for imperative
    /// `focus()`/`blur()`/`select_all()`/`insert_text()` from the parent.
    pub fn bind(mut self, r: Ref<TextInputHandle>) -> Self {
        if let Element::TextInput { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::TextInput(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Attach a keyboard hook that fires on every keydown while the
    /// input has focus. Return [`KeyOutcome::PreventDefault`] to
    /// suppress the platform's default behaviour for that key.
    /// See [`primitives::key`](crate::primitives::key) for the
    /// cross-platform contract.
    pub fn on_key_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&KeyEvent) -> KeyOutcome + 'static,
    {
        if let Element::TextInput { on_key_down, .. } = &mut self.primitive {
            *on_key_down = Some(Rc::new(handler));
        }
        self
    }
}
