//! `Element::View` build path, including the batched-Repeat
//! shortcut (`try_build_repeat_batched` + `enqueue_primitive`).
//!
//! [`build`] is the entry point invoked by the walker dispatcher; it
//! creates the native container, walks children via [`insert_children`]
//! (which expands `Element::Repeat` inline), then layers on style /
//! safe-area / touch handlers / ref-fill.
//!
//! [`insert_children`] is exported so other primitives with child
//! containers (`Pressable`, `ScrollView`, `Link`, `Portal`) reuse the
//! same expansion path — they get the batched-Repeat fast path for
//! free.

use super::debug::time_backend_create;
use super::style::{attach_safe_area, attach_style, register_static_cohort_batch};
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::batch::{BackendBatch, BatchOp};
use crate::handles::RefFill;
use crate::element::Element;
use crate::sources::{StyleSource, TextSource};
use crate::style::{self, resolve as resolve_style, StyleApplication, StyleRules};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Element>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    safe_area_sides: crate::SafeAreaSides,
    on_touch: Option<crate::TouchHandler>,
    a11y: AccessibilityProps,
) -> B::Node {
    let n = build_view(backend, children, &a11y);
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if !safe_area_sides.is_empty() {
        attach_safe_area(backend, &n, safe_area_sides);
    }
    if let Some(h) = on_touch {
        backend.borrow_mut().install_touch_handler(&n, h);
    }
    if let Some(RefFill::View(fill)) = ref_fill {
        let handle = backend.borrow().make_view_handle(&n);
        fill(handle);
    }
    n
}

fn build_view<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Element>,
    a11y: &AccessibilityProps,
) -> B::Node {
    let mut parent = time_backend_create(pkind!(View), || backend.borrow_mut().create_view(a11y));
    insert_children(backend, &mut parent, children);
    parent
}

