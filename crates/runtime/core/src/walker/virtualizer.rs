//! `Element::Virtualizer` build path — both the runtime closure
//! variant ([`build_virtualizer`]) and the structured /
//! generator-backend variant ([`build_virtualizer_declarative`]).
//!
//! [`build`] is the dispatcher invoked by the walker — it picks
//! between the two paths based on the backend's
//! `supports_lazy_slot_capture` capability plus whether the
//! `Element::Virtualizer` carries a `row_template`.

use super::cleanup::VirtualizerHandleCleanup;
use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::{Backend, VirtualizerCallbacks};
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives;
use crate::reactive::{self, Effect};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(feature = "debug-stats")]
use crate::debug;

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: crate::derive::Derived<usize>,
    item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    item_size: primitives::virtualizer::ItemSize,
    render_item: Rc<dyn Fn(usize) -> Element>,
    row_template: Option<Box<Element>>,
    row_index_signal_id: Option<u64>,
    overscan: f32,
    horizontal: bool,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Dispatch on whether the backend opts into the
    // structured / slot-capture path AND whether the
    // Virtualizer carries the structured metadata
    // (row_template) needed for that path. Otherwise fall
    // through to the runtime closure path that drives
    // native virtualization on iOS/Android/Web.
    let lazy = backend.borrow().supports_lazy_slot_capture();
    let n = if lazy && row_template.is_some() && !item_count.is_opaque() {
        build_virtualizer_declarative(
            backend,
            item_count,
            *row_template.unwrap(),
            row_index_signal_id,
            horizontal,
        )
    } else {
        build_virtualizer(
            backend,
            item_count,
            item_key,
            item_size,
            render_item,
            row_template,
            row_index_signal_id,
            overscan,
            horizontal,
            &a11y,
        )
    };
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Cleanup hook: when the surrounding scope drops, this
    // Effect drops, dropping `cleanup`, which calls
    // `release_virtualizer`. Without this, the backend's
    // queued scroll/resize events keep firing into
    // user-supplied callbacks whose captured `Signal`s have
    // been freed → "signal used after its scope was
    // dropped" panic. Same shape as the Graphics cleanup
    // below.
    {
        let cleanup = VirtualizerHandleCleanup {
            backend: backend.clone(),
            node: n.clone(),
        };
        let _e = Effect::new(move || {
            let _ = &cleanup.node;
        });
    }
    if let Some(RefFill::Virtualizer(fill)) = ref_fill {
        let handle = backend.borrow().make_virtualizer_handle(&n);
        fill(handle);
    }
    n
}

