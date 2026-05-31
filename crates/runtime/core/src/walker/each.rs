//! `Element::Each` build path — the keyed reactive list.
//!
//! Each row carries a stable key (the `for … , key = …` clause). On
//! every change to a signal read while *enumerating* the rows, the
//! `Effect` re-runs the list's [`EachSnapshot`] — cheap: it produces one
//! `(key, deferred-builder)` pair per item, building no rows yet. The
//! reconciler then diffs the new keys against the previously mounted set
//! ([`reconcile`]):
//!
//! - **unchanged key** → keep the row's backend nodes AND its render
//!   scope (so component-local signals/effects inside the row survive);
//!   the row's builder thunk is dropped unused,
//! - **removed key** → unmount its nodes and drop its scope (freeing
//!   that row's reactive slots), leaving siblings untouched,
//! - **new key** → build the row in its own fresh scope,
//! - finally the surviving + new rows are placed into the new order.
//!
//! This needs the backend to support child splicing
//! ([`Backend::supports_child_splice`](crate::Backend::supports_child_splice))
//! — `remove_child` to drop one row and `insert_at` to move/insert one.
//! On a backend without it (native, today) the anchored [`build`] path
//! falls back to a full rebuild: correct output, but per-row state
//! resets on every change until that backend implements splicing. The
//! anchorless [`build_spliced`] path is only reached when splicing is
//! supported (its caller gates on the capability).
//!
//! Tracking split: `snapshot()` runs in the Effect's tracked region so
//! the iterable's signal reads (and the per-row `key` reads) become the
//! rebuild dependencies; per-row building runs untracked + scoped so
//! reactive constructs *inside* a row subscribe to their own deps rather
//! than pinning the whole list to a rebuild.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::backend::Backend;
use crate::element::{EachKey, EachSnapshot, Element};
use crate::reactive::{self, untrack, Effect};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

/// One mounted row: its render scope (owns the row's signals/effects/
/// cleanups) and the backend nodes the row's builder produced (a row
/// body can be several flat siblings).
struct Entry<B: Backend> {
    scope: Box<reactive::Scope>,
    nodes: Vec<B::Node>,
}

/// Cross-rebuild state for one keyed list: the currently mounted rows in
/// DOM order, plus a monotonic counter handing each newly built row a
/// stable identity slot (so a row's hot-patch ref slots don't collide
/// with a sibling's, and survive reorders).
struct ReconcileState<B: Backend> {
    rows: Vec<(EachKey, Entry<B>)>,
    next_slot: u32,
}

impl<B: Backend> ReconcileState<B> {
    fn new() -> Self {
        ReconcileState { rows: Vec::new(), next_slot: 0 }
    }
}

/// Build one new row in its own scope, returning the scope + the backend
/// nodes its builder produced. Untracked + scoped so the row's internal
/// reactive constructs subscribe to their own deps and the row can later
/// be dropped independently of its siblings.
fn build_entry<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    slot: u32,
    build: crate::element::EachRowBuild,
) -> Entry<B> {
    let mut scope = Box::new(reactive::Scope::new());
    let mut nodes: Vec<B::Node> = Vec::new();
    untrack(|| {
        reactive::with_scope(&mut scope, || {
            for elem in build() {
                nodes.push(super::build(backend, slot, elem));
            }
        });
    });
    Entry { scope, nodes }
}

/// Unmount a removed row: detach its backend nodes, then drop its scope
/// (firing the row's `on_cleanup`s and freeing its signals/effects).
fn unmount<B: Backend + 'static>(backend: &Rc<RefCell<B>>, parent: &B::Node, entry: Entry<B>) {
    let Entry { scope, nodes } = entry;
    {
        let mut b = backend.borrow_mut();
        for node in &nodes {
            b.remove_child(parent, node);
        }
    }
    drop(scope); // fires the row's cleanups + frees its reactive slots
}

fn duplicate_key() {
    // Two rows sharing a key make identity ambiguous — the reconciler
    // can't tell which is "the same" across a rebuild. Surface it loudly
    // in dev; in release, degrade rather than crash a shipped app.
    if cfg!(debug_assertions) {
        panic!(
            "duplicate `key` in a reactive `for`: each row's key must be \
             unique within the list, otherwise rows can't be matched \
             across updates"
        );
    } else {
        eprintln!(
            "idealyst: duplicate `key` in a reactive `for` — row state may \
             reset; give each row a unique key"
        );
    }
}

