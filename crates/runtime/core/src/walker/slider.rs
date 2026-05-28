//! `Element::Slider` build path. The walker wraps the user's
//! `on_change` to snap to `step` before dispatching so all backends
//! produce identical values regardless of native step handling.

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
    value: Signal<f32>,
    on_change: Rc<dyn Fn(f32)>,
    min: f32,
    max: f32,
    step: Option<f32>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = value.get();
    // Wrap the user's on_change to snap to `step` first, so all
    // backends produce identical values regardless of native
    // step handling.
    let on_change_snap: Rc<dyn Fn(f32)> = if let Some(s) = step {
        let user = on_change.clone();
        let min_c = min;
        Rc::new(move |v| {
            let snapped = min_c + ((v - min_c) / s).round() * s;
            user(snapped);
        })
    } else {
        on_change.clone()
    };
    let n = time_backend_create(pkind!(Slider), || {
        backend.borrow_mut().create_slider(initial, min, max, step, on_change_snap, &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Reactive: write the controlled value back to the widget
    // whenever the signal changes.
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let v = value.get();
            backend.borrow_mut().update_slider_value(&node, v);
        });
    }
    if let Some(RefFill::Slider(fill)) = ref_fill {
        let handle = backend.borrow().make_slider_handle(&n);
        fill(handle);
    }
    n
}
