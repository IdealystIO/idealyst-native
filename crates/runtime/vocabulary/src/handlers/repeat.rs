//! The `repeat` multi-node handler — port of the old walker's
//! batched-Repeat expansion (`walker/view.rs::splice_children`'s
//! `Element::Repeat` arm + `try_build_repeat_batched` + `enqueue_primitive`).
//!
//! Two paths, decided per expansion (all-or-nothing, like the old core):
//!
//! - **Batched** (`BatchOps::supports_batched_repeat`): every row matches
//!   the batchable shape (View + Text + static-sheet style, no other
//!   primitives, no events/refs/reactive bindings) → the whole expansion
//!   is enqueued as `BatchOp`s and submitted in ONE
//!   `execute_batch_with_attach` round-trip; styled rows share ONE bulk
//!   theme-cohort entry + ONE teardown probe (the old
//!   `register_static_cohort_batch`). No per-row `LiveNode` machinery
//!   beyond a flat node list — mount AND teardown are O(1) in reactive
//!   bookkeeping.
//! - **Fallback**: rows realize per-node through the normal registry
//!   handlers (full per-row work: style attach, effects, cleanups), then
//!   attach with one `Host::insert_many` — the old walker's fallback,
//!   op-for-op.
//!
//! ## Batchable shape (V1 — the old walker's exact condition set)
//!
//! - Row is a `view` with no `safe_area`, no `on_touch`/`on_wheel`/
//!   `on_hover`/`on_file_drop`, no `preserves_focus`, no `is_container`,
//!   no `ref_fill`, and style `None` or a static sheet application
//!   (`StyleProp::Sheet`) with NO state overlays.
//! - Children are exclusively `text` with `Value::Const` content, no
//!   style, no `ref_fill` — or nested batchable views.
//!
//! Anything else fails the check and the WHOLE expansion takes the
//! fallback ("no mixed batched/per-call within one expansion" — the old
//! rule). Matching the old conditions exactly is deliberate: the
//! scene-parity goldens stay shared because both cores make the same
//! batching decision for the same tree (see that crate's README).
//!
//! Like the old core, batched rows skip robot registration and a11y
//! props (the batch ops don't carry them) — the batchable shape is
//! static text rows, where both are inert in practice.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{resolve_style, BackendBatch, BatchOp, StyleApplication};
use runtime_scene::{Element, LiveNode, MountCx, Registry};
use runtime_world::Value;

use crate::caps::BatchOps;
use crate::prims::{PrimCell, RepeatPrim, TextPrim, TextSourceProp, ViewPrim};
use crate::style_attach::{
    apply_sheet, on_teardown, resolve_state_overlays, StyleProp, StyleServices,
};
use crate::theme;

/// Register the `repeat` multi-node handler (dispatched by
/// [`runtime_scene::Element::Many`]).
pub fn register_repeat<H>(registry: &mut Registry<H>)
where
    H: BatchOps + StyleServices + 'static,
{
    registry.register_many::<PrimCell<RepeatPrim>, _>(|cx, cell, parent| {
        mount_repeat(cx, cell.take(), parent)
    });
}

/// Mount a repeat expansion into `parent`. Returns the live subtree and
/// the number of top-level nodes contributed (the splice loop's
/// `inserted` accounting).
pub fn mount_repeat<H>(
    cx: &mut MountCx<'_, H>,
    prim: RepeatPrim,
    parent: &mut H::Node,
) -> (LiveNode<H::Node>, usize)
where
    H: BatchOps + StyleServices,
{
    let RepeatPrim { count, row_builder } = prim;
    // Empty repeat: nothing to do, no FFI (old walker's early-out).
    if count == 0 {
        return (LiveNode::Fragment(Vec::new()), 0);
    }

    let backend = cx.backend().clone();
    if backend.borrow().supports_batched_repeat() {
        if let Some(result) = try_mount_batched(&backend, parent, count, &row_builder) {
            return result;
        }
    }

    // Fallback: build every row eagerly through the registry (full
    // per-row mounts — each row's own subtree does its own backend
    // calls), then hand the lot to the backend for one batched insert.
    let mut lives: Vec<LiveNode<H::Node>> = Vec::with_capacity(count);
    let mut nodes: Vec<H::Node> = Vec::with_capacity(count);
    for i in 0..count {
        let live = cx.realize_in_place(row_builder(i));
        nodes.extend(live.collect_nodes());
        lives.push(live);
    }
    let total = nodes.len();
    backend.borrow_mut().insert_many(parent, nodes);
    (LiveNode::Fragment(lives), total)
}

