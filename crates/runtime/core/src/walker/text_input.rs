//! `Element::TextInput` and `Element::TextArea` build paths.
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
use crate::primitives::text_input::{BlurHandler, FocusHandler};
use crate::reactive::{Effect, Signal};
use crate::sources::StyleSource;
use crate::Reactive;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build_text_input<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    value: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>>,
    on_blur: Option<BlurHandler>,
    on_focus: Option<FocusHandler>,
    placeholder: Reactive<Option<String>>,
    secure: Reactive<bool>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    // The create-time mask state. Backends pick the right native widget
    // up front (notably AppKit, where secure entry is a distinct cell
    // class), so the input is born correct; the effect below only runs
    // for a *live* source.
    let initial_secure = secure.get();
    let initial_placeholder = placeholder.get();
    let n = time_backend_create(pkind!(TextInput), || {
        backend.borrow_mut().create_text_input(
            &initial,
            initial_placeholder.as_deref(),
            on_change,
            on_key_down,
            on_blur,
            initial_secure,
            &a11y,
        )
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Author focus notifier: install the backend hook so it fires the handler
    // on focus/blur. Backends without focus events default to a no-op (the
    // adorned-Field ring just won't light there). Kept here, NOT in
    // `create_text_input`, so the 15 backend signatures stay untouched.
    if let Some(on_focus) = on_focus {
        backend.borrow_mut().set_text_input_focus_handler(&n, on_focus);
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
    // Reactive `secure`: only a *live* source installs an effect. When the
    // source changes, toggle the native secure-entry mode in place (e.g. a
    // password show/hide) without rebuilding the input — the controlled
    // `value` carries the typed text across the toggle. A `Static` mask
    // stays on the create-time value with no effect (the common case).
    if let Reactive::Dynamic(_) = &secure {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let s = secure.get();
            backend.borrow_mut().update_text_input_secure(&node, s);
        });
    }
    // Reactive `placeholder`: same shape — a live source updates the native
    // placeholder in place; a `Static` placeholder installs no effect.
    if let Reactive::Dynamic(_) = &placeholder {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let p = placeholder.get();
            backend.borrow_mut().update_text_input_placeholder(&node, p.as_deref());
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
    wrap: bool,
    min_rows: Option<u32>,
    max_rows: Option<u32>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    let n = time_backend_create(pkind!(TextArea), || {
        backend.borrow_mut().create_text_area(
            &initial,
            placeholder.as_deref(),
            wrap,
            min_rows,
            max_rows,
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
