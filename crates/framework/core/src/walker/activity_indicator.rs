//! `Primitive::ActivityIndicator` build path. Passive widget with
//! no reactive props; only style and ref need wiring after create.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitives;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    size: primitives::activity_indicator::ActivityIndicatorSize,
    color: Option<crate::style::Color>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    let n = time_backend_create(pkind!(ActivityIndicator), || {
        backend.borrow_mut().create_activity_indicator(size, color.as_ref())
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(RefFill::ActivityIndicator(fill)) = ref_fill {
        let handle = backend.borrow().make_activity_indicator_handle(&n);
        fill(handle);
    }
    n
}
