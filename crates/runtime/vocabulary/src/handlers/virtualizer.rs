//! Virtualizer handler — port of `walker/virtualizer.rs`'s closure path
//! (`build` → `build_virtualizer`).
//!
//! The structured / generator-backend path
//! (`build_virtualizer_declarative`: slot capture + `row_template` +
//! `note_virtualizer_binding`) is NOT ported — generator-backend wire
//! metadata is the crate-level sanctioned deferral (see `lib.rs`: every
//! author shape lowers to the equivalent closure form; generator
//! backends re-land post-P7). Runtime backends only ever took the
//! closure path, which is what conformance exercises.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::primitives::virtualizer::ItemKey;
use runtime_core::VirtualizerCallbacks;
use runtime_scene::{realize, Element, MountCx, Realized, Registry};
use runtime_world::{collect_owned, effect, untrack, Owned};

use crate::caps::VirtualizerOps;
use crate::prims::{PrimCell, VirtualizerPrim};
use crate::style_attach::{attach_style, on_teardown, StyleServices};

/// Register the `virtualizer` handler (called from `register_builtins`).
pub fn register_virtualizer<H>(registry: &mut Registry<H>)
where
    H: VirtualizerOps + StyleServices + 'static,
{
    registry.register::<PrimCell<VirtualizerPrim>, _>(|cx, p, children| {
        mount_virtualizer(cx, p.take(), children)
    });
}

/// One mounted row's ownership: the realized subtree PLUS the scope
/// that collected creations made while *constructing* the row element.
/// `realize()` only collects what the mount walk creates; the old
/// walker's `with_scope(|| { render(idx); build(...) })` covered element
/// construction too. Without `extra`, a signal/effect created inside a
/// row's builder chain (component props, probe effects) would be
/// world-root-owned and leak until world drop instead of dying on
/// `release_item` — the "row state pinned forever" leak class.
struct RowScope<N> {
    _realized: Realized<N>,
    _extra: Owned,
}

