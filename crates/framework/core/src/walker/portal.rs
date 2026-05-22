//! `Primitive::Portal` build path. Same lifecycle pattern as
//! `Overlay`/`AnchoredOverlay`: backend stands up the platform-native
//! render-elsewhere mount, framework inserts children, attaches
//! style, wires the optional ref, and installs an RAII cleanup that
//! hits `release_portal` when the surrounding scope drops (host's
//! open-state signal flipped, parent rebuilds, owner teardown).

use super::cleanup::PortalHandleCleanup;
use super::debug::time_backend_create;
use super::style::attach_style;
use super::view::insert_children;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitive::Primitive;
use crate::primitives;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Primitive>,
    target: primitives::portal::PortalTarget,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let dismiss_for_backend = on_dismiss.clone();
    let mut n = time_backend_create(pkind!(Portal), || {
        backend.borrow_mut().create_portal(
            target,
            dismiss_for_backend,
            trap_focus,
            &a11y,
        )
    });

    insert_children(backend, &mut n, children);

    if let Some(s) = style {
        attach_style(backend, &n, s);
    }

    if let Some(RefFill::Portal(fill)) = ref_fill {
        let handle = backend.borrow().make_portal_handle(&n);
        fill(handle);
    }

    let cleanup = PortalHandleCleanup {
        backend: backend.clone(),
        node: n.clone(),
    };
    let _cleanup_effect = Effect::new(move || {
        let _ = &cleanup;
    });

    n
}
