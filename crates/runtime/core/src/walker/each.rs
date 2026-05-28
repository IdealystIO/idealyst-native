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

/// Anchorless reactive region — splices the region's rows DIRECTLY into
/// `parent` (flat siblings, NO wrapper node) instead of nesting them
/// under a `create_reactive_anchor`. Used for `Primitive::Each` in a
/// children list when the backend reports
/// [`supports_child_splice`](crate::Backend::supports_child_splice).
///
/// On each dependency change the Effect removes exactly the rows it
/// previously inserted (via `remove_child`) and appends the freshly
/// built ones, dropping the old per-region scope first so the prior
/// rows' signals/effects are freed atomically — same teardown contract
/// as the anchored [`build`], minus the wrapper.
///
/// **Position:** rows are spliced at `base_index` — the count of the
/// parent's children that precede the region — via `insert_at`, so a
/// region followed by static siblings rebuilds in the right place
/// instead of appending past them. `base_index` is captured at build
/// time and is stable as long as the content BEFORE the region is
/// static (the common case); multiple dynamic regions sharing a parent
/// (where an earlier region's row count shifts a later region's base)
/// is a documented follow-up needing marker/coordination. Returns the
/// region's INITIAL row count so the caller can advance its own
/// running child index for trailing siblings.
///
/// Style is ignored here (there is no region node to apply it to); a
/// styled `Each` keeps the anchored path.
pub(super) fn build_spliced<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &B::Node,
    base_index: usize,
    build_children: Box<dyn Fn() -> Vec<Primitive>>,
) -> usize {
    let parent = parent.clone();
    let backend_for_effect = backend.clone();

    // Exactly the row nodes this region inserted on its last run, so the
    // next run can remove just those (leaving siblings untouched). Kept
    // outside the Effect too so we can read the initial count to return.
    let prev_nodes: Rc<RefCell<Vec<B::Node>>> = Rc::new(RefCell::new(Vec::new()));
    let prev_for_effect = prev_nodes.clone();
    // The region's nested scope; replaced atomically each rebuild.
    let list_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let build_children = Rc::new(build_children);

    let _e = Effect::new(move || {
        // TRACKED: build the children list — reads the region's
        // signal(s), which become its rebuild dependencies. A region
        // that reads nothing simply never re-fires (static).
        let children = (build_children)();

        // Unmount the previous run's rows (just ours), then drop the old
        // scope so their signals/effects are freed.
        {
            let prev = prev_for_effect.borrow();
            let mut b = backend_for_effect.borrow_mut();
            for n in prev.iter() {
                b.remove_child(&parent, n);
            }
        }
        prev_for_effect.borrow_mut().clear();
        *list_scope.borrow_mut() = None;

        // UNTRACKED + scoped: build each row and splice it into `parent`
        // at the region's position. `insert_at(base_index + i)` lands
        // the rows before any trailing siblings; on the first run the
        // parent has exactly `base_index` children, so it behaves as an
        // append. Inner reactive constructs install their own effects
        // here and track their own deps — not this region.
        let mut new_scope = Box::new(reactive::Scope::new());
        let mut new_nodes: Vec<B::Node> = Vec::new();
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                for (i, prim) in children.into_iter().enumerate() {
                    let node = super::build(&backend_for_effect, i as u32, prim);
                    let mut p = parent.clone();
                    backend_for_effect
                        .borrow_mut()
                        .insert_at(&mut p, node.clone(), base_index + i);
                    new_nodes.push(node);
                }
            });
        });
        *prev_for_effect.borrow_mut() = new_nodes;
        *list_scope.borrow_mut() = Some(new_scope);
    });

    // The Effect ran once synchronously above, so `prev_nodes` now holds
    // the initial rows — their count is this region's contribution to
    // the parent's child index.
    let count = prev_nodes.borrow().len();
    count
}