/// Mount a `virtualizer` — port of `walker/virtualizer.rs::build` +
/// `build_virtualizer`.
///
/// Sequence: build the callback bundle → `create_virtualizer` →
/// data-changed effect (first fire at mount, exactly like the old
/// `Effect::new`) → attach_style → teardown probe
/// (`release_virtualizer`) → ref-fill. The teardown order at drop
/// matches the old core because both sides free effects in creation
/// order: data effect (no cleanup), style probe (`on_node_unstyled`),
/// release probe (`release_virtualizer`).
///
/// # The backend-callback contract (ported invariants)
///
/// - **Lazy row realization inverts control**: the platform asks for
///   rows via `mount_item`/`release_item`; the handler must therefore
///   capture the backend + registry `Rc`s — `MountCx` is dead by the
///   time a scroll event mounts a row. The backend MUST invoke
///   `mount_item`/`release_item` with the owning world ambient (inside
///   `World::enter`): `mount_item` REALIZES — creation-side work
///   (`theme_ctx` → `inject`) that panics outside `enter`. Note the
///   settled dispatch-site flush drivers do NOT provide this for free —
///   ordinary author callbacks run outside `enter` (they only stage
///   writes through captured handles), so each backend's newcore
///   `VirtualizerOps` wrapper enters the boot-stored world around
///   mount/release explicitly (`enter_mounted_world`; the
///   flat_list-renders-zero-rows bug was every backend missing this).
///   The backend must also NOT invoke `mount_item` synchronously inside
///   `create_virtualizer` (the backend is mutably borrowed there; the
///   old core had the identical constraint — web/iOS/Android all defer
///   the initial window fill).
/// - **Rows realize DETACHED.** Nothing here attaches a row to the
///   scene tree or assumes in-tree layout — macOS mounts rows as
///   Taffy-orphan cells laid out via `layout_detached_root`
///   ([[project_macos_virtualizer_fix]]); the handler hands the backend
///   a bare node and the backend owns placement.
/// - **Per-row ownership scope**: each row's signals/effects die on
///   `release_item` (drop of its [`RowScope`]) — without this, queued
///   platform events fire into freed row state ("signal used after its
///   scope was dropped").
/// - **The measured-size cache is keyed by item KEY, not scope id, and
///   deliberately survives release**: a re-entering row reuses its
///   previously measured size instead of restarting from the estimate.
///   `set_measured_size` arrives keyed by scope id (that's what the
///   backend stored), so a scope-id → key reverse map bridges; the
///   reverse-map entry IS removed on release.
pub fn mount_virtualizer<H>(
    cx: &mut MountCx<'_, H>,
    prim: VirtualizerPrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: VirtualizerOps + StyleServices,
{
    let backend = cx.backend().clone();
    let registry: Rc<Registry<H>> = cx.registry().clone();

    // Per-item scope registry, shared (Rc) between the mount/release
    // closures the backend holds. Monotonic u64 ids identify mounted
    // rows; the backend stores the id alongside its cell/pool entry.
    let scopes: Rc<RefCell<HashMap<u64, RowScope<H::Node>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // Measured sizes, keyed by item KEY (survives release — see above).
    let measured_sizes: Rc<RefCell<HashMap<ItemKey, f32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    // scope id → key reverse map for `set_measured_size`.
    let scope_id_to_key: Rc<RefCell<HashMap<u64, ItemKey>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    let item_count: Rc<dyn Fn() -> usize> = Rc::from(prim.item_count);
    let item_key: Rc<dyn Fn(usize) -> ItemKey> = Rc::from(prim.item_key);

    let measure_sizes = prim.item_size.is_measured();
    let user_size: Rc<dyn Fn(usize) -> f32> = match prim.item_size {
        runtime_core::primitives::virtualizer::ItemSize::Known(f)
        | runtime_core::primitives::virtualizer::ItemSize::Measured(f) => f,
    };

    // `item_size` with the measured-override store layered on top: a
    // measured value (by key, so it survives reorder AND release) wins
    // over the author's known/estimate.
    let item_size: Rc<dyn Fn(usize) -> f32> = {
        let measured = measured_sizes.clone();
        let key_fn = item_key.clone();
        Rc::new(move |idx| {
            let key = key_fn(idx);
            if let Some(v) = measured.borrow().get(&key) {
                return *v;
            }
            user_size(idx)
        })
    };

    // mount_item: realize the row for `idx` detached, inside a fresh
    // ownership scope, and record scope-id → key.
    let mount_item: Rc<dyn Fn(usize) -> (H::Node, u64)> = {
        let backend = backend.clone();
        let registry = registry.clone();
        let render = prim.render_item.clone();
        let scopes = scopes.clone();
        let key_map = scope_id_to_key.clone();
        let key_fn = item_key.clone();
        let next_id = next_scope_id.clone();
        Rc::new(move |idx| {
            // Element construction runs INSIDE the row's collector (and
            // untracked, like keyed row renders — only the backend's
            // window math decides row lifetime, never a stray signal
            // read during construction). `realize` then collects the
            // mount walk's creations into the row's own `Realized`.
            let ((node, realized), extra) = collect_owned(|| {
                let element = untrack(|| render(idx));
                let realized = realize(&backend, &registry, element);
                let mut nodes = realized.collect_nodes();
                let node = match nodes.len() {
                    1 => nodes.pop().expect("len checked"),
                    n => panic!(
                        "virtualizer render_item({idx}) must produce a single-root element \
                         (got {n} top-level nodes) — wrap fragment rows in an item"
                    ),
                };
                (node, realized)
            });
            let id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                // Wrap like the old core: ids identify LIVE rows only.
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(
                id,
                RowScope {
                    _realized: realized,
                    _extra: extra,
                },
            );
            key_map.borrow_mut().insert(id, key_fn(idx));
            (node, id)
        })
    };

    // release_item: forget the reverse-map entry, then drop the row
    // scope (frees every signal/effect/cleanup inside the row). The
    // measured cache is NOT touched — it's keyed by item key and
    // intentionally survives unmount (see the fn docs).
    let release_item: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        let key_map = scope_id_to_key.clone();
        Rc::new(move |id| {
            key_map.borrow_mut().remove(&id);
            // Take the scope OUT of the map before dropping so row
            // cleanups can never observe a held map borrow.
            let row = scopes.borrow_mut().remove(&id);
            drop(row);
        })
    };

    // set_measured_size: backend reports a rendered size by scope id;
    // store it by item key via the reverse map.
    let set_measured_size: Rc<dyn Fn(u64, f32)> = {
        let measured = measured_sizes.clone();
        let key_map = scope_id_to_key.clone();
        Rc::new(move |scope_id, size| {
            if let Some(key) = key_map.borrow().get(&scope_id) {
                measured.borrow_mut().insert(*key, size);
            }
        })
    };

    let callbacks = VirtualizerCallbacks {
        item_count: item_count.clone(),
        item_key: item_key.clone(),
        item_size,
        measure_sizes,
        mount_item,
        release_item,
        set_measured_size,
    };

    let node = backend
        .borrow_mut()
        .create_virtualizer(callbacks, prim.overscan, prim.layout, &prim.a11y);

    // Data effect: touching item_count subscribes to whatever signals
    // the count closure reads; each change tells the backend to
    // re-query count/key/size and diff its mounted set. First fire at
    // mount — same as the old `Effect::new`'s immediate run.
    {
        let backend = backend.clone();
        let node = node.clone();
        let count = item_count;
        let _data = effect(move || {
            let _ = count();
            backend.borrow_mut().virtualizer_data_changed(&node);
        });
    }

    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }

    // Teardown: tell the backend to drop its listeners + callback
    // handles so queued scroll/resize events can't call into freed
    // per-item scopes ("signal used after its scope was dropped") —
    // the old `VirtualizerHandleCleanup` empty-Effect, as a probe.
    // Registered AFTER attach_style so teardown order matches the old
    // effect-creation order (unstyle first, then release).
    {
        let b = backend.clone();
        let n = node.clone();
        on_teardown(move || {
            b.borrow_mut().release_virtualizer(&n);
        });
    }

    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_virtualizer_handle(&node);
        fill(handle);
    }

    node
}