/// Reconcile the mounted rows under `parent` (starting at child index
/// `base_index`) against the new ordered `rows`, preserving the scope of
/// rows whose key is unchanged. Requires splice support on `backend`.
fn reconcile<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &B::Node,
    base_index: usize,
    rows: Vec<(EachKey, crate::element::EachRowBuild)>,
    state: &Rc<RefCell<ReconcileState<B>>>,
) {
    // Take the prior mounted rows (in DOM order) + slot counter out of
    // the shared state so we can build/drop without holding its borrow
    // across user code (row builders re-enter the reactive system).
    let (old_rows, mut next_slot) = {
        let mut st = state.borrow_mut();
        (std::mem::take(&mut st.rows), st.next_slot)
    };
    let mut old_slots: Vec<Option<(EachKey, Entry<B>)>> =
        old_rows.into_iter().map(Some).collect();

    // PASS A — walk the new order. Reuse the mounted entry when a key is
    // unchanged (keep its scope + nodes, drop the unused builder thunk);
    // otherwise build a fresh row. `reorder` tracks whether survivors
    // kept their relative order: if their old positions stay strictly
    // increasing across the new order, no survivor needs to move.
    let mut new_rows: Vec<(EachKey, Entry<B>, bool)> = Vec::with_capacity(rows.len());
    let mut last_old_pos: isize = -1;
    let mut reorder = false;
    for (key, build) in rows {
        let mut found = None;
        for (i, slot) in old_slots.iter().enumerate() {
            if matches!(slot, Some((k, _)) if *k == key) {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            let (_old_key, entry) = old_slots[i].take().unwrap();
            drop(build); // unchanged row: never rebuilt
            if (i as isize) < last_old_pos {
                reorder = true;
            }
            last_old_pos = i as isize;
            new_rows.push((key, entry, false));
        } else {
            let slot = next_slot;
            next_slot = next_slot.wrapping_add(1);
            let entry = build_entry(backend, slot, build);
            new_rows.push((key, entry, true));
        }
    }

    // PASS B — leftover old slots are removed keys: unmount them. The
    // surviving rows we moved into `new_rows` are untouched.
    for slot in old_slots.iter_mut() {
        if let Some((_k, entry)) = slot.take() {
            unmount(backend, parent, entry);
        }
    }

    // PASS C — place rows into the new order. `insert_at` inserts a new
    // node and *moves* an already-mounted one (DOM `insertBefore`
    // semantics). When survivors kept their relative order (the common
    // add/remove case) we only insert NEW rows — reused rows stay put, so
    // focus / scroll / IME state in another row isn't disturbed. A real
    // reorder repositions every node in target order.
    {
        let mut p = parent.clone();
        let mut b = backend.borrow_mut();
        let mut pos = base_index;
        for (_k, entry, is_new) in &new_rows {
            for node in &entry.nodes {
                if reorder || *is_new {
                    b.insert_at(&mut p, node.clone(), pos);
                }
                pos += 1;
            }
        }
    }

    // PASS D — publish the new ordering as the mounted set, flagging any
    // duplicate key (a usage error).
    {
        let mut st = state.borrow_mut();
        st.next_slot = next_slot;
        st.rows = Vec::with_capacity(new_rows.len());
        for (key, entry, _) in new_rows {
            if st.rows.iter().any(|(k, _)| *k == key) {
                duplicate_key();
            }
            st.rows.push((key, entry));
        }
    }
}

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    snapshot: EachSnapshot,
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
    let snapshot = Rc::new(snapshot);

    // Capture the ambient navigator context ONCE, synchronously, while the
    // screen's guards are still on the stack. Re-established around every
    // row (re)build below so a `link` rebuilt by a list change keeps its
    // navigator (see `walker::when_switch::build_when_closure`). The Effect
    // re-fires after the screen build returned (guards dropped), so without
    // this a remounted-row link captures `None` and silently no-ops. Weak
    // nav ref inside — see `AmbientNavContext`. `Clone` so each (mutually
    // exclusive) branch's Effect can own its own copy.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();
    let nav_ctx_fallback = nav_ctx.clone();

    if backend.borrow().supports_child_splice() {
        // Keyed reconcile under the anchor (the rows are the anchor's
        // children, so `base_index` is 0).
        let state = Rc::new(RefCell::new(ReconcileState::<B>::new()));
        let _e = Effect::new(move || {
            // TRACKED: enumerate the rows. Reads the iterable's signal(s)
            // + each row's key — that's the rebuild dependency set.
            let rows = (snapshot)();
            // Re-establish the ambient nav context for the duration of the
            // reconcile (which builds any new/changed rows synchronously).
            let _nav_restore = nav_ctx.enter();
            reconcile(&backend_for_effect, &anchor_for_effect, 0, rows, &state);
        });
    } else {
        // No splice support → full-rebuild fallback (the pre-keyed
        // behavior): drop the whole list scope, clear the anchor, rebuild
        // every row in a fresh scope. Per-row state is NOT preserved here
        // — that requires `supports_child_splice` (web today; native
        // pending). Kept explicit so the limitation is never a silent
        // wrong result, only a missed optimization.
        let list_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
        let _e = Effect::new(move || {
            let rows = (snapshot)(); // TRACKED
            *list_scope.borrow_mut() = None;
            backend_for_effect.borrow_mut().clear_children(&anchor_for_effect);
            let mut new_scope = Box::new(reactive::Scope::new());
            untrack(|| {
                // Re-establish the ambient nav context for the rebuild.
                let _nav_restore = nav_ctx_fallback.enter();
                reactive::with_scope(&mut new_scope, || {
                    let children: Vec<Element> =
                        rows.into_iter().flat_map(|(_, build)| build()).collect();
                    let mut anchor_mut = anchor_for_effect.clone();
                    super::view::insert_children(&backend_for_effect, &mut anchor_mut, children);
                });
            });
            *list_scope.borrow_mut() = Some(new_scope);
        });
    }

    anchor
}

