//! Style attachment for the build walker.
//!
//! Three entry points for style application (one per `StyleSource`
//! variant) plus a few helpers:
//! - [`attach_style`] — dispatches to one of the three paths below.
//! - [`attach_style_static`] — fast path with one shared theme-cohort
//!   driver instead of per-node Effects.
//! - [`attach_style_signal_class`] — backend-side JS dispatcher fan-out
//!   for `signal_class!` cohorts.
//! - [`attach_style_reactive`] — closure-based path with per-node
//!   Effect (the historic default; still used for state-bearing
//!   reactive styles).
//! - [`register_static_cohort_batch`] — the bulk variant used by the
//!   batched-Repeat path so 10k rows share a single cohort entry +
//!   guard instead of one per row.
//! - [`apply_one`] — applies a single `StyleApplication`; used both
//!   inline at mount and by the cohort driver on theme change.
//! - [`resolve_state_overlays`] — pre-resolves each declared state
//!   axis so the backend can emit pseudo-class CSS in one call.
//! - [`attach_disabled`] — reactive disabled-state wiring.
//! - [`attach_safe_area`] / [`attach_scroll_view_safe_area_inset`] —
//!   per-primitive safe-area opt-in, in two flavors.

use super::theme_cohort::{
    install_theme_cohort_driver, theme_cohort_register, theme_cohort_unregister, CohortId,
};
use crate::backend::Backend;
use crate::handles::StateBits;
use crate::reactive::{self, Effect, Signal};
use crate::sources::{SignalClassSpec, StyleSource};
use crate::style::{self, resolve as resolve_style, StyleApplication, StyleRules, StyleSheet};
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(feature = "debug-stats")]
use crate::debug;

thread_local! {
    /// Build-time stack of the nearest ancestor containment context's
    /// inline-size signal. A `.container()` view pushes its width signal
    /// before its children are built and pops it after, so during a
    /// descendant's `attach_style` the top is exactly its nearest
    /// container ancestor.
    ///
    /// Why a build-time stack rather than `provide`/`inject`: the build
    /// walk is synchronous and depth-first, but a plain `view` opens no
    /// reactive scope, so `provide` would leak a container's signal to
    /// its *siblings*. The stack is pushed/popped around the exact
    /// `insert_children` window that builds the subtree, so it sees only
    /// descendants. Each styled descendant captures the signal (a cheap
    /// `Copy` handle) into its style Effect at build time; the stack
    /// itself is empty again once the build returns.
    static CONTAINER_STACK: RefCell<Vec<Signal<f32>>> = const { RefCell::new(Vec::new()) };
}

/// Push a containment context's inline-size signal for the duration of
/// its children's build. Paired with [`pop_container`].
pub(super) fn push_container(sig: Signal<f32>) {
    CONTAINER_STACK.with(|s| s.borrow_mut().push(sig));
}

/// Pop the most recently pushed containment context. Paired with
/// [`push_container`].
pub(super) fn pop_container() {
    CONTAINER_STACK.with(|s| {
        s.borrow_mut().pop();
    });
}

/// The nearest ancestor containment context's inline-size signal at the
/// current point in the build walk, or `None` when outside any
/// `.container()`. Read at `attach_style` time and captured into the
/// node's style Effect (which reads `.get()` to subscribe).
pub(super) fn nearest_container() -> Option<Signal<f32>> {
    CONTAINER_STACK.with(|s| s.borrow().last().copied())
}

/// RAII wrapper that calls `Backend::on_node_unstyled` when dropped.
/// Captured by the styled effect's closure so backend per-node state
/// (e.g. the web backend's dynamic CSS class slot) gets cleaned up
/// when the effect's scope drops — which happens on `when()` rebuilds
/// and on `Owner` teardown.
pub(super) struct StyleHandle<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    /// Per-row node handle, shared via `Rc` so the cohort closure
    /// (when present) and this handle can both hold it without
    /// each maintaining its own wasm-bindgen JsValue slot. On
    /// web, this means one underlying `web_sys::Node` slot per
    /// styled row instead of two — halves the
    /// `__wbindgen_object_drop_ref` FFI hops fired at scope
    /// teardown. On backends where `B::Node` doesn't cross an FFI
    /// boundary (mobile native), `Rc` is just an Rc — same code
    /// path, no per-platform fork.
    pub(super) node: Rc<B::Node>,
    /// For nodes attached via the static-style path: id into the
    /// theme cohort. `None` for reactive-style nodes (those re-apply
    /// via their own `Effect`'s theme subscription, not the cohort).
    cohort_id: Option<CohortId>,
}

impl<B: Backend + 'static> Drop for StyleHandle<B> {
    fn drop(&mut self) {
        // Remove from the theme cohort first, if registered. The
        // cohort holds a `Box<dyn Any>` that owns an `Rc` clone of
        // the node; dropping it decrements the Rc count. The
        // underlying `B::Node` only releases its JS-side slot when
        // the LAST `Rc` reference drops (typically here, since the
        // cohort entry was just unregistered).
        if let Some(id) = self.cohort_id.take() {
            theme_cohort_unregister(id);
        }
        self.backend.borrow_mut().on_node_unstyled(&self.node);
    }
}

/// Registers a **single** theme-cohort entry that owns every member
/// of a batched `Element::Repeat`, plus a **single** RAII guard
/// adopted into the active scope. On theme/token change the cohort
/// entry's re-apply closure iterates the member Vec and calls
/// [`apply_one`] for each — semantically identical to per-row
/// registration but with O(1) heap allocations + slab inserts
/// instead of O(N).
///
/// Per-row registration cost ~88 µs (heap alloc for the reapply
/// closure, heap alloc for the StyleHandle guard, two
/// `Box<dyn ...>` allocations through wasm-bindgen tracking, slab
/// inserts). At 10k rows that's ~880 ms of pure Rust-side
/// bookkeeping. The bulk path skips it.
///
/// Members move into the cohort. The shared `Rc<StyleApplication>`s
/// avoid cloning the (possibly heavy) `StyleApplication` into the
/// reapply closure.
pub(super) fn register_static_cohort_batch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    members: Vec<(B::Node, StyleApplication)>,
) {
    if members.is_empty() {
        return;
    }
    install_theme_cohort_driver(backend);
    let handles_states_natively = backend.borrow().handles_states_natively();
    let backend_for_cohort = backend.clone();

    // Single Rc-wrapped Vec shared between the cohort entry's
    // reapply closure and the BulkStyleHandle guard. We do NOT
    // per-row-wrap each `StyleApplication` in its own Rc — `apply_one`
    // takes `&StyleApplication` and works fine from `Vec` iteration.
    // That saves N heap allocations (was: ~88 µs/row even after the
    // per-row deferred-closure replacement, traced to `Rc::new` calls
    // + StyleApplication moves; StyleApplication carries a BTreeMap +
    // StyleRules, several hundred bytes each).
    let members_rc: Rc<Vec<(B::Node, StyleApplication)>> = Rc::new(members);
    let members_for_cohort = members_rc.clone();
    let cohort_id = theme_cohort_register(Box::new(move || {
        for (node, app) in members_for_cohort.iter() {
            // Batched-Repeat rows don't carry a container ancestor of
            // their own (the batch fast path is for flat, simple rows),
            // so container overlays — if any — resolve at width 0 (inert).
            // A row that needs container queries falls out of the batch
            // path into the per-node Effect anyway.
            apply_one(&backend_for_cohort, node, app, handles_states_natively, 0.0);
        }
    }));

    /// Bulk RAII guard for a batched-Repeat's static-style rows. On
    /// drop: unregister the cohort entry, then invoke
    /// `on_node_unstyled` for every member. The latter is generally
    /// a no-op for batched-Repeat rows (they were never given a
    /// dynamic class), but called for symmetry with the per-row
    /// path and to keep the invariant that every `mint_style_class`
    /// hit has a matching unstyle.
    struct BulkStyleHandle<B: Backend + 'static> {
        backend: Rc<RefCell<B>>,
        members: Rc<Vec<(B::Node, StyleApplication)>>,
        cohort_id: Option<CohortId>,
    }
    impl<B: Backend + 'static> Drop for BulkStyleHandle<B> {
        fn drop(&mut self) {
            if let Some(id) = self.cohort_id.take() {
                theme_cohort_unregister(id);
            }
            let mut b = self.backend.borrow_mut();
            for (node, _) in self.members.iter() {
                b.on_node_unstyled(node);
            }
        }
    }

    let handle = BulkStyleHandle {
        backend: backend.clone(),
        members: members_rc,
        cohort_id: Some(cohort_id),
    };
    let adopted = reactive::adopt_guard_into_active_scope(handle);
    debug_assert!(
        adopted,
        "register_static_cohort_batch called outside an active Scope"
    );
}

