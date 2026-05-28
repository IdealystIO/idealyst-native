//! `Primitive::Each` build path — the reactive, full-rebuild list.
//!
//! Mirrors [`super::when_switch::build_when`]'s closure form: a stable
//! `create_reactive_anchor` whose children are swapped wholesale every
//! time a signal read by `build` changes. The prior list's nested
//! `Scope` is dropped first (freeing every signal/effect in the old
//! rows atomically), the anchor's children cleared, then the new list
//! built inside a fresh scope.
//!
//! Tracking split (same as `when`/`switch`): `build()` runs in the
//! Effect's tracked region so the iterable's signal reads become
//! rebuild dependencies; the actual backend node creation
//! (`insert_children` → `super::build`) runs untracked + scoped so
//! reactive constructs *inside* rows subscribe to their own deps via
//! their own effects rather than pinning the whole list to a rebuild.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::backend::Backend;
use crate::primitive::Primitive;
use crate::reactive::{self, untrack, Effect};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    build_children: Box<dyn Fn() -> Vec<Primitive>>,
    style: Option<StyleSource>,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    if let Some(s) = style {
        attach_style(backend, &anchor, s);
    }

    let backend_for_effect = backend.clone();
    let anchor_for_effect = anchor.clone();

    // The list's nested scope lives across effect re-runs.
    // Rc<RefCell<Option<…>>> so we can replace it atomically on each
    // rebuild — drop the old (freeing its rows' reactive slots) before
    // building the new.
    let list_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let list_scope_for_effect = list_scope.clone();

    let build_children = Rc::new(build_children);

    let _e = Effect::new(move || {
        // TRACKED: construct the children list. Reads the iterable's
        // signal(s) — that's the dependency set we want. Only data is
        // produced here (Primitive values); no backend nodes yet.
        let children = (build_children)();

        // Drop the previous list's scope, then clear the anchor — the
        // old rows are unmounted atomically before the new ones build.
        *list_scope_for_effect.borrow_mut() = None;
        backend_for_effect
            .borrow_mut()
            .clear_children(&anchor_for_effect);

        // UNTRACKED + scoped: materialize the rows. `insert_children`
        // expands any `Repeat` and inserts each child as a flat sibling
        // of the anchor. Inner reactive constructs set up their own
        // effects here, subscribing to their own deps — not this outer
        // region.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                let mut anchor_mut = anchor_for_effect.clone();
                super::view::insert_children(&backend_for_effect, &mut anchor_mut, children);
            });
        });
        *list_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    anchor
}