/// Walk a children vec and append each child to `parent`. Expands
/// `Element::Repeat` inline: instead of `count` individual `insert`
/// calls, builds all `count` child nodes first and hands them to the
/// backend's `insert_many` for batched DOM insertion (typically via
/// a `DocumentFragment` on web). For non-Repeat children this is the
/// same `build + insert` loop as before.
///
/// Why expand Repeat here and not as a regular Element in the
/// match: Repeat doesn't correspond to a single backend node — it
/// stands for N sibling nodes. So it can only appear inside a
/// children list, never as the root of a subtree.
pub(super) fn insert_children<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &mut B::Node,
    children: Vec<Element>,
) {
    // Running count of backend nodes inserted into `parent` so far. A
    // normal child contributes 1, a `Repeat` contributes `count`, a
    // spliced `Each` contributes its current row count. Anchorless
    // regions capture this as their `base_index` so they splice at the
    // right position relative to preceding (static) siblings.
    let mut inserted: usize = 0;
    for (slot_idx, child) in children.into_iter().enumerate() {
        let slot = slot_idx as u32;
        match child {
            Element::Repeat { count, row_builder } => {
                // Try the batched-Repeat path first: if the backend
                // opts in AND every row matches the batchable shape
                // (View+Text+static-style, no other primitives), we
                // collapse 4N FFI calls into one. On rejection or
                // backend-side opt-out, fall back to the per-call
                // path (the original loop).
                let supports = backend.borrow().supports_batched_repeat();
                if supports {
                    if try_build_repeat_batched(backend, parent, slot, count, &*row_builder) {
                        continue;
                    }
                }
                // Fallback: build every row eagerly, then hand the
                // lot to the backend for one batched insert.
                // Building eagerly means each row's own subtree may
                // have done its own backend FFI calls (createElement
                // etc.) — those can't be batched further at this
                // layer, but the *parent insert* is.
                let mut rows: Vec<B::Node> = Vec::with_capacity(count);
                for i in 0..count {
                    let row_prim = row_builder(i);
                    // Each row gets a distinct identity within the
                    // Repeat's slot: its iteration index. `Repeat` is the
                    // STATIC range lowering (`for i in 0..n`) — built once,
                    // never reconciled, so positional identity is correct
                    // here. Reactive keyed reconciliation is a separate
                    // path: a reactive `for` lowers to a keyed
                    // `Element::Each` (key required at compile time), not
                    // to `Repeat`.
                    let row_id = crate::Identity::node(
                        crate::Identity::node(crate::current_identity(), slot, None, None),
                        i as u32,
                        None,
                        None,
                    );
                    let row_node = crate::with_current_identity(row_id, || {
                        super::build_inner(backend, row_prim)
                    });
                    rows.push(row_node);
                }
                backend.borrow_mut().insert_many(parent, rows);
                inserted += count;
            }
            // Anchorless reactive region: when the backend supports child
            // splicing, a (style-less) `Each` in a children list splices
            // its rows DIRECTLY into `parent` — flat siblings, no wrapper
            // `create_reactive_anchor` node. Styled `Each` (or backends
            // without splice support) fall through to `other`, taking the
            // anchored path that can host the style on the anchor node.
            Element::Each { snapshot, style }
                if style.is_none() && backend.borrow().supports_child_splice() =>
            {
                inserted += super::each::build_spliced(backend, parent, inserted, snapshot);
            }
            // Anchorless reactive conditional: a (style-less) `When` whose
            // backend supports child splicing mounts the active branch's
            // node DIRECTLY into `parent` — no `create_reactive_anchor`
            // wrapper — so an absolutely-positioned branch resolves its
            // containing block against the real parent (matching web's
            // `display:contents` anchor) instead of a collapsed 0×0
            // wrapper view. See `when_switch::build_when_spliced`. A styled
            // `When`, or a backend without splice support, falls through to
            // `other` and takes the anchored path that can host the style.
            Element::When { cond, then, otherwise, style }
                if style.is_none() && backend.borrow().supports_child_splice() =>
            {
                inserted += super::when_switch::build_when_spliced(
                    backend, parent, inserted, cond, then, otherwise,
                );
            }
            other => {
                let child_node = super::build(backend, slot, other);
                backend.borrow_mut().insert(parent, child_node);
                inserted += 1;
            }
        }
    }
}