/// Attaches a style to an already-constructed node by spawning an
/// independent reactive Effect that re-applies on each signal change.
/// The effect captures a `StyleHandle` so that when its scope drops
/// the backend gets `on_node_unstyled` notification for per-node
/// cleanup (e.g. dropping the web backend's dynamic CSS rule).
///
/// Independent of any content effect on the same node — a content
/// signal change doesn't re-fire the style effect, and vice versa.
pub(super) fn attach_style<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: StyleSource,
) -> Rc<dyn Fn(StateBits, bool)> {
    match style {
        StyleSource::Static(app) => attach_style_static(backend, node, app),
        StyleSource::Reactive(f) => attach_style_reactive(backend, node, f),
        StyleSource::SignalClass(spec) => attach_style_signal_class(backend, node, spec),
    }
}

/// Wire `safe_area_sides` to the backend reactively. Subscribes to
/// the framework's global insets signal so the backend re-applies
/// padding on every change (orientation flip, sheet adaptation,
/// dynamic island). The Effect lives until this primitive's scope
/// drops — same RAII model as `attach_style`'s reactive path.
pub(super) fn attach_safe_area<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    sides: crate::SafeAreaSides,
) {
    let backend = backend.clone();
    let node = node.clone();
    let _effect = Effect::new(move || {
        // Touch the insets signal so this effect re-runs whenever
        // the platform reports new values. We don't need the value
        // here — the backend reads its own platform source inside
        // `apply_safe_area_padding`. The subscription is the point.
        let _ = crate::safe_area::safe_area_insets().get();
        backend
            .borrow_mut()
            .apply_safe_area_padding(&node, sides);
    });
}

/// Sibling of `attach_safe_area` for `Element::ScrollView`. Routes
/// the safe-area opt-in through `apply_scroll_view_safe_area_inset`
/// so backends apply *contentInset* semantics (background bleeds
/// edge-to-edge, content origin insets) rather than the outer
/// padding mode that fits a `View`.
pub(super) fn attach_scroll_view_safe_area_inset<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    sides: crate::SafeAreaSides,
) {
    let backend = backend.clone();
    let node = node.clone();
    let _effect = Effect::new(move || {
        let _ = crate::safe_area::safe_area_insets().get();
        backend
            .borrow_mut()
            .apply_scroll_view_safe_area_inset(&node, sides);
    });
}

/// Static-style fast path: no per-node `Effect`, no signal
/// subscription. The style is applied inline at mount, and the node
/// is registered with the framework's theme cohort so a `set_theme`
/// call re-applies it in bulk via a single shared `Effect`. Saves
/// 10k arena slots + 10k closure boxes for a 10k-row scoreboard
/// vs. the reactive path. RAII guard inside the build walker (via
/// the returned `StyleHandle` captured by the cleanup effect)
/// removes the cohort entry on teardown.
fn attach_style_static<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    app: StyleApplication,
) -> Rc<dyn Fn(StateBits, bool)> {
    let handles_states_natively = backend.borrow().handles_states_natively();

    // Native state-machine divert: an event-driven backend (macOS / iOS /
    // Android) drives hover/press/focus through the Rust state machine, which
    // lives in `attach_style_reactive` — it allocates the `states_signal` that
    // the backend's `attach_states` setter flips, then re-resolves and
    // re-applies on each flip. The cohort-only static path below returns a
    // NO-OP setter and never re-applies, so a STATIC-styled node whose sheet
    // declares `state` overlays (e.g. a `MenuItem` / `ListItem` whose `active`
    // prop happens to be a static bool, so the component emits a resolved
    // `StyleApplication` rather than a closure) silently lost hover on native
    // while web — which bakes `:hover` into CSS — kept it. Divert such nodes to
    // the reactive path so their state overlays work; it also handles any
    // breakpoint/container overlays the sheet declares. Web
    // (`handles_states_natively`) stays on the cohort path: its hover is in the
    // emitted CSS, no per-node signal needed. Done BEFORE the cohort-driver
    // install since a diverted node never joins the cohort.
    if !handles_states_natively && !app.sheet.state_axes().is_empty() {
        return attach_style_reactive(backend, node, Box::new(move || app.clone()));
    }

    // Make sure the cohort driver is alive before we register.
    install_theme_cohort_driver(backend);

    // Native container-query divert: a static sheet that declares
    // `container` overlays AND sits inside a containment context can't
    // ride the shared theme cohort — the cohort re-applies via global
    // signals, but each node's container width is position-specific. Give
    // it its own per-node Effect that subscribes to the captured
    // container signal (plus tokens + breakpoint) instead. Web keeps the
    // cohort: it emits `@container` CSS and the browser tracks size, so
    // no per-node signal is needed there.
    if !handles_states_natively && !app.sheet.container_axes().is_empty() {
        if let Some(container_sig) = nearest_container() {
            return attach_style_static_container(
                backend,
                node,
                app,
                handles_states_natively,
                container_sig,
            );
        }
    }

    // Inline first apply. Identical work to what the reactive
    // path's Effect would do on its first run — just without
    // wrapping it in an Effect closure. No container ancestor here, so
    // container overlays (if any) resolve at width 0 (inert).
    apply_one(backend, node, &app, handles_states_natively, 0.0);

    // Register the node with the theme cohort. We wrap the
    // `StyleApplication` in an `Rc` so the cohort closure pays
    // only a pointer-clone on registration — `StyleApplication`
    // itself transitively owns a `StyleRules` overrides struct
    // that's ~1 KB, and at 10k rows the per-row clone of that
    // was the dominant new allocation cost vs. the reactive path.
    //
    // Node sharing: the cohort closure and the cleanup handle
    // BOTH need a Node reference. Pre-Rc-share, each made an
    // independent `node.clone()` (= a wasm-bindgen JsValue
    // clone-FFI per clone for web), and each fired
    // `__wbindgen_object_drop_ref` independently at teardown —
    // 2 mount FFI hops + 2 teardown FFI hops PER ROW just for
    // node refcount management. The shared `Rc<B::Node>` here
    // collapses both clones to one underlying JsValue slot;
    // the cohort and handle each hold cheap `Rc` clones (atomic
    // bumps, no FFI), and the LAST drop fires one drop_ref hop
    // for the row.
    let backend_for_cohort = backend.clone();
    let node_rc: Rc<B::Node> = Rc::new(node.clone());
    let node_for_cohort = node_rc.clone();
    let app_for_cohort = Rc::new(app);
    let cohort_id = theme_cohort_register(Box::new(move || {
        // Non-diverted static nodes have no container ancestor (the
        // divert above handles those), so container width is 0 (inert).
        apply_one(
            &backend_for_cohort,
            &node_for_cohort,
            &app_for_cohort,
            handles_states_natively,
            0.0,
        );
    }));

    // Attach the cleanup guard directly to the active scope —
    // bypasses the arena entirely (no `Effect` slot, no subscriber
    // set entry, no dependency set entry). The guard is held in
    // `Scope::guards`, dropped in the same batch as effects when
    // the scope tears down. For a 10k-row scope this is the
    // difference between 10k arena allocs and ~10k cheap Vec
    // pushes — the underlying `Box<dyn Any>` and the `StyleHandle`
    // contents are the same shape either way, but we save the
    // arena bookkeeping.
    let cleanup_handle = StyleHandle {
        backend: backend.clone(),
        node: node_rc,
        cohort_id: Some(cohort_id),
    };
    let adopted = reactive::adopt_guard_into_active_scope(cleanup_handle);
    debug_assert!(
        adopted,
        "attach_style_static called outside an active Scope — \
         StyleHandle would leak (cohort entry + per-node backend state \
         never cleaned). The renderer's `Owner` always sets a scope, \
         so this fires only for ad-hoc top-level use."
    );

    // The setter is a no-op on natively-handling backends — `setter`
    // is exposed for `attach_disabled` etc., but with no Signal in
    // play it has nothing to flip. For event-driven backends the
    // static path doesn't apply (we'd lose state reactivity), but
    // those backends would route through `attach_style_reactive`
    // anyway because the macro emits a closure for state-bearing
    // styles. Returning a no-op keeps the return type aligned.
    //
    // TODO: revisit when adding native iOS/Android backends. The
    // static path may need to keep a Signal<StateBits> after all.
    Rc::new(|_, _| {})
}

