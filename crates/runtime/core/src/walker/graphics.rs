//! `Element::Graphics` build path. Installs an unconditional
//! cleanup hook — an empty Effect capturing a
//! `GraphicsHandleCleanup` whose Drop calls `release_graphics`.
//! Independent of the style effect so unstyled Graphics still get
//! torn down.

use super::cleanup::GraphicsHandleCleanup;
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

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    on_ready: primitives::graphics::OnReady,
    on_resize: primitives::graphics::OnResize,
    on_lost: primitives::graphics::OnLost,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let n = time_backend_create(pkind!(Graphics), || {
        backend.borrow_mut().create_graphics(on_ready, on_resize, on_lost, &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Install an unconditional cleanup hook. The empty Effect
    // captures a `GraphicsHandleCleanup` whose Drop calls
    // `release_graphics`. Independent of the style effect so
    // unstyled Graphics still get torn down. Same scope-drop
    // mechanics: `when()` branch flips, list recycling, and
    // `Owner` drop all cascade through here.
    {
        let cleanup = GraphicsHandleCleanup {
            backend: backend.clone(),
            node: n.clone(),
        };
        let _e = Effect::new(move || {
            let _ = &cleanup.node;
        });
    }
    if let Some(RefFill::Graphics(fill)) = ref_fill {
        let handle = backend.borrow().make_graphics_handle(&n);
        fill(handle);
    }
    n
}