/// Try the batched-Repeat path for a `Element::Repeat` expansion.
/// Returns `true` if the batch was submitted and the parent inserted;
/// `false` if any row failed the batchable-shape check, in which case
/// the caller falls back to the per-call path.
///
/// Batchable shape (V1):
/// - Row is a `Element::View` with no `safe_area_sides`, no
///   `on_touch`, no `ref_fill`, and a `StyleSource::Static` (or no
///   style at all).
/// - Row's children are exclusively `Element::Text` with a
///   `TextSource::Static`, no `style`, no `ref_fill`.
///
/// Anything else (Button, Image, reactive bindings, state overlays,
/// nested Views, etc.) returns `false` so the whole Repeat takes the
/// fallback. This is "all-or-nothing per Repeat" by design — no
/// mixed batched/per-call within one expansion.
fn try_build_repeat_batched<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &mut B::Node,
    slot: u32,
    count: usize,
    row_builder: &dyn Fn(usize) -> Element,
) -> bool {
    // Empty Repeat: nothing to do, no FFI needed. Report success
    // so the fallback doesn't run.
    if count == 0 {
        return true;
    }

    // Pass 1: build each row's `Element`, check shape, resolve
    // static styles, and queue BatchOps. Each row's reactive setup
    // (theme cohort, cleanup handle adoption) is captured here too
    // — we apply it after the batch returns, against the real
    // nodes.
    let mut batch = BackendBatch::with_capacity(count * 3, 0);
    // Per-row style-attachment work, collected as `(view_local_id,
    // StyleApplication)` pairs. After `execute_batch` returns we
    // resolve the local ids to real Nodes and hand the whole Vec to
    // `register_static_cohort_batch` for **bulk** cohort
    // registration — one slab insert + one guard + one Box, instead
    // of per-row. This is the dominant cost in mount time for large
    // N: pre-bulk, per-row registration was ~88 µs each (heap
    // allocation churn for boxed closures, RAII guards, Rc/Node
    // clones inside the closures).
    let mut style_attachments: Vec<(u32, StyleApplication)> = Vec::with_capacity(count);
    let mut row_top_ids: Vec<u32> = Vec::with_capacity(count);

    let parent_identity = crate::current_identity();
    let slot_identity = crate::Identity::node(parent_identity, slot, None, None);

    // Run the whole enqueue loop under one identity context — the
    // slot's. The batched shape (View + Text + static style, no
    // Effects, no scopes, no event handlers) has no per-row
    // identity-keyed bookkeeping: there's no per-row Effect to attach
    // to a Scope, no per-row Signal to register against an Identity,
    // and no per-row `backend.create_*` call to feed
    // `CURRENT_IDENTITY` to a recording backend. The only thing the
    // outer walker reads `current_identity()` for is the slot-level
    // identity used by `execute_batch` (one call total) and
    // `register_static_cohort_batch` (also one call total) — both
    // run AFTER this loop and see `slot_identity` already.
    //
    // Previously we computed `Identity::node(slot_identity, i, None,
    // None)` per row and wrapped each iteration in
    // `with_current_identity(row_id, ...)`. The mix is a SipHash —
    // ~1µs/call — so per-row identity work was ~10ms at 10k rows
    // (the dominant cost in `enqueue_loop` after style minting).
    // Backends that observe `CURRENT_IDENTITY` mid-loop now see the
    // slot identity; that's fine, no observer cares about per-row
    // identities for the batchable shape.
    #[cfg(feature = "debug-stats")]
    let _t_enqueue_loop = crate::debug::now_micros();
    let queued_all = crate::with_current_identity(slot_identity, || -> Option<()> {
        for i in 0..count {
            let row_prim = row_builder(i);
            let queued = enqueue_primitive(backend, &mut batch, row_prim, &mut style_attachments)?;
            row_top_ids.push(queued);
        }
        Some(())
    });
    if queued_all.is_none() {
        // Non-batchable shape encountered somewhere in the loop. Abort.
        return false;
    }
    #[cfg(feature = "debug-stats")]
    crate::debug::record_apply_phase(
        "batched_repeat_enqueue_loop",
        crate::debug::now_micros().saturating_sub(_t_enqueue_loop),
    );

    // Pass 2: submit batch AND attach row tops to parent in one
    // backend call. Backends that fold both into a single FFI (web)
    // override `execute_batch_with_attach`; the default impl is the
    // literal "execute_batch + insert_many" sequence we used to do
    // here, so backends that haven't been updated still work.
    //
    // Why combined: with separate calls, each `appendChild` of a row
    // top to `parent` is its own FFI hop on the web backend (~100k
    // for the rebuild bench at 100k rows). The combined call ships
    // the row-top `local_id`s as a `Uint32Array` and lets the JS
    // shim do all N appendChild calls without crossing the boundary
    // per child. Measured savings: ~60 ms at 100 k.
    #[cfg(feature = "debug-stats")]
    let _t_execute_and_attach = crate::debug::now_micros();
    let nodes = backend
        .borrow_mut()
        .execute_batch_with_attach(batch, parent, &row_top_ids);
    #[cfg(feature = "debug-stats")]
    crate::debug::record_apply_phase(
        "batched_repeat_execute_and_attach",
        crate::debug::now_micros().saturating_sub(_t_execute_and_attach),
    );
    debug_assert_eq!(
        nodes.len(),
        row_top_ids
            .last()
            .map(|_| nodes.len())
            .unwrap_or(0),
        "execute_batch return-vec length must match batch.node_count"
    );

    // Pass 3: resolve style attachments to real nodes and bulk-register
    // them with the theme cohort. ONE Box, ONE slab insert, ONE guard
    // — regardless of N rows. This replaces the previous per-row
    // `Box<dyn FnOnce>` deferred loop which was ~88 µs/row (the
    // dominant mount cost at large N).
    #[cfg(feature = "debug-stats")]
    let _t_deferred_loop = crate::debug::now_micros();
    let mut members: Vec<(B::Node, StyleApplication)> =
        Vec::with_capacity(style_attachments.len());
    for (local_id, app) in style_attachments {
        members.push((nodes[local_id as usize].clone(), app));
    }
    register_static_cohort_batch(backend, members);
    #[cfg(feature = "debug-stats")]
    crate::debug::record_apply_phase(
        "batched_repeat_deferred_loop",
        crate::debug::now_micros().saturating_sub(_t_deferred_loop),
    );
    true
}

