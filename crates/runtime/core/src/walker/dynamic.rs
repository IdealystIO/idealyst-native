//! `Element::Dynamic` build path — the generic reactive single-subtree.
//!
//! The whole-element dual of a reactive `text(move || …)` leaf: an effect
//! tracks the build closure's *eager* reads and rebuilds the subtree on
//! every dependency change, dropping the prior nested `Scope` first so its
//! signals/effects free atomically (dispose-on-hide, same model as
//! [`build_when`](super::when_switch::build_when)).
//!
//! Simpler than `When`/`Switch`: there is no condition/scrutinee to dedup
//! against. The dependency source *is* the build closure, so it runs
//! **tracked** (its eager signal reads become rebuild deps) while the
//! backend node construction runs **untracked** (so it doesn't subscribe
//! the outer effect to incidental reads) — the same tracked-enumerate /
//! untracked-build split `Each` uses.

use super::debug::time_backend_create;
use crate::backend::Backend;
use crate::element::Element;
use crate::reactive::{self, untrack_for_build, Effect};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build_dynamic<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    build: Box<dyn Fn() -> Element>,
) -> B::Node {
    let placeholder = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    // The subtree's nested scope, replaced atomically on each rebuild so
    // the prior subtree's signals + effects free before the new one mounts.
    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();

    // Capture the ambient navigator context ONCE while the screen's guards
    // are on the stack; re-establish it around every rebuild so a `link`
    // inside a reactively-remounted subtree keeps its navigator. See
    // `when_switch::build_when_closure` for the full rationale.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    let _e = Effect::new(move || {
        let hydrating = backend_for_effect.borrow().is_hydrating();

        // Drop the prior subtree's scope and clear the anchor before
        // rebuilding. Skipped on the first fire during hydration: there is
        // no previous scope and we must NOT wipe the SSR children the build
        // below adopts via the hydration cursor (mirrors `build_when_closure`).
        if !hydrating {
            *branch_scope_for_effect.borrow_mut() = None;
            backend_for_effect
                .borrow_mut()
                .clear_children(&placeholder_for_effect);
        }

        let mut new_scope = Box::new(reactive::Scope::new());
        // Nav context alive for the whole build so links built inside
        // capture the restored navigator; dropped at end of this fire.
        let _nav_restore = nav_ctx.enter();
        reactive::with_scope(&mut new_scope, || {
            // TRACKED: eager signal reads in the user's closure subscribe
            // THIS effect, so a change to any of them rebuilds the subtree.
            let element = (build)();
            // UNTRACKED: backend node construction (and any incidental reads
            // it performs, e.g. style resolution) must not add rebuild deps —
            // only the closure's own reads should. Inner reactive constructs
            // (nested `text(move || …)`, `when`) defer their reads to their
            // own effects created here and are unaffected.
            untrack_for_build(|| {
                let child_node = super::build(&backend_for_effect, 0, element);
                let mut placeholder_mut = placeholder_for_effect.clone();
                backend_for_effect
                    .borrow_mut()
                    .insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    placeholder
}
