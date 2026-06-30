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
    // A bare pressable lowers to a non-form-control node (`<div>` on web,
    // a plain view on native), so a backend's `set_disabled` can't make it
    // inert the way it does a real `<button>`. To block the press uniformly
    // we wrap `on_click` behind a shared flag the disabled Effect drives, and
    // consult it before firing. Covers mouse, keyboard, and programmatic
    // (`PressableHandle::click`) activation since they all route through this
    // one closure. Only allocate the flag when a `disabled` source exists.
    let (on_click, press_block_flag): (Rc<dyn Fn()>, Option<Rc<std::cell::Cell<bool>>>) =
        if disabled.is_some() {
            let flag = Rc::new(std::cell::Cell::new(false));
            let flag_for_click = flag.clone();
            let inner = on_click;
            let wrapped: Rc<dyn Fn()> = Rc::new(move || {
                if !flag_for_click.get() {
                    (inner)();
                }
            });
            (wrapped, Some(flag))
        } else {
            (on_click, None)
        };

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
        attach_disabled(backend, &n, d, state_setter, press_block_flag);
    }
    n
}
