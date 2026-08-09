//! `virtual_grid` handler — the two-axis sibling of
//! `handlers/virtualizer.rs`.
//!
//! The contract is the virtualizer's, widened to a cell coordinate;
//! read that module's header for the full rationale on lazy
//! realization, detached rows, and per-item ownership scopes. What
//! differs here:
//!
//! - **Cells are keyed by `(col, row)`**, not a flat index. The
//!   backend's window is a rectangle, so a flat index would have every
//!   engine doing divmod against a column count that can change under
//!   it mid-frame.
//! - **No measured-size cache.** The virtualizer keeps one because a
//!   1-D list reconciles a single axis; a grid cell's measured height
//!   would have to agree with its whole row, so sizes are
//!   author-supplied only (see the primitive's module docs). That
//!   removes the scope-id → key reverse map entirely.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::primitives::virtual_grid::{CellKey, GridCallbacks};
use runtime_scene::{realize, Element, MountCx, Realized, Registry};
use runtime_world::{collect_owned, effect, untrack, Owned};

use crate::caps::GridOps;
use crate::prims::{PrimCell, VirtualGridPrim};
use crate::style_attach::{attach_style, on_teardown, StyleServices};

/// Register the `virtual_grid` handler (called from `register_builtins`).
pub fn register_virtual_grid<H>(registry: &mut Registry<H>)
where
    H: GridOps + StyleServices + 'static,
{
    registry.register::<PrimCell<VirtualGridPrim>, _>(|cx, p, children| {
        mount_virtual_grid(cx, p.take(), children)
    });
}

/// One mounted cell's ownership: the realized subtree PLUS the scope
/// that collected creations made while *constructing* the cell
/// element. Same two-part shape (and the same leak it prevents) as the
/// virtualizer's `RowScope`.
struct CellScope<N> {
    _realized: Realized<N>,
    _extra: Owned,
}

/// Mount a `virtual_grid`.
///
/// Sequence mirrors the virtualizer exactly — build the callback
/// bundle → `create_virtual_grid` → data effect (first fire at mount)
/// → attach_style → teardown probe → ref-fill — so teardown order is
/// the same on both primitives.
pub fn mount_virtual_grid<H>(
    cx: &mut MountCx<'_, H>,
    prim: VirtualGridPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: GridOps + StyleServices,
{
    let backend = cx.backend().clone();
    let registry: Rc<Registry<H>> = cx.registry().clone();

    let scopes: Rc<RefCell<HashMap<u64, CellScope<H::Node>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    let col_count: Rc<dyn Fn() -> usize> = Rc::from(prim.col_count);
    let row_count: Rc<dyn Fn() -> usize> = Rc::from(prim.row_count);

    // mount_cell: realize the cell for `(col, row)` detached, inside a
    // fresh ownership scope.
    let mount_cell: Rc<dyn Fn(usize, usize) -> (H::Node, u64)> = {
        let backend = backend.clone();
        let registry = registry.clone();
        let render = prim.render_cell.clone();
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        Rc::new(move |col, row| {
            // Element construction runs INSIDE the cell's collector and
            // untracked — only the backend's window math decides cell
            // lifetime, never a stray signal read during construction.
            let ((node, realized), extra) = collect_owned(|| {
                let element = untrack(|| render(col, row));
                let realized = realize(&backend, &registry, element);
                let mut nodes = realized.collect_nodes();
                let node = match nodes.len() {
                    1 => nodes.pop().expect("len checked"),
                    n => panic!(
                        "virtual_grid render_cell({col}, {row}) must produce a single-root \
                         element (got {n} top-level nodes) — wrap fragment cells in an item"
                    ),
                };
                (node, realized)
            });
            let id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(
                id,
                CellScope {
                    _realized: realized,
                    _extra: extra,
                },
            );
            (node, id)
        })
    };

    // release_cell: drop the cell scope (frees every signal/effect
    // inside it). Taken OUT of the map before dropping so cell
    // cleanups can never observe a held map borrow.
    let release_cell: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            let cell = scopes.borrow_mut().remove(&id);
            drop(cell);
        })
    };

    let cell_key: Rc<dyn Fn(usize, usize) -> CellKey> = prim.cell_key.clone();

    let callbacks = GridCallbacks {
        col_count: col_count.clone(),
        row_count: row_count.clone(),
        col_width: prim.col_width.clone(),
        row_height: prim.row_height.clone(),
        cell_key,
        mount_cell,
        release_cell,
        on_scroll: prim.on_scroll,
    };

    let node = backend
        .borrow_mut()
        .create_virtual_grid(callbacks, prim.overscan, &prim.a11y);

    // Data effect: touching BOTH counts subscribes to whatever signals
    // either closure reads, so a change to rows or columns re-queries
    // metrics and re-diffs the mounted set. Reading both unconditionally
    // matters — short-circuiting on one would silently drop the other
    // axis's dependency.
    {
        let backend = backend.clone();
        let node = node.clone();
        let cols = col_count;
        let rows = row_count;
        let _data = effect(move || {
            let _ = cols();
            let _ = rows();
            backend.borrow_mut().virtual_grid_data_changed(&node);
        });
    }

    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }

    // Teardown: drop backend listeners + callback handles so queued
    // scroll/resize events can't call into freed per-cell scopes.
    // Registered AFTER attach_style so teardown order matches the
    // virtualizer's (unstyle first, then release).
    {
        let b = backend.clone();
        let n = node.clone();
        on_teardown(move || {
            b.borrow_mut().release_virtual_grid(&n);
        });
    }

    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_virtual_grid_handle(&node);
        fill(handle);
    }

    node
}