/// Native divert for a static-styled node that declares `container`
/// overlays and sits inside a containment context. Instead of the shared
/// theme cohort (which re-applies via global signals), this gives the
/// node its own Effect subscribing to:
///
/// - the captured nearest-container inline-size `Signal<f32>` (so a
///   container resize that crosses a threshold re-applies),
/// - all token signals (so theme changes still re-apply — the cohort
///   normally owns this for static nodes), and
/// - `current_breakpoint()` (read transitively by `apply_one` ->
///   `merge_active_breakpoints`, so viewport breakpoints still work
///   alongside container queries).
///
/// Convergence: the re-apply may change layout (e.g. `flex_direction`),
/// triggering a relayout; the new layout feeds the container's
/// inline-size signal again, but under the inline-size containment
/// invariant that width is unchanged, so the change-guarded signal does
/// not re-fire and the system settles. See [`crate::container_query`].
fn attach_style_static_container<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    app: StyleApplication,
    handles_states_natively: bool,
    container_sig: Signal<f32>,
) -> Rc<dyn Fn(StateBits, bool)> {
    let backend_for_effect = backend.clone();
    let node_for_effect = node.clone();
    let app_rc = Rc::new(app);

    // The active Scope owns the Effect's slot (its handle drop is a
    // no-op), so the Effect lives until the node's scope tears down —
    // same RAII model as `attach_style_reactive`.
    let _e = Effect::new(move || {
        // Subscribe to theme/token changes — the cohort would normally
        // do this for a static node, but this node is out of the cohort.
        crate::style::subscribe_to_all_token_signals();
        // Reading the signal subscribes this Effect; a container width
        // change re-fires it. `apply_one` reads `current_breakpoint()`
        // internally, so viewport breakpoints re-fire it too.
        let width = container_sig.get();
        apply_one(
            &backend_for_effect,
            &node_for_effect,
            &app_rc,
            handles_states_natively,
            width,
        );
    });

    // Per-node cleanup so the backend drops any per-node state on
    // unmount. No cohort entry to release (cohort_id = None).
    let cleanup_handle = StyleHandle {
        backend: backend.clone(),
        node: Rc::new(node.clone()),
        cohort_id: None,
    };
    let adopted = reactive::adopt_guard_into_active_scope(cleanup_handle);
    debug_assert!(
        adopted,
        "attach_style_static_container cleanup handle not adopted into a Scope"
    );

    Rc::new(|_, _| {})
}

