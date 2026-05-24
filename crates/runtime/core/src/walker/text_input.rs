//! `Primitive::TextInput` and `Primitive::TextArea` build paths.
//! Same controlled-value pattern: the parent owns a `Signal<String>`;
//! the framework installs an Effect that writes the signal's value
//! back to the native widget on every change. Widgets no-op when set
//! to their current value, so the round-trip stays stable.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitives::key::{KeyEvent, KeyOutcome};
use crate::reactive::{Effect, Signal};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build_text_input<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    value: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>>,
    placeholder: Option<String>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    let n = time_backend_create(pkind!(TextInput), || {
        backend.borrow_mut().create_text_input(
            &initial,
            placeholder.as_deref(),
            on_change,
            on_key_down,
            &a11y,
        )
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Reactive: whenever the controlled signal changes, push
    // the new value into the widget. Setting to the same
    // value is a no-op on most platforms (web ignores no-change
    // sets on inputs).
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let v = value.get();
            backend.borrow_mut().update_text_input_value(&node, &v);
        });
    }
    if let Some(RefFill::TextInput(fill)) = ref_fill {
        let handle = backend.borrow().make_text_input_handle(&n);
        fill(handle);
    }
    n
}

pub(super) fn build_text_area<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    value: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>>,
    placeholder: Option<String>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    let n = time_backend_create(pkind!(TextArea), || {
        backend.borrow_mut().create_text_area(
            &initial,
            placeholder.as_deref(),
            on_change,
            on_key_down,
            &a11y,
        )
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Same controlled-value effect as TextInput. The textarea
    // ignores no-change sets, so this is safe to fire on every
    // signal write — including the one our own `on_change`
    // round-trips back.
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let v = value.get();
            backend.borrow_mut().update_text_area_value(&node, &v);
        });
    }
    if let Some(RefFill::TextArea(fill)) = ref_fill {
        let handle = backend.borrow().make_text_area_handle(&n);
        fill(handle);
    }
    n
}
