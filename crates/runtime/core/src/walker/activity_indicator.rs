//! `Element::ActivityIndicator` build path. Passive widget; a reactive
//! `size` source installs an Effect that resizes the spinner in place.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitives;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    size: primitives::activity_indicator::ActivityIndicatorSize,
    size_fn: Option<Box<dyn Fn() -> primitives::activity_indicator::ActivityIndicatorSize>>,
    color: Option<crate::style::Color>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let n = time_backend_create(pkind!(ActivityIndicator), || {
        backend.borrow_mut().create_activity_indicator(size, color.as_ref(), &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Reactive `size`: a live source installs an Effect that resizes the
    // spinner in place when the closure's signals change (no node
    // rebuild). The node is born at the create-time `size`; a fixed size
    // (`size_fn == None`) installs no effect (the common case).
    if let Some(f) = size_fn {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let s = f();
            backend.borrow_mut().update_activity_indicator_size(&node, s);
        });
    }
    if let Some(RefFill::ActivityIndicator(fill)) = ref_fill {
        let handle = backend.borrow().make_activity_indicator_handle(&n);
        fill(handle);
    }
    n
}