/// Attach a `StyleSource::SignalClass` to `node`. Pre-resolves
/// `(value, app)` pairs to minted class names at mount, hands the
/// table to the backend's JS-binding registry (if supported), and
/// adopts a release guard into the active scope.
///
/// **Fast path (backend supports JS class bindings):** at mount we
/// resolve once and pay zero per-fire Rust work — JS-side fan-out
/// applies the right class on every signal write. Closes the gap to
/// React for SHARED reactive-style cohorts where one signal drives
/// N nodes.
///
/// **Fallback path:** for backends that don't support JS bindings,
/// the spec's `compute_fallback` runs inside a normal style Effect
/// — same shape as `attach_style_reactive` would produce. No
/// behavioral difference, just no FFI fan-out optimization.
fn attach_style_signal_class<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    spec: SignalClassSpec,
) -> Rc<dyn Fn(StateBits, bool)> {
    if !backend.borrow().supports_js_class_bindings() {
        // Backend can't host the JS-side binding — fall back to the
        // closure path. The walker would have produced the same
        // shape if the user had passed a plain reactive closure
        // instead of building a `SignalClassSpec`.
        return attach_style_reactive(
            backend,
            node,
            Box::new(move || (spec.compute_fallback)()),
        );
    }

    install_theme_cohort_driver(backend);

    // Resolve every (value, app) pair to a minted class. The apps
    // themselves are pinned by `_kept_apps` below so the
    // stylesheet registrations don't get dead-Weak-swept while
    // the binding is live.
    let mut class_names: Vec<String> = Vec::with_capacity(spec.apps.len());
    for app in &spec.apps {
        // Drive registration through the same path `apply_one`
        // uses. We don't apply to the node here — the JS-binding
        // dispatcher does the actual setAttribute on signal writes.
        let backend_for_register = backend.clone();
        let backend_for_unregister = backend.clone();
        let backend_for_install_tokens = backend.clone();
        let backend_for_update_tokens = backend.clone();
        let backend_for_asset = backend.clone();
        let backend_for_typeface = backend.clone();
        let backend_for_app_bg = backend.clone();
        let backend_for_scrollbar = backend.clone();
        let backend_for_app_key = backend.clone();
        style::ensure_registered_with(
            &app.sheet,
            |rules| { backend_for_register.borrow_mut().register_stylesheet(rules); },
            |rules| { backend_for_unregister.borrow_mut().unregister_stylesheet(rules); },
            |tokens| { backend_for_install_tokens.borrow_mut().install_tokens(tokens); },
            |tokens| { backend_for_update_tokens.borrow_mut().update_tokens(tokens); },
            |id, kind, source| { backend_for_asset.borrow_mut().register_asset(id, kind, source); },
            |id, fname, faces, fb| {
                backend_for_typeface.borrow_mut().register_typeface(id, fname, faces, fb);
            },
            |c| { backend_for_app_bg.borrow_mut().set_app_background(c); },
            |thumb, track| { backend_for_scrollbar.borrow_mut().set_scrollbar_theme(thumb, track); },
            |h| { backend_for_app_key.borrow_mut().set_app_key_handler(h); },
        );
        // `mint_class_for_app` mints a fresh dynamic class if the
        // app's resolved content isn't already a pre-generated
        // entry (the common case for `.override_*` styles). Returns
        // None for backends without a named-class model — in that
        // case `supports_js_class_bindings` should also return
        // false and we'd have already taken the fallback path
        // above, so reaching here with None is a backend
        // contract violation.
        let class = backend
            .borrow_mut()
            .mint_class_for_app(app)
            .expect("mint_class_for_app returned None for a SignalClass app — \
                     backends that support JS class bindings must mint fresh \
                     classes for dynamic override content");
        class_names.push(class);
    }

    let class_refs: Vec<&str> = class_names.iter().map(|s| s.as_str()).collect();
    let binding_id = backend.borrow_mut().register_reactive_class_binding(
        node,
        spec.signal_id,
        &spec.values,
        &class_refs,
        spec.read_signal.clone(),
    );

    // Release guard: drops the binding from the backend's registry
    // on scope teardown AND keeps the apps Vec alive so the
    // stylesheet Rcs aren't dead-Weak-swept while the binding is
    // live. The `_kept_apps` field is intentionally unused — its
    // Drop is the side effect we want.
    struct SignalClassGuard<B: Backend + 'static> {
        backend: Rc<RefCell<B>>,
        binding_id: u32,
        _kept_apps: Vec<StyleApplication>,
    }
    impl<B: Backend + 'static> Drop for SignalClassGuard<B> {
        fn drop(&mut self) {
            self.backend
                .borrow_mut()
                .release_reactive_class_binding(self.binding_id);
        }
    }
    let guard = SignalClassGuard {
        backend: backend.clone(),
        binding_id,
        _kept_apps: spec.apps,
    };
    let adopted = reactive::adopt_guard_into_active_scope(guard);
    debug_assert!(
        adopted,
        "attach_style_signal_class called outside an active Scope — \
         binding would leak (JS-side registry entry never released)."
    );

    // Same no-op state setter the static path returns — state
    // overlays aren't part of the SignalClass abstraction today.
    Rc::new(|_, _| {})
}

/// Apply a style to a single node. Pulled out as a free function
/// so both the static path (called inline at mount) and the cohort
/// driver (called on theme change) can re-use it.
pub(super) fn apply_one<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    app: &StyleApplication,
    handles_states_natively: bool,
    container_width: f32,
) {
    {
        let backend_for_register = backend.clone();
        let backend_for_unregister = backend.clone();
        let backend_for_install_tokens = backend.clone();
        let backend_for_update_tokens = backend.clone();
        let backend_for_asset = backend.clone();
        let backend_for_typeface = backend.clone();
        let backend_for_app_bg = backend.clone();
        let backend_for_scrollbar = backend.clone();
        let backend_for_app_key = backend.clone();
        style::ensure_registered_with(
            &app.sheet,
            |rules| {
                backend_for_register.borrow_mut().register_stylesheet(rules);
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
                backend_for_asset.borrow_mut().register_asset(id, kind, source);
            },
            |id, family_name, faces, fallback| {
                backend_for_typeface
                    .borrow_mut()
                    .register_typeface(id, family_name, faces, fallback);
            },
            |c| {
                backend_for_app_bg.borrow_mut().set_app_background(c);
            },
            |thumb, track| {
                backend_for_scrollbar
                    .borrow_mut()
                    .set_scrollbar_theme(thumb, track);
            },
            |h| {
                backend_for_app_key.borrow_mut().set_app_key_handler(h);
            },
        );
    }
    if handles_states_natively {
        let base = resolve_style(app);
        let state_overlays = resolve_state_overlays(app);
        let bp_overlays = resolve_breakpoint_overlays(app);
        // Web emits container overlays as `@container` CSS (the browser
        // resolves them against the nearest `container-type` ancestor),
        // so `container_width` is unused on this path.
        let cq_overlays = resolve_container_overlays(app);
        warn_if_orphan_system_font(&base);
        backend.borrow_mut().apply_styled_variants(
            node,
            &base,
            &state_overlays,
            &bp_overlays,
            &cq_overlays,
        );
    } else {
        // Native backends: fold the active breakpoint bucket's overlay
        // into the resolved rules and apply through the regular path.
        // `merge_active_breakpoints` reads `current_breakpoint()` (only
        // when overlays exist), so static-styled nodes re-apply when
        // the bucket changes — the theme cohort driver subscribes to
        // the same signal and re-runs `apply_one` on a bucket flip.
        let base = resolve_style(app);
        let bp_overlays = resolve_breakpoint_overlays(app);
        let resolved = merge_active_breakpoints(base, &bp_overlays);
        // Container overlays fold on top of the breakpoint cascade,
        // keyed on the nearest container's resolved inline-size passed by
        // the caller (`0.0` when this node has no container ancestor, in
        // which case no container overlay activates). Container-bearing
        // static nodes with a real container ancestor are diverted out of
        // the cohort into a per-node Effect (see `attach_style_static`)
        // so this width is the up-to-date signal value.
        let cq_overlays = resolve_container_overlays(app);
        let resolved = merge_active_containers(resolved, &cq_overlays, container_width);
        warn_if_orphan_system_font(&resolved);
        backend.borrow_mut().apply_style(node, &resolved);
    }
}

/// Debug-only DX guardrail: if a resolved rule's `font_family` is a
/// bare `System(name)` that matches no registered `typeface!` and no
/// known system font, warn once. Runs after the node's own stylesheet
/// has registered (so a same-sheet typeface is already recorded). The
/// whole call — including the `font_family` read — compiles out in
/// release; see `style::maybe_warn_unregistered_system_font`.
#[inline]
fn warn_if_orphan_system_font(_rules: &StyleRules) {
    #[cfg(debug_assertions)]
    if let Some(crate::style::FontFamily::System(name)) = &_rules.font_family {
        crate::style::maybe_warn_unregistered_system_font(name);
    }
}

