//! `Element::Pressable` build path.
//!
//! Backend creates a bare tappable container with the click handler
//! bound. Children are inserted just like View — the visual is
//! entirely subtree-driven, no UA chrome (no `<button>` border on
//! web; no system styling on native). Same `attach_style` /
//! `attach_disabled` wiring as Button so the state machinery
//! (`hovered`/`pressed`/`disabled`) applies identically.

use super::debug::time_backend_create;
use super::style::{attach_disabled, attach_style};
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
    on_click: Rc<dyn Fn()>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    disabled: Option<Box<dyn Fn() -> bool>>,
    a11y: AccessibilityProps,
) -> B::Node {
    let mut n = time_backend_create(pkind!(Pressable), || {
        backend.borrow_mut().create_pressable(on_click, &a11y)
    });
    insert_children(backend, &mut n, children);
    let state_setter = style.map(|s| attach_style(backend, &n, s));
    if let Some(RefFill::Pressable(fill)) = ref_fill {
        let handle = backend.borrow().make_pressable_handle(&n);
        fill(handle);
    }
    if let Some(d) = disabled {
        attach_disabled(backend, &n, d, state_setter);
    }
    n
}
