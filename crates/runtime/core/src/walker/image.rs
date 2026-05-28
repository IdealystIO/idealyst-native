//! `Element::Image` build path. Reactive `src` is wrapped in an
//! Effect that calls `update_image_src` when the closure's signal
//! deps change. Asset-backed images register the asset with the
//! backend before `create_image` so the sentinel `"asset://{id}"`
//! the closure returns can be resolved to a real URL.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::assets::{kinds, Asset};
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    src: Box<dyn Fn() -> String>,
    alt: Option<String>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    asset: Option<Asset<kinds::Image>>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Asset-backed images register the asset with the backend
    // *before* `create_image` so the sentinel `"asset://{id}"`
    // the closure returns can be resolved to a real URL. The
    // dedup happens backend-side (web caches `asset_urls` by
    // id), so repeated mounts of the same asset are cheap.
    if let Some(a) = asset {
        backend
            .borrow_mut()
            .register_asset(a.id, a.tag, &a.source);
    }
    let initial = src();
    let n = time_backend_create(pkind!(Image), || {
        backend.borrow_mut().create_image(&initial, alt.as_deref(), &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Reactive src: if `src()` re-reads on subsequent fires,
    // the Effect subscribes and `update_image_src` re-runs.
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let url = src();
            backend.borrow_mut().update_image_src(&node, &url);
        });
    }
    if let Some(RefFill::Image(fill)) = ref_fill {
        let handle = backend.borrow().make_image_handle(&n);
        fill(handle);
    }
    n
}
