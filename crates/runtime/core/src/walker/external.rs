//! `Element::External` build path. Backend's
//! `create_external` consults its `ExternalRegistry` to dispatch on
//! `type_id`; unregistered kinds render a platform-native
//! "not supported" placeholder. The cleanup guard mirrors the
//! portal/virtualizer pattern so third-party primitives get
//! scope-tied teardown without per-handler boilerplate.

use super::cleanup::ExternalHandleCleanup;
use super::debug::time_backend_create;
use super::style::attach_style;
use super::view::insert_children;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::element::Element;
use crate::handles::RefFill;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    type_id: TypeId,
    type_name: &'static str,
    payload: Rc<dyn Any>,
    children: Vec<Element>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Adopt-sentinel interception (runtime-server wire client): when a
    // `build_detached` call staged an adopt node whose `TypeId` matches
    // this External's `type_id`, return that pre-built node instead of
    // calling `create_external`. The SDK's `leading_slot` stamps a
    // marker-typed `Element::External`; `dev-client` passes its holder
    // node as the adopt. We adopt the node and skip create_external +
    // children + style + cleanup Effect — the holder is already a live
    // backend node the wire sidebar subtree is inserted into. Reader and
    // writer (`build_detached`) both live in runtime-core, so the
    // thread-local stays coherent across wasm-split chunks (the prior
    // cross-crate-global hand-off did not). See walker.rs module doc.
    if let Some((adopt_tid, adopt_any)) = super::current_adopt() {
        if adopt_tid == type_id {
            if let Ok(node_rc) = adopt_any.downcast::<B::Node>() {
                return (*node_rc).clone();
            }
        }
    }

    let mut n = time_backend_create(pkind!(External), || {
        backend
            .borrow_mut()
            .create_external(type_id, type_name, &payload, &a11y)
    });

    // The handler's returned node is the parent: a leaf widget supplies
    // no children, a container kind (web `<form>`) supplies inputs that
    // must be real backend descendants. Mirrors the `Portal` build path.
    insert_children(backend, &mut n, children);

    if let Some(s) = style {
        attach_style(backend, &n, s);
    }

    // External ref-fill hands the closure an `Rc<dyn Any>`
    // wrapping the backend node. Third-party facades downcast
    // inside to build their `ExternalHandle<T>`.
    if let Some(RefFill::External(fill)) = ref_fill {
        // We don't have a uniform way to type-erase the
        // backend's `Node` here without coupling the trait
        // generics — but `B::Node: Clone + 'static` (a Backend
        // requirement), so we can wrap a clone in `Rc<dyn Any>`.
        let any_node: Rc<dyn Any> = Rc::new(n.clone());
        fill(any_node);
    }

    let cleanup = ExternalHandleCleanup {
        backend: backend.clone(),
        node: n.clone(),
    };
    let _cleanup_effect = Effect::new(move || {
        let _ = &cleanup;
    });

    n
}