fn attach_style_reactive<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: Box<dyn Fn() -> StyleApplication>,
) -> Rc<dyn Fn(StateBits, bool)> {
    // Per-phase timing of attach_style. The point is to separate
    // "framework overhead per styled node" (Effect alloc, Signal
    // alloc, scope registration, clones) from "actual style work"
    // (resolve, apply, register stylesheet) so a high-row-count
    // render's overhead can be measured rather than guessed at.
    //
    // Phases emitted (all only when `debug-stats` is on):
    //   attach_style_total          wraps the whole call
    //   attach_style_setup          pre-Effect setup (clones, Signal::new, borrow for caps)
    //   attach_style_effect_alloc   Effect::new — alloc slot AND first run
    //   attach_style_first_run      just the closure body inside Effect::new's first run
    //   attach_style_post_effect    Rc<setter>, backend.attach_states
    //   attach_style_resolve        resolve_style + resolve_state_overlays per run
    //   attach_style_apply_call     the backend's apply_styled_states / apply_style call
    //
    // The interesting quantity is (effect_alloc - first_run) — the
    // pure arena/scope-registration cost per styled node.
    #[cfg(feature = "debug-stats")]
    let _t_total_start = debug::now_micros();

    #[cfg(feature = "debug-stats")]
    let _t_setup_start = debug::now_micros();

    // StyleHandle owns the node-handle the effect closure needs. The
    // closure body reads `handle.node` directly, so we don't clone
    // the node twice per row — one Node clone per row is the floor,
    // and each clone is a wasm-bindgen JsValue (decref runs a JS-side
    // FFI call on drop, ~3μs in practice). At 10k rows that's the
    // difference between ~60ms and ~120ms of teardown cost.
    let backend_for_effect = backend.clone();

    let handle = StyleHandle {
        backend: backend.clone(),
        // Same `Rc<B::Node>` shape as the static path — keeps the
        // struct's field type uniform across both call sites. The
        // reactive path only holds one node reference (no cohort
        // closure to share with) so there's no per-row FFI saving
        // here, just the heap alloc for the Rc — a few ns at most.
        node: Rc::new(node.clone()),
        cohort_id: None,
    };

    let handles_states_natively = backend.borrow().handles_states_natively();

    // Per-node active interaction states. For backends that don't
    // handle states natively (Android, iOS), we keep a Signal<StateBits>
    // that flips on native events; the style effect re-resolves on
    // each flip and merges the relevant `__state_*` axes.
    //
    // For backends that DO handle states natively (web), no signal is
    // needed — `apply_styled_states` pre-emits all state overlays as
    // CSS pseudo-class rules, so the browser drives state tracking
    // without a Rust round-trip. Skipping the alloc is worth ~10k
    // arena slot creations per 10k-row rebuild.
    let states_signal: Option<Signal<StateBits>> = if handles_states_natively {
        None
    } else {
        Some(Signal::new(StateBits::NONE))
    };

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_setup",
        debug::now_micros().saturating_sub(_t_setup_start),
    );

    #[cfg(feature = "debug-stats")]
    let _t_effect_alloc_start = debug::now_micros();

    // Per-Effect strong handle to the latest sheet returned by the
    // closure. Without this, a closure that builds an inline
    // `Rc<StyleSheet>` per call (the common shape for
    // `with_style(|| { ... StyleApplication::new(Rc::new(StyleSheet::r#static(...))) })`)
    // would drop the sheet to refcount 0 the moment the Effect body
    // returns. `ensure_registered_with` stores a `Weak<StyleSheet>`
    // keyed by `Rc::as_ptr`; once the strong count hits zero the
    // Weak is dead, and the NEXT call to `ensure_registered_with`
    // (from any other styled view in the same mount pass) runs the
    // dead-Weak sweep, queues the rules into PENDING_UNREGISTER,
    // and the flush calls `unregister_stylesheet` — deleting the
    // CSS rule the current node still references via its class
    // attribute.
    //
    // Pinning the latest `Rc<StyleSheet>` in this slot keeps the
    // Weak alive for the Effect's lifetime, which is exactly "the
    // node has this style applied." When the surrounding scope
    // drops (node unmount), the Effect drops, the closure drops,
    // this slot drops, the strong ref decrements, and the sheet
    // becomes eligible for cleanup on the next sweep — correctly,
    // because the node is gone.
    //
    // The slot holds only `Rc<StyleSheet>` (not the full
    // `StyleApplication`) so the closure can still consume `app`
    // by move on the event-driven backend path below — we clone
    // the cheap `Rc<StyleSheet>` into the slot before that.
    //
    // Underscore prefix: the variable's value is never *read* —
    // its Drop on Effect teardown IS the side effect we want.
    // Without the prefix, rustc rightly complains "value assigned
    // but never read" since we only ever write to it; that
    // warning is correct (the value isn't a value-of-interest)
    // but the variable itself is load-bearing.
    let mut _pinned_sheet: Option<Rc<StyleSheet>> = None;

    // Capture the nearest ancestor containment context at build time (the
    // build-time stack is empty by the time the Effect re-fires). The
    // Effect reads `.get()` inside its body so a container width change
    // re-fires it; nodes whose sheet declares no `container` blocks never
    // call `.get()` and so never subscribe. Cheap `Copy` handle.
    let container_sig = nearest_container();

    let _e = Effect::new(move || {
        #[cfg(feature = "debug-stats")]
        let _t_first_run_start = debug::now_micros();

        // `handle` is captured by-move so its Drop runs iff the
        // effect is dropped — that's how `on_node_unstyled` fires
        // exactly once per styled node when its scope tears down.

        #[cfg(feature = "debug-stats")]
        debug::record_apply_style_enter();
        #[cfg(feature = "debug-stats")]
        debug::record_effect_fired();

        let app = style();

        // Same fast-path as the batched-Repeat walker: once the
        // sheet is registered (which holds for the entire lifetime
        // of every steady-state row), skip the 6 Rc clones + the
        // closure-passing into `ensure_registered_with`. The full
        // function would early-return at its `already` check
        // anyway, but only AFTER it's done its pending-token flush
        // + dead-Weak sweep — ~500 ns of pure bookkeeping per fire.
        // For a SHARED reactive-style bump that fans out to N rows
        // with the same sheet, that's N × 500 ns we shouldn't pay.
        if !style::is_registered(&app.sheet) {
            let backend_for_register = backend_for_effect.clone();
            let backend_for_unregister = backend_for_effect.clone();
            let backend_for_install_tokens = backend_for_effect.clone();
            let backend_for_update_tokens = backend_for_effect.clone();
            let backend_for_asset = backend_for_effect.clone();
            let backend_for_typeface = backend_for_effect.clone();
            let backend_for_app_bg = backend_for_effect.clone();
            let backend_for_scrollbar = backend_for_effect.clone();
            let backend_for_app_key = backend_for_effect.clone();
            style::ensure_registered_with(
                &app.sheet,
                |rules| {
                    backend_for_register.borrow_mut().register_stylesheet(rules);
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
                    backend_for_asset.borrow_mut().register_asset(id, kind, source);
                },
                |id, family_name, faces, fallback| {
                    backend_for_typeface
                        .borrow_mut()
                        .register_typeface(id, family_name, faces, fallback);
                },
                |c| {
                    backend_for_app_bg.borrow_mut().set_app_background(c);
                },
                |thumb, track| {
                    backend_for_scrollbar
                        .borrow_mut()
                        .set_scrollbar_theme(thumb, track);
                },
                |h| {
                    backend_for_app_key.borrow_mut().set_app_key_handler(h);
                },
            );
        }

        // Pin the sheet so its `Weak` in REGISTRATIONS stays
        // upgradeable for the rest of this Effect's lifetime. Cheap
        // `Rc::clone` — see the long comment at the outer
        // `pinned_sheet` declaration for why this is mandatory.
        // Must happen AFTER `ensure_registered_with` (otherwise
        // there's nothing to pin yet) and BEFORE the
        // event-driven branch below consumes `app` by move.
        _pinned_sheet = Some(app.sheet.clone());

        if handles_states_natively {
            // Resolve the base (no state axes) and each declared state
            // overlay separately. The backend will emit CSS rules
            // scoped to each pseudo-class so the browser does the
            // state switching natively.
            //
            // We deliberately do NOT subscribe to `states_signal` here:
            // CSS handles all transitions, so the style effect should
            // re-fire only on theme/variant/override changes, not on
            // hover/press.
            #[cfg(feature = "debug-stats")]
            let _t_resolve_start = debug::now_micros();
            let base = resolve_style(&app);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve_base",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );
            #[cfg(feature = "debug-stats")]
            let _t_overlays_start = debug::now_micros();
            let overlays = resolve_state_overlays(&app);
            let bp_overlays = resolve_breakpoint_overlays(&app);
            let cq_overlays = resolve_container_overlays(&app);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve_overlays",
                debug::now_micros().saturating_sub(_t_overlays_start),
            );
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );

            #[cfg(feature = "debug-stats")]
            let _t_apply_start = debug::now_micros();
            warn_if_orphan_system_font(&base);
            // Web (the only `handles_states_natively` backend) emits
            // breakpoint overlays as `@media` CSS, so this branch does
            // NOT read `current_breakpoint()` — the browser does the
            // bucket switching, and the style effect should only re-fire
            // on theme/variant/override changes, not on resize.
            backend_for_effect.borrow_mut().apply_styled_variants(
                &handle.node,
                &base,
                &overlays,
                &bp_overlays,
                &cq_overlays,
            );
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_apply_call",
                debug::now_micros().saturating_sub(_t_apply_start),
            );
        } else {
            // Event-driven path: merge active-state axes into the
            // resolved application. Reading the signal subscribes this
            // effect to state changes, so a hover/press flip re-resolves
            // and re-applies through the regular apply_style path.
            //
            // Unwrap is safe: `states_signal` is only `None` when
            // `handles_states_natively == true`, in which case the
            // other branch above runs.
            let bits = states_signal.unwrap().get();
            let mut app = app;
            for axis in bits.active_axes() {
                app = app.with(axis, "on");
            }
            #[cfg(feature = "debug-stats")]
            let _t_resolve_start = debug::now_micros();
            // Breakpoints layer on top of the active-state resolution:
            // `merge_active_breakpoints` reads `current_breakpoint()`
            // (only when the sheet declares breakpoint blocks), which
            // subscribes this effect so a bucket flip re-fires it and
            // re-applies the right overlay — the native analog of the
            // browser re-evaluating `@media` on resize.
            let base = resolve_style(&app);
            let bp_overlays = resolve_breakpoint_overlays(&app);
            let resolved = merge_active_breakpoints(base, &bp_overlays);
            // Container overlays fold on top of the breakpoint cascade.
            // Reading `container_sig.get()` inside this Effect subscribes
            // it, so a container resize that crosses a threshold re-fires
            // and re-applies — the native analog of the browser
            // re-evaluating `@container` on a container resize. Costs
            // nothing when the sheet declares no `container` blocks
            // (`cq_overlays` empty → `merge_active_containers` returns
            // `resolved` untouched and we never read the signal).
            let cq_overlays = resolve_container_overlays(&app);
            let resolved = if cq_overlays.is_empty() {
                resolved
            } else {
                let width = container_sig.map(|s| s.get()).unwrap_or(0.0);
                merge_active_containers(resolved, &cq_overlays, width)
            };
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );

            #[cfg(feature = "debug-stats")]
            let _t_apply_start = debug::now_micros();
            warn_if_orphan_system_font(&resolved);
            backend_for_effect
                .borrow_mut()
                .apply_style(&handle.node, &resolved);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_apply_call",
                debug::now_micros().saturating_sub(_t_apply_start),
            );
        }

        #[cfg(feature = "debug-stats")]
        debug::record_apply_style_exit();

        #[cfg(feature = "debug-stats")]
        debug::record_apply_phase(
            "attach_style_first_run",
            debug::now_micros().saturating_sub(_t_first_run_start),
        );
    });

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_effect_alloc",
        debug::now_micros().saturating_sub(_t_effect_alloc_start),
    );

    #[cfg(feature = "debug-stats")]
    let _t_post_effect_start = debug::now_micros();

    // Hand the backend a setter so it can flip state bits from native
    // event listeners. The setter is `Rc<dyn Fn(StateBits, bool)>`
    // so the backend can clone it into per-event closures, and also
    // returned to the caller so it can wire prop-driven states like
    // `disabled` from the same signal.
    //
    // On natively-handling backends we have no `states_signal`, but
    // callers (e.g. `attach_disabled`) still hold the returned setter
    // and may invoke it from prop-driven flows. The setter is a no-op
    // in that case — `set_disabled` directly toggles the DOM
    // attribute, which is what activates `:disabled` CSS; we don't
    // need a Rust signal in between.
    let setter: Rc<dyn Fn(StateBits, bool)> = match states_signal {
        Some(sig) => Rc::new(move |bit, on| {
            sig.update(|bits| {
                *bits = if on { bits.with(bit) } else { bits.without(bit) };
            });
        }),
        None => Rc::new(|_, _| {}),
    };
    backend.borrow_mut().attach_states(node, setter.clone());

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_post_effect",
        debug::now_micros().saturating_sub(_t_post_effect_start),
    );
    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_total",
        debug::now_micros().saturating_sub(_t_total_start),
    );

    setter
}

