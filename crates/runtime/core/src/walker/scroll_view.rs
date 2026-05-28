//! `Element::ScrollView` build path. The safe-area opt-in routes
//! through `attach_scroll_view_safe_area_inset` (contentInset
//! semantics) instead of the `attach_safe_area` outer-padding mode
//! Views use — the only place the two paths diverge.

use super::debug::time_backend_create;
use super::style::{attach_scroll_view_safe_area_inset, attach_style};
use super::view::insert_children;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Element>,
    horizontal: bool,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    safe_area_sides: crate::SafeAreaSides,
    on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    a11y: AccessibilityProps,
) -> B::Node {
    let mut n = time_backend_create(pkind!(ScrollView), || {
        backend.borrow_mut().create_scroll_view(horizontal, on_scroll, &a11y)
    });
    insert_children(backend, &mut n, children);
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // ScrollViews route safe-area opt-in through the
    // *contentInset* path rather than padding the scroll view
    // itself. The scroll surface keeps its background
    // edge-to-edge; the content origin insets by the
    // safe-area amount and the user can scroll *under* the
    // bar (the iOS-native scroll pattern). Views use
    // `attach_safe_area` (outer padding); this is the only
    // place the two paths diverge.
    if !safe_area_sides.is_empty() {
        attach_scroll_view_safe_area_inset(backend, &n, safe_area_sides);
    }
    if let Some(RefFill::ScrollView(fill)) = ref_fill {
        let handle = backend.borrow().make_scroll_view_handle(&n);
        fill(handle);
    }
    n
}
