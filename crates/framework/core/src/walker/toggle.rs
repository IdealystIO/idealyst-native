//! `Primitive::Toggle` build path. Same controlled pattern as
//! TextInput: `Signal<bool>` round-trips through `on_change`; the
//! framework installs an Effect that pushes signal changes back into
//! the native widget.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::reactive::{Effect, Signal};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    value: Signal<bool>,
    on_change: Rc<dyn Fn(bool)>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    let n = time_backend_create(pkind!(Toggle), || {
        backend.borrow_mut().create_toggle(initial, on_change, &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let v = value.get();
            backend.borrow_mut().update_toggle_value(&node, v);
        });
    }
    if let Some(RefFill::Toggle(fill)) = ref_fill {
        let handle = backend.borrow().make_toggle_handle(&n);
        fill(handle);
    }
    n
}