/// The batched path. `None` = some row failed the batchable-shape check;
/// the caller falls back (the partial batch is discarded — no partial
/// batches, the old rule).
fn try_mount_batched<H>(
    backend: &Rc<RefCell<H>>,
    parent: &mut H::Node,
    count: usize,
    row_builder: &dyn Fn(usize) -> Element,
) -> Option<(LiveNode<H::Node>, usize)>
where
    H: BatchOps + StyleServices,
{
    let mut batch = BackendBatch::with_capacity(count * 3, 0);
    // Per-row style attachments, resolved to real nodes after the batch
    // returns and bulk-registered as ONE cohort entry (the dominant
    // old-core mount cost before bulk registration — see
    // `register_static_cohort_batch`'s rationale in walker/style.rs).
    let mut style_attachments: Vec<(u32, StyleApplication)> = Vec::with_capacity(count);
    let mut row_top_ids: Vec<u32> = Vec::with_capacity(count);

    // ThemeCtx captured ONCE for the whole expansion (the per-node
    // capture is already a bulk-create win; per-expansion is strictly
    // better for 10k identical rows).
    let ctx = theme::theme_ctx();
    // Per-expansion mint memo: rows of one repeat share a handful of
    // resolved styles (the bench's parity alternation = 2), and the
    // seeded resolve fast path returns POINTER-stable `Rc<StyleRules>`
    // per (sheet, variants) — so a tiny ptr-keyed list drops the
    // per-row `mint_style_class` backend lookup (~10k hash probes per
    // 10k-row expansion) to one per distinct style.
    let mut minted: Vec<(*const runtime_core::StyleRules, String)> = Vec::new();

    #[cfg(feature = "debug-stats")]
    let _t_enqueue = runtime_core::debug::now_micros();
    for i in 0..count {
        #[cfg(feature = "debug-stats")]
        let _t_row = runtime_core::debug::now_micros();
        let row = row_builder(i);
        #[cfg(feature = "debug-stats")]
        runtime_core::debug::record_apply_phase(
            "nc_repeat_row_build",
            runtime_core::debug::now_micros().saturating_sub(_t_row),
        );
        let queued = enqueue_element(
            backend,
            &mut batch,
            row,
            &mut style_attachments,
            &ctx,
            &mut minted,
        )?;
        row_top_ids.push(queued);
    }
    #[cfg(feature = "debug-stats")]
    runtime_core::debug::record_apply_phase(
        "nc_repeat_enqueue_loop",
        runtime_core::debug::now_micros().saturating_sub(_t_enqueue),
    );

    // Submit batch AND attach row tops to parent in one backend call
    // (web folds both into a single FFI — see the cap's docs).
    #[cfg(feature = "debug-stats")]
    let _t_exec = runtime_core::debug::now_micros();
    let nodes = backend
        .borrow_mut()
        .execute_batch_with_attach(batch, parent, &row_top_ids);
    #[cfg(feature = "debug-stats")]
    runtime_core::debug::record_apply_phase(
        "nc_repeat_execute_and_attach",
        runtime_core::debug::now_micros().saturating_sub(_t_exec),
    );

    // Bulk cohort registration: ONE slab entry + ONE teardown probe for
    // every styled row, regardless of N.
    #[cfg(feature = "debug-stats")]
    let _t_cohort = runtime_core::debug::now_micros();
    let mut members: Vec<(H::Node, StyleApplication)> = Vec::with_capacity(style_attachments.len());
    for (local_id, app) in style_attachments {
        members.push((nodes[local_id as usize].clone(), app));
    }
    register_static_cohort_batch(backend, members);
    #[cfg(feature = "debug-stats")]
    runtime_core::debug::record_apply_phase(
        "nc_repeat_cohort_bulk",
        runtime_core::debug::now_micros().saturating_sub(_t_cohort),
    );

    // The live tree is an EMPTY fragment on purpose: batched rows carry
    // no per-node live structure, and retaining a per-row
    // `LiveNode::Item` would cost 10k `Node` handle clones at mount +
    // 10k drops at teardown (measured ~2× old-core teardown on the
    // bench gate) for nodes that can never be individually detached —
    // a Many is children-list-only (detached-root mount panics), so
    // structural teardown ALWAYS removes an ancestor node, exactly the
    // old `Element::Repeat` contract. Only the COUNT feeds the splice
    // accounting.
    Some((LiveNode::Fragment(Vec::new()), row_top_ids.len()))
}