/// Build a Virtualizer node. Sets up the callback bundle the
/// backend uses to query data + mount/release items, wraps each
/// `render_item(idx)` call in a fresh per-item Scope so signals
/// and effects nested inside an item are freed when the item is
/// released, and installs an Effect on the data so the backend
/// gets notified when item_count / keys / sizes change.
#[allow(clippy::too_many_arguments)]
fn build_virtualizer<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: crate::derive::Derived<usize>,
    item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    item_size: primitives::virtualizer::ItemSize,
    render_item: Rc<dyn Fn(usize) -> Element>,
    _row_template: Option<Box<Element>>,
    _row_index_signal_id: Option<u64>,
    overscan: f32,
    horizontal: bool,
    a11y: &AccessibilityProps,
) -> B::Node {
    // Per-item scope registry, owned by an Rc so the mount/release
    // closures (which live in the backend) share it. The framework
    // hands out monotonically-increasing u64 ids to identify each
    // mounted item; the backend stores the id alongside its cell so
    // it can release later.
    //
    // Also store measured sizes here. Backends that measure (web
    // ResizeObserver, native layout listeners) push updates via
    // `set_measured_size`; the framework keeps the canonical map.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let measured_sizes: Rc<RefCell<HashMap<u64, f32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    // Shareable closures for the data side. `Rc` so the backend can
    // clone them into per-event handlers. For the count we pull
    // `Derived<usize>`'s compute closure; runtime backends call it
    // directly. Generator backends would consume the structured
    // metadata via a separate code path (TODO when Roku grows real
    // Virtualizer support).
    let item_count_rc: Rc<dyn Fn() -> usize> = item_count.compute.clone();
    let item_key_rc: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey> = Rc::from(item_key);

    let measure_sizes = item_size.is_measured();
    let item_size_rc: Rc<dyn Fn(usize) -> f32> = match item_size {
        primitives::virtualizer::ItemSize::Known(f)
        | primitives::virtualizer::ItemSize::Measured(f) => f,
    };

    // `item_size` callback wraps the user's known/estimate with the
    // measured-override store: if we have a measured size, use it;
    // otherwise fall back to the user's value.
    let item_size_with_override: Rc<dyn Fn(usize) -> f32> = {
        let user = item_size_rc.clone();
        let measured = measured_sizes.clone();
        let key_fn = item_key_rc.clone();
        Rc::new(move |idx| {
            let key = key_fn(idx);
            // Measured cache is keyed by item key (not index) so it
            // survives reorderings.
            if let Some(v) = measured.borrow().get(&key) {
                return *v;
            }
            user(idx)
        })
    };

    // mount_item: build the subtree for `idx` inside a fresh Scope,
    // return its native node + the scope id.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let render = render_item.clone();
        let backend = backend.clone();
        Rc::new(move |idx| {
            let mut scope = Box::new(reactive::Scope::new());
            // Build inside the scope so any Effects the walker creates
            // (switch/when/style/etc.) register with this per-item
            // scope and stay alive for the item's lifetime. See the
            // matching comment in `build_navigator`'s `mount_screen`
            // for why this matters — Effects built outside any scope
            // get `owns: true` and free immediately when the handle
            // drops at end of `build`, taking their shared
            // `Rc<RefCell<...>>` state with them.
            let node = reactive::with_scope(&mut scope, || {
                let primitive = render(idx);
                super::build(&backend, 0, primitive)
            });
            let id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(id, scope);
            #[cfg(feature = "debug-stats")]
            debug::record_virtualizer_mount(idx, id);
            (node, id)
        })
    };

    // release_item: drop the scope, freeing every signal/effect/ref
    // scoped to the item.
    let release_item: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        let measured = measured_sizes.clone();
        Rc::new(move |id| {
            #[cfg(feature = "debug-stats")]
            debug::record_virtualizer_release(id);
            // Drop the scope. Its Drop impl frees the reactive slots.
            scopes.borrow_mut().remove(&id);
            // We can't safely free the measured-size entry here
            // because the entry is keyed by item *key*, not scope
            // id. The measured cache survives unmount intentionally
            // — when the item re-enters the window, we want to use
            // the previously-measured size rather than start over
            // with an estimate.
            let _ = measured;
        })
    };

    // set_measured_size: backend tells us "this scope's rendered
    // size is X." We store it by item key so the cache survives
    // unmount/remount.
    //
    // Backend identifies the item by scope id; we look up the key
    // by walking which idx this scope was mounted for. Simpler:
    // have the backend pass the *index* too. But scope_id is what
    // it stored, and it doesn't know the current index after
    // reorders. So we maintain a scope_id -> key reverse map.
    let scope_id_to_key: Rc<RefCell<HashMap<u64, primitives::virtualizer::ItemKey>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let set_measured_size: Rc<dyn Fn(u64, f32)> = {
        let measured = measured_sizes.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |scope_id, size| {
            if let Some(key) = map.borrow().get(&scope_id) {
                measured.borrow_mut().insert(*key, size);
            }
        })
    };

    // Augment mount_item to also record scope_id -> key.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let inner = mount_item.clone();
        let key_fn = item_key_rc.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |idx| {
            let (node, id) = inner(idx);
            let k = key_fn(idx);
            map.borrow_mut().insert(id, k);
            (node, id)
        })
    };

    // Augment release_item to clean up the scope_id -> key entry.
    let release_item: Rc<dyn Fn(u64)> = {
        let inner = release_item.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |id| {
            map.borrow_mut().remove(&id);
            inner(id);
        })
    };

    let callbacks = VirtualizerCallbacks {
        item_count: item_count_rc.clone(),
        item_key: item_key_rc.clone(),
        item_size: item_size_with_override,
        measure_sizes,
        mount_item,
        release_item,
        set_measured_size,
    };

    let node = time_backend_create(pkind!(Virtualizer), || {
        backend.borrow_mut().create_virtualizer(callbacks, overscan, horizontal, a11y)
    });

    // Effect: re-fires whenever the data signal changes (any reads
    // inside item_count / item_key / etc. subscribe). We tell the
    // backend to re-diff its mounted set.
    {
        let backend = backend.clone();
        let node = node.clone();
        let count = item_count_rc.clone();
        let _e = Effect::new(move || {
            // Touch item_count so we subscribe to the data signal.
            // (item_count's body calls data.get().) We don't use the
            // value here directly — the backend re-queries.
            let _ = count();
            backend.borrow_mut().virtualizer_data_changed(&node);
        });
    }

    node
}

/// Build a `Element::Virtualizer` for the structured /
/// generator-backend path (Roku). Captures the row template's
/// commands as a single slot the backend stashes for per-row
/// replay on the device. Skips the closure-driven Virtualizer
/// machinery entirely — there's no Effect, no per-cell scope
/// registry, no item_size measurement. Generator backends own
/// the cell lifecycle.
fn build_virtualizer_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: crate::derive::Derived<usize>,
    row_template: Element,
    row_index_signal_id: Option<u64>,
    horizontal: bool,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Capture the row template's construction commands as a slot.
    backend.borrow_mut().begin_slot_capture();
    let template_node = super::build(backend, 0, row_template);
    backend.borrow_mut().end_slot_capture(&template_node);

    {
        let mut b = backend.borrow_mut();
        for (sid, val) in item_count.inputs.iter().zip(item_count.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_virtualizer_binding(
            &anchor,
            &item_count.inputs,
            item_count.method,
            &template_node,
            row_index_signal_id,
            horizontal,
        );
    }

    anchor
}