/// Walk a single Element subtree and push `BatchOp` entries to
/// `batch`. Returns the `local_id` of the subtree's top node, or
/// `None` if the subtree contains any non-batchable shape. On `None`
/// the caller discards `batch` and `deferred` — no partial batches.
///
/// Batchable shapes (V1):
/// - `Element::View` with `style: None | Some(Static)`, no
///   `safe_area_sides`, no `on_touch`, no `ref_fill`, and children
///   that are themselves batchable.
/// - `Element::Text` with `source: Static`, no `style`, no
///   `ref_fill`.
///
/// Why so narrow: the rebuild benchmark uses exactly this shape, and
/// every additional primitive variant we support here requires
/// thinking about reactive setup, refs, and event handlers — each
/// of which has its own per-row state to manage. V1 keeps the
/// payoff (the 80k → 1 FFI collapse) while keeping the
/// implementation small.
fn enqueue_primitive<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    batch: &mut BackendBatch,
    prim: Element,
    style_attachments: &mut Vec<(u32, StyleApplication)>,
) -> Option<u32> {
    match prim {
        Element::Text {
            source,
            style,
            ref_fill,
            ..
        } => {
            // Reactive bindings, styled text, or refs break out
            // because they require per-node Effect/scope work the
            // batch path doesn't model.
            if ref_fill.is_some() || style.is_some() {
                return None;
            }
            let content = match source {
                TextSource::Static(s) => s,
                // Reactive sources break out of the batch path — the
                // walker's per-Effect setup is the only place
                // `update_text_by_id` / `update_text_by_id_with`
                // wiring lives, so we drop back to it.
                TextSource::Bound(_) => return None,
                // JS-binding sources aren't lowered through this
                // batch path either; same drop-back-to-per-Effect
                // treatment as `Bound` once the binding lowering
                // lands. For now, bail to the slow path.
                TextSource::JsBinding(_) => return None,
            };
            let id = batch.next_id();
            batch.ops.push(BatchOp::CreateText {
                local_id: id,
                content,
            });
            Some(id)
        }
        Element::View {
            children,
            style,
            ref_fill,
            safe_area_sides,
            on_touch,
            ..
        } => {
            if ref_fill.is_some() || on_touch.is_some() || !safe_area_sides.is_empty() {
                return None;
            }

            // For a styled View, resolve & mint the class up front
            // so the batch can ship a class-name string. Reactive
            // styles bail out.
            let mut style_app_for_defer: Option<StyleApplication> = None;
            let resolved_class: Option<(String, Rc<StyleRules>)> = match style {
                None => None,
                Some(StyleSource::Static(app)) => {
                    // Drive theme/asset/typeface/token registration
                    // Rust-side immediately. This is the same call
                    // `apply_one` would make on the per-call path —
                    // we just defer the *DOM apply* to the batch.
                    //
                    // Cheap fast-path: if the sheet's already known to
                    // the registration table, skip the full
                    // `ensure_registered_with` invocation. The full
                    // version does 6 Rc clones, builds 6 closures, and
                    // sweeps the dead-Weak table BEFORE its own
                    // already-registered early-return — per-row that's
                    // ~1µs of pure bookkeeping (3% of mount time at
                    // 10k rows in a Repeat where every row shares one
                    // sheet). The peek path is a single
                    // thread-local-keyed `contains_key`.
                    if !style::is_registered(&app.sheet) {
                        let backend_for_register = backend.clone();
                        let backend_for_unregister = backend.clone();
                        let backend_for_install_tokens = backend.clone();
                        let backend_for_update_tokens = backend.clone();
                        let backend_for_asset = backend.clone();
                        let backend_for_typeface = backend.clone();
                        let backend_for_app_bg = backend.clone();
                        let backend_for_scrollbar = backend.clone();
                        style::ensure_registered_with(
                            &app.sheet,
                            |rules| {
                                backend_for_register
                                    .borrow_mut()
                                    .register_stylesheet(rules);
                            },
                            |rules| {
                                backend_for_unregister
                                    .borrow_mut()
                                    .unregister_stylesheet(rules);
                            },
                            |tokens| {
                                backend_for_install_tokens
                                    .borrow_mut()
                                    .install_tokens(tokens);
                            },
                            |tokens| {
                                backend_for_update_tokens
                                    .borrow_mut()
                                    .update_tokens(tokens);
                            },
                            |id, kind, source| {
                                backend_for_asset
                                    .borrow_mut()
                                    .register_asset(id, kind, source);
                            },
                            |id, family_name, faces, fallback| {
                                backend_for_typeface
                                    .borrow_mut()
                                    .register_typeface(
                                        id,
                                        family_name,
                                        faces,
                                        fallback,
                                    );
                            },
                            |c| {
                                backend_for_app_bg
                                    .borrow_mut()
                                    .set_app_background(c);
                            },
                            |thumb, track| {
                                backend_for_scrollbar
                                    .borrow_mut()
                                    .set_scrollbar_theme(thumb, track);
                            },
                        );
                    }
                    // State overlays force a fresh dynamic class
                    // per node, which doesn't lend itself to the
                    // share-by-pointer batch path. Bail to per-call.
                    let overlays = super::style::resolve_state_overlays(&app);
                    if !overlays.is_empty() {
                        return None;
                    }
                    let resolved = resolve_style(&app);
                    let class = backend.borrow_mut().mint_style_class(&resolved)?;
                    style_app_for_defer = Some(app);
                    Some((class, resolved))
                }
                Some(StyleSource::Reactive(_)) => {
                    // Reactive styles need per-node Effects — out of
                    // scope for V1 batching.
                    return None;
                }
                Some(StyleSource::SignalClass(_)) => {
                    // Signal-class bindings install a JS-side
                    // dispatcher at mount; not a static-shape entry
                    // the batch path can stamp.
                    return None;
                }
            };

            let view_id = batch.next_id();
            batch.ops.push(BatchOp::CreateView { local_id: view_id });

            // Walk children. Order matters for the visual layout, so
            // we enqueue Create + Insert pairs in iteration order.
            for child in children {
                let child_id = enqueue_primitive(backend, batch, child, style_attachments)?;
                batch.ops.push(BatchOp::Insert {
                    parent: view_id,
                    child: child_id,
                });
            }

            if let Some((class_name, rules)) = resolved_class {
                batch.ops.push(BatchOp::ApplyStyleStatic {
                    node: view_id,
                    class_name,
                    rules,
                });
                // Hand off the (view_id, app) pair for bulk
                // post-batch registration. The walker's
                // `register_static_cohort_batch` call after
                // `execute_batch` returns resolves these pairs to
                // real Nodes and registers them as ONE cohort entry
                // — replacing the previous per-row
                // `Box<dyn FnOnce>` path that allocated ~3 boxes per
                // row × 10k rows and dominated mount time.
                let app = style_app_for_defer
                    .expect("style_app_for_defer set together with resolved_class");
                style_attachments.push((view_id, app));
            }
            Some(view_id)
        }
        // Everything else is unsupported in V1. Return None so the
        // walker falls back to the per-call path for the entire
        // Repeat expansion.
        _ => None,
    }
}