/// For backends that handle states natively, resolve each declared
/// state overlay against the application's variants + theme. Walks
/// the stylesheet's variant keys looking for `__state_*` axes,
/// resolves each one with the corresponding axis set to `"on"`, and
/// returns `(StateBits, Rc<StyleRules>)` pairs the backend can emit
/// as pseudo-class CSS.
pub(super) fn resolve_state_overlays(app: &StyleApplication) -> Vec<(StateBits, Rc<StyleRules>)> {
    // Fast path: most stylesheets declare zero state blocks. The
    // cached slice is empty for them, so we skip both the
    // `variant_keys()` walk (which clones every axis/value String
    // out of the BTreeMap) AND any per-state resolve work.
    //
    // For 10k styled rows with no `state` blocks, this drops
    // `attach_style_resolve` from ~13μs per row to ~3μs — about a
    // 100ms total saving on the 10k-row case.
    let state_axes = app.sheet.state_axes();
    if state_axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(StateBits, Rc<StyleRules>)> = Vec::with_capacity(state_axes.len());
    for (bit, axis) in state_axes {
        // Resolve with this single state axis added on top of the
        // application's existing variants.
        let mut state_app = app.clone();
        state_app = state_app.with(axis.clone(), "on");
        let resolved = resolve_style(&state_app);
        out.push((*bit, resolved));
    }
    out
}

