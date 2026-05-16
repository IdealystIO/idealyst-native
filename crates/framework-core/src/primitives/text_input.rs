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

use crate::{Bound, Primitive, Ref, RefFill, Signal};
use std::any::Any;
use std::rc::Rc;

/// Handle exposed to a parent via `Ref<TextInputHandle>`. Backends
/// implement the ops trait below to make `focus()`, `blur()`, and
/// `select_all()` work.
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
}

pub trait TextInputOps {
    fn focus(&self, node: &dyn Any);
    fn blur(&self, node: &dyn Any);
    fn select_all(&self, node: &dyn Any);
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
    Bound::new(Primitive::TextInput {
        value,
        on_change: Rc::new(on_change),
        placeholder: None,
        style: None,
        ref_fill: None,
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<TextInputHandle> {
    /// Placeholder text shown when the input is empty.
    pub fn placeholder(mut self, text: String) -> Self {
        if let Primitive::TextInput { placeholder, .. } = &mut self.primitive {
            *placeholder = Some(text);
        }
        self
    }

    /// Bind to a `Ref<TextInputHandle>` for imperative
    /// `focus()`/`blur()`/`select_all()` from the parent.
    pub fn bind(mut self, r: Ref<TextInputHandle>) -> Self {
        if let Primitive::TextInput { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::TextInput(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