/// Anchorless keyed reactive region — reconciles the region's rows
/// DIRECTLY into `parent` (flat siblings, NO wrapper node) instead of
/// nesting them under a `create_reactive_anchor`. Used for
/// `Element::Each` in a children list when the backend reports
/// [`supports_child_splice`](crate::Backend::supports_child_splice) (its
/// caller gates on that, so this path always has splice support).
///
/// **Position:** rows are spliced at `base_index` — the count of the
/// parent's children that precede the region — so a region followed by
/// static siblings reconciles in the right place instead of past them.
/// `base_index` is captured at build time and is stable as long as the
/// content BEFORE the region is static (the common case); multiple
/// dynamic regions sharing a parent (where an earlier region's row count
/// shifts a later region's base) is a documented follow-up needing
/// marker/coordination. Returns the region's INITIAL node count so the
/// caller can advance its own running child index for trailing siblings.
///
/// Style is ignored here (there is no region node to apply it to); a
/// styled `Each` keeps the anchored path.
pub(super) fn build_spliced<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &B::Node,
    base_index: usize,
    snapshot: EachSnapshot,
) -> usize {
    let parent = parent.clone();
    let backend_for_effect = backend.clone();
    let snapshot = Rc::new(snapshot);
    let state = Rc::new(RefCell::new(ReconcileState::<B>::new()));
    let count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let count_for_effect = count.clone();

    // Capture the ambient navigator context ONCE, synchronously (guards
    // still on the stack), and re-establish it around every reconcile so
    // a row's `link` rebuilt by a list change keeps its navigator. See
    // `build` above / `build_when_closure` for the full rationale.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    let _e = Effect::new(move || {
        let rows = (snapshot)(); // TRACKED
        let _nav_restore = nav_ctx.enter();
        reconcile(&backend_for_effect, &parent, base_index, rows, &state);
        *count_for_effect.borrow_mut() =
            state.borrow().rows.iter().map(|(_, e)| e.nodes.len()).sum();
    });

    // The Effect ran once synchronously above, so `count` now holds the
    // initial node count — this region's contribution to the parent's
    // child index.
    let c = *count.borrow();
    c
}