/// Walk one row subtree and push `BatchOp`s. Returns the row top's
/// `local_id`, or `None` on any non-batchable shape (whole batch is then
/// discarded by the caller). Mirrors `walker/view.rs::enqueue_primitive`
/// condition-for-condition.
fn enqueue_element<H>(
    backend: &Rc<RefCell<H>>,
    batch: &mut BackendBatch,
    element: Element,
    style_attachments: &mut Vec<(u32, StyleApplication)>,
    ctx: &theme::ThemeCtx,
    minted: &mut Vec<(*const runtime_core::StyleRules, String)>,
) -> Option<u32>
where
    H: BatchOps + StyleServices,
{
    let Element::Item { data, children } = element else {
        // Fragments, holes, keyed lists, nested repeats, component
        // scopes: all break out (the old walker's `_ => None`).
        return None;
    };
    // Try `text` first (the common leaf), then `view`.
    let data = match data.downcast::<PrimCell<TextPrim>>() {
        Ok(cell) => {
            let prim = cell.take();
            if prim.ref_fill.is_some() || prim.style.is_some() {
                return None;
            }
            let content = match prim.content {
                TextSourceProp::Value(Value::Const(s)) => s,
                // Reactive / styled-run / JS-binding sources need the
                // per-node effect setup only the text handler wires up.
                _ => return None,
            };
            let id = batch.next_id();
            batch.ops.push(BatchOp::CreateText {
                local_id: id,
                content,
            });
            return Some(id);
        }
        Err(data) => data,
    };
    let Ok(cell) = data.downcast::<PrimCell<ViewPrim>>() else {
        return None;
    };
    let prim = cell.take();
    if prim.ref_fill.is_some()
        || prim.on_touch.is_some()
        || prim.on_wheel.is_some()
        || prim.on_hover.is_some()
        || prim.on_file_drop.is_some()
        || prim.preserves_focus
        || prim.is_container
        || !prim.safe_area.is_empty()
    {
        return None;
    }

    // For a styled view, resolve & mint the class up front so the batch
    // ships a class-name string. Only the static-sheet arm batches; the
    // old walker's exact set (reactive/signal-class/preminted bail).
    let resolved_class: Option<(String, Rc<runtime_core::StyleRules>, StyleApplication)> =
        match prim.style {
            None => None,
            Some(StyleProp::Sheet(app)) => {
                // Registration fast path (identical to `apply_sheet`'s):
                // 10k identical rows register once.
                if !ctx.sheet_is_registered(&app.sheet) {
                    theme::ensure_sheet_registered(backend, &app.sheet);
                }
                // State overlays force per-node dynamic classes — bail
                // to per-call (old walker's overlay check).
                if !resolve_state_overlays(&app).is_empty() {
                    return None;
                }
                let resolved = resolve_style(&app);
                // Expansion-local mint memo (see `try_mount_batched`):
                // the resolve fast path returns pointer-stable rules per
                // (sheet, variants), so a linear scan of the few
                // distinct styles beats a backend hash probe per row.
                let ptr = Rc::as_ptr(&resolved);
                let class = match minted.iter().find(|(p, _)| *p == ptr) {
                    Some((_, class)) => class.clone(),
                    None => {
                        let class = backend.borrow_mut().mint_style_class(&resolved)?;
                        minted.push((ptr, class.clone()));
                        class
                    }
                };
                Some((class, resolved, app))
            }
            // Raw rules / dynamic closures / signal-class / preminted:
            // per-call path (the old walker batched only
            // `StyleSource::Static` sheet applications).
            Some(_) => return None,
        };

    let view_id = batch.next_id();
    batch.ops.push(BatchOp::CreateView { local_id: view_id });

    for child in children {
        let child_id = enqueue_element(backend, batch, child, style_attachments, ctx, minted)?;
        batch.ops.push(BatchOp::Insert {
            parent: view_id,
            child: child_id,
        });
    }

    if let Some((class_name, rules, app)) = resolved_class {
        batch.ops.push(BatchOp::ApplyStyleStatic {
            node: view_id,
            class_name,
            rules,
        });
        style_attachments.push((view_id, app));
    }
    Some(view_id)
}