/// Breakpoint analog of [`resolve_state_overlays`]. Resolves each
/// declared `__bp_*` overlay axis against the application's variants +
/// theme, returning `(Breakpoint, Rc<StyleRules>)` pairs **sorted
/// ascending by breakpoint rank**.
///
/// Each entry is the *fully resolved* rules for that bucket (base
/// merged with the single breakpoint overlay), not just the overlay's
/// own properties — so two consumers can reproduce the mobile-first
/// min-width cascade by stacking them in order:
///
/// - **Web** emits each as `@media (min-width: <threshold>px) { .ui-x {
///   … } }`; the browser layers all matching media queries by source
///   order, higher breakpoints winning.
/// - **Native** ([`merge_active_breakpoints`]) folds the overlays whose
///   bucket is active at the current viewport width onto the base in
///   the same ascending order.
///
/// Returns an empty Vec (no allocation past the slice check) for the
/// common case of a sheet with no `breakpoint` blocks.
pub(super) fn resolve_breakpoint_overlays(
    app: &StyleApplication,
) -> Vec<(crate::Breakpoint, Rc<StyleRules>)> {
    let bp_axes = app.sheet.breakpoint_axes();
    if bp_axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(crate::Breakpoint, Rc<StyleRules>)> = Vec::with_capacity(bp_axes.len());
    for (bp, axis) in bp_axes {
        let mut bp_app = app.clone();
        bp_app = bp_app.with(axis.clone(), "on");
        let resolved = resolve_style(&bp_app);
        out.push((*bp, resolved));
    }
    out.sort_by_key(|(bp, _)| bp.rank());
    out
}

/// For backends that don't handle breakpoints natively (everything but
/// web), fold the overlays whose bucket is active at the current
/// viewport width onto the base rules, lowest breakpoint first so
/// higher breakpoints win — the same mobile-first min-width cascade the
/// web backend gets from stacked `@media` rules.
///
/// Reads [`crate::current_breakpoint`] *only when overlays are present*,
/// so the caller's `Effect` (or the theme cohort driver) re-subscribes
/// and re-fires when the bucket changes — but a style with no
/// breakpoint blocks pays nothing and never subscribes. Returns the
/// original `base` Rc untouched when no overlay is active (e.g. a phone
/// width below `sm`), so the common cases allocate no new `StyleRules`.
fn merge_active_breakpoints(
    base: Rc<StyleRules>,
    overlays: &[(crate::Breakpoint, Rc<StyleRules>)],
) -> Rc<StyleRules> {
    if overlays.is_empty() {
        return base;
    }
    let current = crate::current_breakpoint().get();
    let mut merged: Option<StyleRules> = None;
    for (bp, overlay) in overlays {
        if bp.rank() <= current.rank() {
            // `StyleRules::merge` consumes `self` and returns the
            // overlaid result (overlay's `Some(_)` fields win), so we
            // thread the accumulator through by value rather than
            // mutating in place.
            let acc = merged.take().unwrap_or_else(|| (*base).clone());
            merged = Some(acc.merge(overlay));
        }
    }
    match merged {
        Some(rules) => Rc::new(rules),
        None => base,
    }
}

/// Container-query analog of [`resolve_breakpoint_overlays`]. Resolves
/// each declared `__cq_minw_*` overlay axis against the application's
/// variants + theme, returning `(threshold_px, Rc<StyleRules>)` pairs
/// **sorted ascending by threshold**.
///
/// Each entry is the fully resolved rules for that overlay (base merged
/// with the single container overlay), so two consumers reproduce the
/// mobile-first min-width cascade by stacking them in order:
///
/// - **Web** emits each as `@container (min-width: <threshold>px) {
///   .ui-x { … } }`; the browser layers all matching container queries
///   by source order, higher thresholds winning, against the nearest
///   `container-type` ancestor.
/// - **Native** ([`merge_active_containers`]) folds the overlays whose
///   threshold is `<=` the nearest container's resolved inline-size onto
///   the base in the same ascending order.
///
/// Returns an empty Vec (no allocation past the slice check) for the
/// common case of a sheet with no `container` blocks.
pub(super) fn resolve_container_overlays(app: &StyleApplication) -> Vec<(f32, Rc<StyleRules>)> {
    let cq_axes = app.sheet.container_axes();
    if cq_axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(f32, Rc<StyleRules>)> = Vec::with_capacity(cq_axes.len());
    for (threshold, axis) in cq_axes {
        let mut cq_app = app.clone();
        cq_app = cq_app.with(axis.clone(), "on");
        let resolved = resolve_style(&cq_app);
        out.push((*threshold, resolved));
    }
    // Ascending by threshold so the mobile-first cascade (lower
    // threshold first, higher wins) is just an in-order fold. Thresholds
    // come from compile-time literals, so NaN can't occur.
    out.sort_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// For backends that don't handle container queries natively (everything
/// but web), fold the overlays whose `min_width` threshold is `<=` the
/// nearest container's resolved inline-size onto the base, lowest
/// threshold first so higher thresholds win — the same mobile-first
/// min-width cascade the web backend gets from stacked `@container`
/// rules.
///
/// `container_width` is the nearest ancestor containment context's
/// resolved inline-size (`0.0` when there is no container ancestor,
/// activating no overlay). The caller reads it from the build-time
/// container stack's signal *inside the style Effect*, so the Effect
/// re-subscribes and re-fires when the container's width crosses a
/// threshold. Returns the original `base` Rc untouched when no overlay
/// is active, so the common cases allocate no new `StyleRules`.
pub(super) fn merge_active_containers(
    base: Rc<StyleRules>,
    overlays: &[(f32, Rc<StyleRules>)],
    container_width: f32,
) -> Rc<StyleRules> {
    if overlays.is_empty() {
        return base;
    }
    let mut merged: Option<StyleRules> = None;
    for (threshold, overlay) in overlays {
        if *threshold <= container_width {
            let acc = merged.take().unwrap_or_else(|| (*base).clone());
            merged = Some(acc.merge(overlay));
        }
    }
    match merged {
        Some(rules) => Rc::new(rules),
        None => base,
    }
}

/// Reactive disabled-state wiring. Runs the user's closure inside an
/// `Effect` so the result tracks any signals it reads. On each
/// firing: (1) calls `Backend::set_disabled` so the native widget
/// is marked inert (web `disabled` attr, Android `setEnabled`); and
/// (2) flips the `DISABLED` state bit on the styled node so any
/// `state disabled { ... }` overlay applies via the existing state
/// machinery. If the button has no styled effect, `state_setter` is
/// `None` and step 2 is skipped.
pub(super) fn attach_disabled<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    disabled: Box<dyn Fn() -> bool>,
    state_setter: Option<Rc<dyn Fn(StateBits, bool)>>,
    // Optional handler-level press-block flag. `set_disabled` only makes
    // *native form controls* inert (a `<button disabled>` / `setEnabled:NO`);
    // a bare pressable lowers to a `<div>`/plain view whose `set_disabled` is
    // a CSS hook at best and a no-op at worst, so the click/keydown handler
    // would still fire. Threading the disabled state into this shared flag —
    // consulted by the wrapped `on_click` — blocks the press uniformly across
    // every backend (rule #7: fix upstream, every backend converges), instead
    // of relying on per-backend gesture plumbing that doesn't exist.
    press_block: Option<Rc<std::cell::Cell<bool>>>,
) {
    let node_for_effect = node.clone();
    let backend_for_effect = backend.clone();
    let _e = Effect::new(move || {
        let d = disabled();
        if let Some(flag) = press_block.as_ref() {
            flag.set(d);
        }
        backend_for_effect
            .borrow_mut()
            .set_disabled(&node_for_effect, d);
        if let Some(setter) = state_setter.as_ref() {
            setter(StateBits::DISABLED, d);
        }
    });
}

