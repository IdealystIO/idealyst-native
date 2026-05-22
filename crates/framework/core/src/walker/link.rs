//! `Primitive::Link` build path. The walker builds the
//! `on_activate` callable (route lookup + URL emit + param
//! rebuilds) and hands the backend a `LinkConfig`. Children are
//! inserted just like View into the link's native container.

use super::debug::time_backend_create;
use super::style::attach_style;
use super::view::insert_children;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitive::Primitive;
use crate::primitives;
use crate::sources::StyleSource;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Primitive>,
    route: &'static str,
    url: String,
    make_params: Rc<dyn Fn() -> Box<dyn Any>>,
    kind: primitives::link::NavKind,
    target: Option<Rc<primitives::navigator::NavigatorControl>>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    let on_activate = primitives::link::make_on_activate(
        target,
        route,
        url.clone(),
        kind,
        make_params,
    );
    let config = primitives::link::LinkConfig {
        route,
        url,
        on_activate,
    };
    let mut n = time_backend_create(pkind!(Link), || {
        backend.borrow_mut().create_link(config)
    });
    // Children are built recursively (same shape as View)
    // and inserted into the link's native container. The
    // backend is responsible for making the container
    // tappable / clickable as a whole; children are just
    // visual content.
    insert_children(backend, &mut n, children);
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(RefFill::Link(fill)) = ref_fill {
        let handle = backend.borrow().make_link_handle(&n);
        fill(handle);
    }
    n
}