/// Bulk theme-cohort registration for a batch's styled rows — port of
/// `walker/style.rs::register_static_cohort_batch`: ONE cohort entry
/// re-applying every member on theme change, ONE teardown probe
/// (cohort unregister FIRST, then `on_node_unstyled` per member — the
/// `BulkStyleHandle::drop` order).
fn register_static_cohort_batch<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    members: Vec<(H::Node, StyleApplication)>,
) {
    if members.is_empty() {
        return;
    }
    theme::ensure_theme_driver(backend);
    let handles_states_natively = backend.borrow().handles_states_natively();
    let ctx = theme::theme_ctx();

    // Single Rc-wrapped Vec shared between the cohort reapply closure
    // and the teardown probe (the old code's rationale: no per-row Rc
    // wrapping, `apply_sheet` takes `&StyleApplication`).
    let members: Rc<Vec<(H::Node, StyleApplication)>> = Rc::new(members);
    let members_for_cohort = members.clone();
    let backend_for_cohort = backend.clone();
    let ctx_for_cohort = ctx.clone();
    let cohort_id = theme::cohort_register(Rc::new(move || {
        for (node, app) in members_for_cohort.iter() {
            // Batched rows carry no container ancestor (flat, simple
            // rows) — container overlays resolve at width 0 (inert),
            // same as the old bulk reapply.
            apply_sheet(
                &backend_for_cohort,
                node,
                app,
                handles_states_natively,
                &ctx_for_cohort,
            );
        }
    }));

    on_teardown(move || {
        // Cohort unregister must be SYNCHRONOUS (a theme swap after
        // teardown must not reapply to dead nodes). Per-row
        // `on_node_unstyled` is deliberately NOT emitted here, diverging
        // from the old `BulkStyleHandle::drop` "for symmetry" loop:
        // batched rows never acquire per-node backend style state — the
        // `mint_style_class` cap contract is "without touching any
        // node", and `ApplyStyleStatic` only stamps the shared minted
        // class — so the loop was N guaranteed no-ops costing one
        // `node_id` FFI each (~4 ms at 10k rows, the whole
        // teardown-gate residual). The old core only got it "free"
        // because `install_drop_deferral` pushed the scope drop into
        // rAF slices outside the measured window. A backend that DOES
        // allocate per-node state for batch-applied classes must free
        // it from its own node-release path, not from this hook (see
        // `BatchOps` docs).
        ctx.cohort_unregister(cohort_id);
        // Release the members OFF the synchronous teardown path: the Vec
        // holds N node handles + N StyleApplications, and freeing them
        // (JS handle-table releases + map drops) is the old core's
        // deferred-drop territory (`install_drop_deferral` sliced scope
        // drops into rAF). `after_ms_detached(0, …)` = deferred where a
        // scheduler is installed (web), synchronous fallback elsewhere
        // (unit tests, native without scheduler). Sheet pinning holds
        // until the timer fires — the nodes are gone by then, so the
        // registration Weak may die with them, exactly as before.
        runtime_core::scheduling::after_ms_detached(0, move || drop(members));
    });
}
