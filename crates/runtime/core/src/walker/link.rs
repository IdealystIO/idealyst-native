//! `Element::Link` build path. The walker builds the
//! `on_activate` callable (route lookup + URL emit + param
//! rebuilds) and hands the backend a `LinkConfig`. Children are
//! inserted just like View into the link's native container.

use super::debug::time_backend_create;
use super::style::attach_style;
use super::view::insert_children;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Element>,
    route: &'static str,
    url: String,
    url_fn: Option<Box<dyn Fn() -> String>>,
    make_params: Rc<dyn Fn() -> Box<dyn Any>>,
    kind: primitives::link::NavKind,
    target: Option<Rc<primitives::navigator::NavigatorControl>>,
    external: bool,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // External links bypass the navigator entirely: activation hands
    // the URL to the platform's external handler. In-app links build
    // the navigator-dispatch closure as before. (On web, external
    // links navigate via the native `<a target="_blank">`, so the web
    // backend ignores this closure — it only fires on native.)
    let on_activate: Rc<dyn Fn()> = if external {
        let url = url.clone();
        Rc::new(move || crate::backend::open_url(&url))
    } else {
        primitives::link::make_on_activate(
            target,
            route,
            url.clone(),
            kind,
            make_params,
        )
    };
    let config = primitives::link::LinkConfig {
        route,
        url,
        external,
        on_activate,
    };
    let mut n = time_backend_create(pkind!(Link), || {
        backend.borrow_mut().create_link(config, &a11y)
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
    // Reactive `url`: a live source installs an Effect that swaps the
    // `<a href>` in place when the closure's signals change (no node
    // rebuild). The node is born at the closure's initial value; a fixed
    // `url` (`url_fn == None`) installs no effect (the common case).
    if let Some(f) = url_fn {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let u = f();
            backend.borrow_mut().update_link_url(&node, &u);
        });
    }
    if let Some(RefFill::Link(fill)) = ref_fill {
        let handle = backend.borrow().make_link_handle(&n);
        fill(handle);
    }
    n
}