#[cfg(test)]
mod breakpoint_tests {
    use super::*;
    use crate::style::StyleSheet;
    use crate::{set_viewport_size, Breakpoint, Length, Tokenized, ViewportSize};

    fn px(p: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(p)))
    }

    fn width_of(rules: &StyleRules) -> Length {
        *rules
            .width
            .as_ref()
            .expect("width is set in these fixtures")
            .value()
    }

    /// A sheet: base `width: 100`, `breakpoint md { width: 500 }`,
    /// `breakpoint lg { width: 900 }` — declared out of rank order to
    /// prove `resolve_breakpoint_overlays` sorts, not just preserves
    /// declaration order.
    fn responsive_app() -> StyleApplication {
        let sheet = Rc::new(
            StyleSheet::new(|_vs| StyleRules { width: px(100.0), ..Default::default() })
                // lg declared BEFORE md on purpose.
                .variant("__bp_lg", "on", |_vs| StyleRules { width: px(900.0), ..Default::default() })
                .variant("__bp_md", "on", |_vs| StyleRules { width: px(500.0), ..Default::default() }),
        );
        StyleApplication::new(sheet)
    }

    #[test]
    fn resolve_breakpoint_overlays_sorts_ascending_and_resolves_each() {
        let app = responsive_app();
        let overlays = resolve_breakpoint_overlays(&app);
        assert_eq!(overlays.len(), 2, "two breakpoint overlays declared");
        // Sorted ascending by rank regardless of declaration order.
        assert_eq!(overlays[0].0, Breakpoint::Md);
        assert_eq!(overlays[1].0, Breakpoint::Lg);
        // Each overlay is the FULLY resolved rules for that bucket
        // (base merged with the bp overlay), not just the overlay's own
        // props — so consumers can stack them.
        assert_eq!(width_of(&overlays[0].1), Length::Px(500.0));
        assert_eq!(width_of(&overlays[1].1), Length::Px(900.0));
    }

    #[test]
    fn resolve_breakpoint_overlays_empty_without_breakpoint_blocks() {
        let sheet = Rc::new(StyleSheet::new(|_vs| StyleRules { width: px(100.0), ..Default::default() }));
        let app = StyleApplication::new(sheet);
        assert!(resolve_breakpoint_overlays(&app).is_empty());
    }

    #[test]
    fn merge_active_breakpoints_layers_mobile_first_by_viewport_width() {
        let app = responsive_app();
        let base = resolve_style(&app);
        let overlays = resolve_breakpoint_overlays(&app);

        // Below sm: no overlay active → base width, and the SAME Rc is
        // returned (no needless allocation on the common mobile path).
        set_viewport_size(ViewportSize::new(390.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(100.0));
        assert!(Rc::ptr_eq(&merged, &base), "no active overlay must reuse the base Rc");

        // Md bucket: only the md overlay is active (lg is above) →
        // width 500.
        set_viewport_size(ViewportSize::new(800.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(500.0));

        // Lg bucket: md AND lg both active (min-width cumulative),
        // lg wins on the conflicting `width` → width 900.
        set_viewport_size(ViewportSize::new(1100.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(900.0));
    }
}

#[cfg(test)]
mod container_tests {
    use super::*;
    use crate::container_query::container_axis_name;
    use crate::style::StyleSheet;
    use crate::{Length, Tokenized};

    fn px(p: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(p)))
    }

    fn width_of(rules: &StyleRules) -> Length {
        *rules.width.as_ref().expect("width is set in these fixtures").value()
    }

    /// A sheet: base `width: 100`, `container (min_width: 600) { width: 900 }`,
    /// `container (min_width: 300) { width: 500 }` — declared with the
    /// larger threshold FIRST to prove `resolve_container_overlays` sorts
    /// ascending by threshold, not by declaration order.
    fn container_app() -> StyleApplication {
        let sheet = Rc::new(
            StyleSheet::new(|_vs| StyleRules { width: px(100.0), ..Default::default() })
                .variant(container_axis_name(600.0), "on", |_vs| StyleRules {
                    width: px(900.0),
                    ..Default::default()
                })
                .variant(container_axis_name(300.0), "on", |_vs| StyleRules {
                    width: px(500.0),
                    ..Default::default()
                }),
        );
        StyleApplication::new(sheet)
    }

    #[test]
    fn resolve_container_overlays_sorts_ascending_and_resolves_each() {
        let app = container_app();
        let overlays = resolve_container_overlays(&app);
        assert_eq!(overlays.len(), 2, "two container overlays declared");
        // Ascending by threshold regardless of declaration order.
        assert_eq!(overlays[0].0, 300.0);
        assert_eq!(overlays[1].0, 600.0);
        // Each overlay is the FULLY resolved rules (base merged with the
        // overlay), so consumers can stack them mobile-first.
        assert_eq!(width_of(&overlays[0].1), Length::Px(500.0));
        assert_eq!(width_of(&overlays[1].1), Length::Px(900.0));
    }

    #[test]
    fn resolve_container_overlays_empty_without_container_blocks() {
        let sheet =
            Rc::new(StyleSheet::new(|_vs| StyleRules { width: px(100.0), ..Default::default() }));
        let app = StyleApplication::new(sheet);
        assert!(resolve_container_overlays(&app).is_empty());
    }

    #[test]
    fn merge_active_containers_layers_mobile_first_by_container_width() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);

        // Container narrower than the smallest threshold → no overlay, and
        // the SAME Rc is returned (no needless allocation).
        let merged = merge_active_containers(base.clone(), &overlays, 200.0);
        assert_eq!(width_of(&merged), Length::Px(100.0));
        assert!(Rc::ptr_eq(&merged, &base), "no active overlay must reuse the base Rc");

        // Between 300 and 600: only the 300 overlay is active → width 500.
        let merged = merge_active_containers(base.clone(), &overlays, 450.0);
        assert_eq!(width_of(&merged), Length::Px(500.0));

        // At/above 600: both active (min-width cumulative), the higher
        // threshold wins on the conflicting `width` → width 900.
        let merged = merge_active_containers(base.clone(), &overlays, 700.0);
        assert_eq!(width_of(&merged), Length::Px(900.0));

        // Exactly at a threshold is inclusive (min-width semantics).
        let merged = merge_active_containers(base.clone(), &overlays, 300.0);
        assert_eq!(width_of(&merged), Length::Px(500.0));
    }

    /// Container width 0 (no container ancestor / unwired backend)
    /// activates nothing — the node renders at its mobile-first base.
    #[test]
    fn merge_active_containers_zero_width_is_base() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);
        let merged = merge_active_containers(base.clone(), &overlays, 0.0);
        assert!(Rc::ptr_eq(&merged, &base));
    }

    /// Convergence property: merging at the SAME width twice yields
    /// byte-identical rules. The native feedback loop relies on this —
    /// after a restyle, the container's width is unchanged (inline-size
    /// containment), so re-resolving produces the same result and the
    /// change-guarded signal never re-fires.
    #[test]
    fn merge_active_containers_is_idempotent_at_fixed_width() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);
        let a = merge_active_containers(base.clone(), &overlays, 700.0);
        let b = merge_active_containers(base.clone(), &overlays, 700.0);
        assert_eq!(width_of(&a), width_of(&b));
    }
}
