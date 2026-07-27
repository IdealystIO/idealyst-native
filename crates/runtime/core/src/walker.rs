//! The render walker.
//!
//! `render(backend, primitive_tree)` (and the closure-taking
//! [`mount`]) is the entry point: it sets up a reactive `Scope`,
//! walks the primitive tree via the [`build`] dispatcher, hands the
//! resulting backend node off to `Backend::finish`, and returns an
//! [`Owner`] whose `Drop` tears down everything reactive that was
//! created.
//!
//! Internally this module is split by primitive — each
//! `Element::X` variant has its own `walker::x` submodule with a
//! `build(...)` function that owns that primitive's mount logic
//! (initial create, attach_style, reactive Effects, ref_fill,
//! cleanup hooks). The dispatcher [`build_inner`] below is just a
//! match-and-delegate.
//!
//! Cross-cutting infrastructure also lives in submodules:
//! - [`style`] — `attach_style` + state overlays + safe-area opt-in.
//! - [`theme_cohort`] — shared theme-change subscription.
//! - [`cleanup`] — RAII wrappers that call `Backend::release_*`.
//! - [`debug`] — `time_backend_create` + the `PrimitiveKind` mapper.
//! - [`robot`] — robot-feature metadata extraction (cfg-gated).
//!
//! Public surface from this module: just `render`, `mount`, and
//! `Owner`. The rest is implementation detail.

use crate::backend::Backend;
use crate::element::Element;
use crate::reactive;
use std::cell::RefCell;
use std::rc::Rc;

// `pkind!` produces a `PrimitiveKind` tag when the debug feature is
// on, and `()` when off. Paired with `debug::time_backend_create`,
// this keeps call sites identical between build modes without
// scattering `#[cfg]` attributes through the walker. Defined here at
// the parent so `pub(crate) use pkind;` below makes it importable
// from every submodule via `use super::pkind;`.
#[cfg(feature = "debug-stats")]
macro_rules! pkind {
    ($variant:ident) => {
        $crate::debug::PrimitiveKind::$variant
    };
}
#[cfg(not(feature = "debug-stats"))]
macro_rules! pkind {
    ($variant:ident) => {
        ()
    };
}

#[cfg(feature = "prim-activity")]
mod activity_indicator;
mod button;
mod cleanup;
mod debug;
mod dynamic;
mod each;
mod external;
#[cfg(feature = "prim-graphics")]
mod graphics;
#[cfg(feature = "prim-icon")]
mod icon;
#[cfg(feature = "prim-image")]
mod image;
#[cfg(feature = "prim-lazy")]
mod lazy;
mod link;
#[cfg(feature = "prim-navigator")]
mod navigator;
#[cfg(feature = "prim-portal")]
mod portal;
#[cfg(feature = "prim-presence")]
mod presence;
mod pressable;
#[cfg(feature = "robot")]
mod robot;
mod scroll_view;
#[cfg(feature = "prim-slider")]
mod slider;
mod style;
mod text;
#[cfg(feature = "prim-text-input")]
mod text_input;
mod theme_cohort;
#[cfg(feature = "prim-toggle")]
mod toggle;
mod view;
#[cfg(feature = "prim-virtualizer")]
mod virtualizer;
mod when_switch;

/// Owns the reactive state created by a render call. Dropping the `Owner`
/// drops its `Scope`, which frees every signal and effect created during
/// rendering — no leaks across the boundary.
pub struct Owner {
    // Boxed so we can hand out a `&mut Scope` to `with_scope` calls inside
    // reactive subtree rebuilds without invalidating other references.
    // Field is dropped-only: it's never read, but its `Drop` impl is what
    // actually frees the arena slots.
    #[allow(dead_code)]
    scope: Box<reactive::Scope>,
}

/// Render a pre-built `Element` tree under `backend`.
///
/// The root reactive scope wraps the build walk only — the tree
/// itself is already a value by the time it's handed in. That means
/// any signals / effects / refs declared by the caller while
/// constructing `tree` (e.g. inside an `app()` function called by the
/// host glue as `render(backend, app())`) run *outside* any active
/// scope and aren't adopted by the returned `Owner`.
///
/// In practice this usually doesn't matter — most reactive primitives
/// happily leak for the lifetime of the page. The exception is
/// `effect!`: with no scope to adopt the new effect, the macro's
/// hidden handle drops at the end of its block and the effect's
/// cleanups fire immediately. Any timers scheduled inside (via
/// `after_ms` + `on_cleanup`) get cancelled before they fire. See
/// [`mount`] for the closure-taking variant that fixes this by
/// running the constructor inside the root scope.
#[must_use = "drop the Owner to dispose the UI; keep it alive to keep the UI reactive"]
pub fn render<B: Backend + 'static>(backend: Rc<RefCell<B>>, tree: Element) -> Owner {
    mount(backend, move || tree)
}

/// Render the tree produced by `tree_fn` under `backend`.
///
/// Mirrors [`render`] but takes a closure instead of a pre-built
/// `Element`. The closure runs *inside* the root reactive scope,
/// so any signals, effects, and refs declared by the closure are
/// adopted by the returned `Owner`. That makes patterns like
///
/// ```ignore
/// mount(backend, || {
///     let phase = signal(0u8);
///     effect!({
///         std::mem::forget(after_ms(900, move || phase.set(1)));
///     });
///     app(phase)
/// });
/// ```
///
/// behave the way author code expects: the `effect!` and its
/// scheduled tasks live until the `Owner` drops at page teardown,
/// not until the macro's hidden handle goes out of scope microseconds
/// later.
///
/// New host-glue code should prefer `mount` over [`render`]. Both
/// produce the same kind of `Owner`.
#[must_use = "drop the Owner to dispose the UI; keep it alive to keep the UI reactive"]
pub fn mount<B, F>(backend: Rc<RefCell<B>>, tree_fn: F) -> Owner
where
    B: Backend + 'static,
    F: FnOnce() -> Element,
{
    // Stash the backend's cascade capability so the theme-cohort
    // driver (installed lazily inside `build`) can short-circuit
    // its fan-out on token-only updates without holding a backend
    // reference. Read once here; the value can't change for the
    // lifetime of this `Owner`.
    theme_cohort::set_backend_cascade_tokens(
        backend.borrow().token_updates_propagate_via_cascade(),
    );

    // Stash the backend's platform identity so author code can
    // branch on host via `runtime_core::platform()` without
    // holding a Backend reference. Same one-shot read as above —
    // Backend impls return a constant per instance.
    let platform = backend.borrow().platform();
    crate::backend::install_current_platform(platform);

    // Stash the backend's reported color scheme so author code can read
    // the platform's light/dark default via `runtime_core::color_scheme()`
    // at startup and install a matching theme (avoids a wrong-theme flash).
    let scheme = backend.borrow().color_scheme();
    crate::backend::install_current_color_scheme(scheme);

    // Install the platform-appropriate default monotonic clock unless
    // the host already wired one. Native hosts get an
    // `InstantTimeSource`; `Web` is skipped (its backend installs a
    // `performance.now()` source during bootstrap, and `Instant::now()`
    // panics on wasm). Branching on the runtime `Platform` here keeps
    // the clock free of a `#[cfg(target_arch)]` fallback.
    crate::time::install_default_time_source(platform);

    // Stash the backend's external-URL opener so author code can fire
    // `runtime_core::open_url(...)` from any event handler without a
    // Backend reference. Same one-shot read as the platform identity —
    // the opener is a self-contained closure that calls a platform
    // singleton, so it survives past this borrow.
    crate::backend::install_url_opener(backend.borrow().url_opener());

    // Stash the backend's full-screen / immersive-mode setter so author
    // code can call `runtime_core::set_fullscreen(...)` from any event
    // handler without a Backend reference. Same one-shot read as the URL
    // opener — a self-contained closure making a window/system call.
    crate::backend::install_fullscreen_setter(backend.borrow().fullscreen_setter());

    // Stash an accessibility announcer so author code can post a
    // live-region announcement via `runtime_core::announce(...)` from any
    // event handler. Unlike the URL opener / full-screen setter, the
    // backend's `announce_for_accessibility` takes `&mut self`, so the
    // closure captures the backend handle and borrows it on each call.
    {
        let announce_backend = backend.clone();
        crate::backend::install_announcer(Some(Rc::new(
            move |msg: &str, priority: crate::accessibility::LiveRegionPriority| {
                announce_backend
                    .borrow_mut()
                    .announce_for_accessibility(msg, priority);
            },
        )));
    }

    // Auto-start the Robot bridge when the `dev` feature is on so
    // the MCP server's runtime tools can attach without the user
    // wiring `bridge::start(...)` themselves. The call is
    // idempotent (subsequent mounts won't bind a second listener)
    // and a no-op without the feature.
    //
    // Gated to non-wasm targets — the bridge uses `std::net::TcpListener`
    // + `std::thread::spawn`, neither of which is available on
    // `wasm32-unknown-unknown`. Web dev gets the catalog via the
    // server-side path (CLI's `--from-bin` + the user's app's
    // emitted JSON); runtime control of the wasm app is a separate
    // transport (out of scope here).
    #[cfg(all(feature = "robot", not(target_arch = "wasm32")))]
    {
        // Universal transport: when a relay URL is configured (dev/device
        // tooling sets `IDEALYST_ROBOT_RELAY_URL`), DIAL the relay — the same
        // path web takes — so every platform reaches the MCP server the same
        // way. Otherwise self-host a TCP bridge (standalone desktop, no relay
        // running). Either way the MCP side sees an identical TCP bridge.
        if let Some(url) = crate::robot::bridge::relay_url_from_env() {
            crate::robot::bridge::start_relay_client(url);
        } else {
            crate::robot::bridge::start_auto_polling(crate::robot::bridge::DEFAULT_PORT);
        }
        // Register the live `"screenshot"` verb only when this backend
        // can snapshot its real surface. Gating on the capability keeps a
        // `MockBackend` (or any backend without native capture) from
        // shadowing the headless wgpu-replay `"screenshot"` the
        // dev-server registers for mocked sessions. The capture closure
        // borrows the backend on the UI thread — the same thread the
        // bridge polls on — so no cross-thread handoff is needed.
        if backend.borrow().supports_screenshot() {
            let backend = backend.clone();
            crate::robot::screenshot::register_native_screenshot(move |done| {
                backend.borrow().capture_screenshot(done);
            });
        }
    }

    let mut scope = Box::new(reactive::Scope::new());
    let root = reactive::with_scope(&mut scope, || {
        // Install the theme-cohort driver EAGERLY, while the ROOT scope
        // is the active one, so the driver's lifetime is the whole
        // mount. Installed lazily (at the first static-style attach) it
        // would be owned by whatever scope happens to be active THEN —
        // under the outlet navigation model that's typically the first
        // SCREEN's scope, and navigating away (`LazyDisposing`) dropped
        // the driver and WIPED the whole cohort map, orphaning every
        // still-mounted static-styled node (the navigator chrome). The
        // visible bug: after any navigation, a theme toggle re-tinted
        // screen content but the sidebar/header background stayed on
        // the old theme (native only — web re-tints via CSS vars).
        theme_cohort::install_theme_cohort_driver(&backend);
        // Both the tree constructor and the build walk run inside the
        // same root scope. Reactive primitives created during
        // construction adopt this scope and are freed on `Owner`
        // drop alongside the per-build effects that wire them up.
        let tree = tree_fn();
        build(&backend, 0, tree)
    });
    // Drain the navigator SDK's deferred chrome/screen microtasks NOW —
    // adoption/build window still open (`finish` not yet run), no backend borrow
    // held — so they fire BEFORE the first layout instead of post-`finish`
    // (which re-builds fresh and, on native, paints once without the chrome —
    // the "toolbar renders a frame late" bug). Two schedulers buffer for this:
    //   - web, during SSR hydration (adopt the server's DOM, not rebuild), and
    //   - the Apple hosts, during the initial mount (`begin_mount_buffering`),
    //     so the drawer header/sidebar land in the first paint.
    // No-op for any scheduler that isn't buffering (nothing queued to drain).
    crate::scheduling::drain_buffered_microtasks();
    backend.borrow_mut().finish(root);
    // Forward any page metadata an author screen declared during the
    // build to the backend (SSR emits <head>; most backends no-op).
    if let Some(meta) = crate::page_meta::take_page_metadata() {
        backend.borrow_mut().set_page_metadata(&meta);
    }
    Owner { scope }
}

// =============================================================================
// Detached build + adopt ambient
//
// `build_detached` materializes a standalone `Element` subtree under
// `backend` *outside* any `mount`/`render` call — the runtime-server
// `dev-client` uses it to build the navigator's chrome (sidebar/screen)
// from server-pushed primitive subtrees. It mirrors the `mount` body's
// scope setup (new root `Scope` + `with_scope`) so the External cleanup
// Effect and any theme subscriptions created during the build have a
// live scope to adopt; the returned `DetachedScope` must be retained by
// the caller or those effects fire their cleanups immediately.
//
// The `adopt` parameter threads a pre-built backend node into the walk:
// when the build encounters an `Element::External` whose `type_id`
// matches `adopt.0`, the external build path returns `adopt.1` instead
// of calling `create_external` (see `walker::external::build`). This is
// the wire client's "adopt sentinel" — the SDK's `leading_slot` stamps
// an `Element::External` with a known marker `TypeId`, and `dev-client`
// passes its holder node as the adopt node so the handler's wrapper
// (e.g. iOS's `scroll_view`) materializes for real *around* the holder
// while the leaf adopts it.
//
// Why an ambient thread-local (not a cross-crate global): the writer
// (`build_detached`) and the reader (`external::build`) are BOTH in
// runtime-core, so they live in the same `wasm-split` chunk and observe
// the same thread-local. A prior design staged the holder in a global
// owned by `wire`/`dev-client` and read it from the `drawer-navigator`
// SDK — different chunks, and `wasm-split` does not keep a cross-crate
// mutable global coherent (it duplicates the data), so the reader saw
// `None`. Keeping both ends inside runtime-core (exactly like the
// `CURRENT_IDENTITY` ambient, which works fine across chunks) sidesteps
// that entirely. See [[project_navigator_over_wire_wip]].
// =============================================================================

thread_local! {
    // The node `build_detached` staged for the External-adopt path to
    // return. `RefCell<Option<...>>` (not `Cell`) because the value is
    // not `Copy`; save/restore the previous value so nesting is safe.
    static CURRENT_ADOPT: RefCell<Option<(std::any::TypeId, Rc<dyn std::any::Any>)>> =
        const { RefCell::new(None) };
}

/// Set the adopt node for the duration of `f`. Restores the previous
/// value on return (RAII), so nested `build_detached` calls compose.
fn with_adopt<R>(
    adopt: Option<(std::any::TypeId, Rc<dyn std::any::Any>)>,
    f: impl FnOnce() -> R,
) -> R {
    let prev = CURRENT_ADOPT
        .try_with(|c| c.replace(adopt))
        .unwrap_or(None);
    struct Guard(Option<(std::any::TypeId, Rc<dyn std::any::Any>)>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = CURRENT_ADOPT.try_with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _g = Guard(prev);
    f()
}

/// Read the currently-staged adopt node, if any. Called from the
/// External build path before `create_external`.
pub(super) fn current_adopt() -> Option<(std::any::TypeId, Rc<dyn std::any::Any>)> {
    CURRENT_ADOPT
        .try_with(|c| c.borrow().clone())
        .unwrap_or(None)
}

/// Owns the reactive `Scope` created by a [`build_detached`] call.
/// Drop it to dispose the detached subtree's reactive state (cleanup
/// Effects, theme subscriptions); keep it alive to keep that subtree
/// reactive. Mirrors [`Owner`] but for a subtree built outside a mount.
pub struct DetachedScope {
    #[allow(dead_code)]
    _scope: Box<reactive::Scope>,
}

/// Materialize a standalone `Element` subtree under `backend`, outside
/// any active `mount`/`render`. Returns the root backend node plus a
/// [`DetachedScope`] the caller MUST retain (dropping it disposes the
/// subtree's reactive state).
///
/// `adopt` optionally threads a pre-built node into the walk: an
/// `Element::External` whose `type_id` equals `adopt.0` adopts `adopt.1`
/// instead of calling `Backend::create_external`. See the module-level
/// comment above for the wire-client adopt-sentinel use case.
pub fn build_detached<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    element: Element,
    adopt: Option<(std::any::TypeId, B::Node)>,
) -> (B::Node, DetachedScope) {
    let mut scope = Box::new(reactive::Scope::new());
    let identity = crate::Identity::node(crate::current_identity(), 0, None, None);
    // Erase the adopt node to `Rc<dyn Any>` so the runtime-core-internal
    // ambient is backend-agnostic; the External build path downcasts it
    // back to `B::Node`.
    let adopt_any = adopt.map(|(tid, node)| (tid, Rc::new(node) as Rc<dyn std::any::Any>));
    let node = reactive::with_scope(&mut scope, || {
        with_adopt(adopt_any, || {
            crate::with_current_identity(identity, || build(backend, 0, element))
        })
    });
    (node, DetachedScope { _scope: scope })
}

/// Build a `Element` subtree. `slot` is the emission's position in
/// its parent's children (or its branch index inside a conditional /
/// switch arm). Combined with the ambient
/// [`current_identity()`][crate::current_identity] this determines
/// the stable [`Identity`][crate::Identity] for every `backend.create_*`
/// call inside the subtree — the runtime-server recorder uses that identity to
/// keep wire `NodeId`s consistent across sidecar respawns.
///
/// Callers in iteration loops pass the loop index; standalone /
/// sole-occupant call sites pass `0`. Branch sites
/// (`when` / `switch` / `if-else`) pass the branch index so the two
/// arms get distinct identities.
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    slot: u32,
    node: Element,
) -> B::Node {
    // Compute this emission's Identity from the ambient parent + our
    // slot. `with_current_identity` makes it the new parent for any
    // recursive `build(...)` calls inside this body — see the doc
    // comment on `crate::identity` for the model.
    let parent = crate::current_identity();
    let my_identity = crate::Identity::node(parent, slot, None, None);
    crate::with_current_identity(my_identity, move || build_inner(backend, node))
}

pub(super) fn build_inner<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: Element,
) -> B::Node {
    // Robot: a `#[component]` with `#[method]` fns wraps its root primitive in
    // `Element::Component` (via `__component_root`). Unwrap it BEFORE anything
    // else sees it — arm the element↔component link, then build the real
    // child. The child's registration (just below, on recursion) consumes the
    // pending link, mapping the component instance to its root element id.
    #[cfg(feature = "robot")]
    if let Element::Component { instance, child } = node {
        crate::robot::set_pending_component_link(instance);
        return build_inner(backend, *child);
    }

    // Walker-level timing. Record the kind once on entry; the matching
    // exit fires after the match returns. Tag covers the full subtree
    // build (children inclusive). Each backend create call below
    // records its own narrower BackendCreate pair.
    #[cfg(feature = "debug-stats")]
    let _debug_kind = debug::debug_kind_of(&node);
    #[cfg(feature = "debug-stats")]
    crate::debug::record_build_enter(_debug_kind);

    // Robot: extract metadata and pre-register so children see us as parent.
    #[cfg(feature = "robot")]
    let robot_id = {
        if let Some(meta) = robot::robot_extract_meta(&node) {
            use crate::robot::{self, RegistryEntry};
            let parent = robot::current_parent();
            let id = robot::register(RegistryEntry {
                kind: meta.kind,
                test_id: meta.test_id,
                label: meta.label,
                label_fn: meta.label_fn,
                actions: meta.actions,
                parent,
                children: Vec::new(),
            });
            // Link child → parent.
            if let Some(pid) = parent {
                robot::add_child(pid, id);
            }
            // If a `#[component]` wrapper armed a pending link, this element
            // is that component's root primitive — record element↔component.
            if let Some(instance) = robot::take_pending_component_link() {
                robot::link_component_element(instance, id);
            }
            // Deregister when this element's owning reactive scope drops.
            // A `when`/`switch`/`each` branch builds inside a fresh
            // `Scope` (via `with_scope`); when the condition flips and the
            // old scope is dropped, this fires and removes the stale
            // entry. Without it the robot registry leaks every torn-down
            // branch as a phantom live root in `snapshot()` — the
            // double-live-root the AAS host reported (onboarding subtree
            // surviving alongside the main screen). Registration runs
            // inside `untrack`, so `on_cleanup` anchors to the active
            // SCOPE (not the outer `When` effect, which would re-run, not
            // drop, on every flip).
            crate::reactive::on_cleanup(move || robot::deregister(id));
            robot::push_parent(id);
            Some(id)
        } else {
            None
        }
    };

    // Dispatch on the variant discriminant, then call the matching
    // per-variant `dispatch_*` function through a single function
    // pointer. Both the discriminant match and the call live in
    // `build_inner` — but because there's exactly ONE call site, the
    // compiler only reserves arg-passing slots for `Element` once.
    //
    // Why: the previous shape (one dispatch_X call per arm) made the
    // compiler reserve a separate arg-copy slot per arm — 23 × ~1.8
    // KiB = ~40 KiB just for the by-value `Element` arg moving into
    // each call. Even at `opt-level = "z"` LLVM didn't merge them
    // (the arms are mutually exclusive but the slot allocator
    // doesn't see that). One call site, one slot.
    //
    // Combined with pushing the destructure into the per-variant
    // functions, this collapses `build_inner`'s frame from ~77 KiB
    // (the original "destructure inline in every arm" shape, which
    // blew the 1 MiB wasm stack at ~13 levels of recursion and
    // surfaced as the `RuntimeError: memory access out of bounds`
    // crash on `/demo`) down to roughly `sizeof(Element)` + a few
    // words.
    //
    // The function-pointer call is monomorphic (`B` is fixed for any
    // given build), so this is a single direct call after match
    // selection — no virtual dispatch overhead.
    // Capture editable-text-ness before `node` is moved into the dispatcher, so
    // the post-dispatch robot block can wire a `focus`/`blur` action (the node
    // exists only after dispatch). Lets the robot drive real input focus.
    #[cfg(feature = "robot")]
    let robot_is_text_input = matches!(&node, Element::TextInput { .. });
    // Same pre-dispatch capture for scroll views: they get a `set_scroll`
    // action (the programmatic pan analogue e2e drives use on platforms
    // with no scriptable touch input, e.g. the iOS simulator).
    #[cfg(feature = "robot")]
    let robot_is_scroll_view = matches!(&node, Element::ScrollView { .. });

    type Dispatcher<B> = fn(&Rc<RefCell<B>>, Element) -> <B as Backend>::Node;
    let dispatcher: Dispatcher<B> = match &node {
        Element::Text { .. } => dispatch_text::<B>,
        Element::View { .. } => dispatch_view::<B>,
        Element::Pressable { .. } => dispatch_pressable::<B>,
        Element::Button { .. } => dispatch_button::<B>,
        Element::Image { .. } => dispatch_image::<B>,
        Element::Icon { .. } => dispatch_icon::<B>,
        Element::TextInput { .. } => dispatch_text_input::<B>,
        Element::TextArea { .. } => dispatch_text_area::<B>,
        Element::Toggle { .. } => dispatch_toggle::<B>,
        Element::ScrollView { .. } => dispatch_scroll_view::<B>,
        Element::Slider { .. } => dispatch_slider::<B>,
        Element::ActivityIndicator { .. } => dispatch_activity_indicator::<B>,
        Element::Virtualizer { .. } => dispatch_virtualizer::<B>,
        Element::Graphics { .. } => dispatch_graphics::<B>,
        Element::When { .. } => dispatch_when::<B>,
        Element::Switch { .. } => dispatch_switch::<B>,
        Element::Each { .. } => dispatch_each::<B>,
        Element::Dynamic { .. } => dispatch_dynamic::<B>,
        Element::Link { .. } => dispatch_link::<B>,
        Element::External { .. } => dispatch_external::<B>,
        Element::Navigator { .. } => dispatch_navigator::<B>,
        Element::NavigatorOutlet { .. } => dispatch_navigator_outlet::<B>,
        Element::Portal { .. } => dispatch_portal::<B>,
        Element::Presence { .. } => dispatch_presence::<B>,
        Element::Lazy { .. } => dispatch_lazy::<B>,
        Element::Repeat { .. } => {
            // `Repeat` represents N sibling nodes, not a single
            // node. It can only appear inside a parent's children
            // list, where `insert_children` expands it inline.
            // Reaching this arm means a `Repeat` was used outside
            // a children context — author or macro bug.
            panic!(
                "Element::Repeat encountered as a standalone subtree root. \
                 Repeat is a children-list primitive (used for `for` loops \
                 inside `ui!`); it cannot be the result of a `build()` call \
                 on its own. Wrap it in a View / ScrollView / fragment."
            );
        }
        Element::Fragment { .. } => dispatch_fragment::<B>,
        // Unwrapped at the top of `build_inner` (early return); never reaches
        // dispatch. Arm exists only for match exhaustiveness.
        #[cfg(feature = "robot")]
        Element::Component { .. } => unreachable!(
            "Element::Component is unwrapped before dispatch in build_inner"
        ),
    };
    let result = dispatcher(backend, node);

    #[cfg(feature = "debug-stats")]
    crate::debug::record_build_exit(_debug_kind);

    // Robot: wire frame-reading closures now that the backend node
    // exists. Each closure captures the node + backend Rc; they're
    // called on demand by `Robot::frame` / `Robot::absolute_frame`
    // via the bridge or in-app paths.
    #[cfg(feature = "robot")]
    if let Some(id) = robot_id {
        let node_for_frame = result.clone();
        let node_for_abs = result.clone();
        let node_for_dev = result.clone();
        let backend_for_frame = backend.clone();
        let backend_for_abs = backend.clone();
        let backend_for_dev = backend.clone();
        crate::robot::attach_frame_actions(
            id,
            Rc::new(move || backend_for_frame.borrow().frame(&node_for_frame)),
            Rc::new(move || backend_for_abs.borrow().absolute_frame(&node_for_abs)),
            Rc::new(move || backend_for_dev.borrow().device_frame(&node_for_dev)),
        );
        // Native introspection: read the platform's resolved render state on
        // demand (parity testing). Same capture pattern as the frame closures.
        let node_for_introspect = result.clone();
        let backend_for_introspect = backend.clone();
        crate::robot::attach_introspect_action(
            id,
            Rc::new(move || backend_for_introspect.borrow().introspect_native(&node_for_introspect)),
        );
        // Record this node as a primitive-root boundary so a backend's native
        // introspection walk knows where this element's subtree ends and a
        // child element's begins (no-op on backends that don't need it).
        backend.borrow().note_introspection_root(&result);
        // Editable text inputs get `focus`/`blur` actions so the robot can drive
        // real keyboard focus (a user-click analogue) — used by tests and the
        // inspector. The handle is made on demand from the backend node.
        if robot_is_text_input {
            let node_focus = result.clone();
            let node_blur = result.clone();
            let backend_focus = backend.clone();
            let backend_blur = backend.clone();
            crate::robot::attach_focus_actions(
                id,
                Rc::new(move || backend_focus.borrow().make_text_input_handle(&node_focus).focus()),
                Rc::new(move || backend_blur.borrow().make_text_input_handle(&node_blur).blur()),
            );
        }
        // Scroll views get a `set_scroll` action — the programmatic pan
        // analogue. Route through the backend's `ScrollViewHandle` (made
        // here, borrow released) rather than `Backend::set_node_scroll`
        // under a live `borrow_mut`: the native scroll write fires scroll
        // notifications SYNCHRONOUSLY (AppKit `reflectScrolledClipView:`),
        // whose reactive effects re-borrow the backend to re-style — a
        // held borrow aborts with "RefCell already borrowed" (seen live on
        // the macOS website: sticky/TOC-spy restyles under the robot
        // drive). The handle's ops take no backend borrow, same as the
        // author-facing `scroll_to` path.
        if robot_is_scroll_view {
            let handle = backend.borrow().make_scroll_view_handle(&result);
            crate::robot::attach_scroll_action(
                id,
                Rc::new(move |x, y| handle.scroll_to(x, y)),
            );
        }
    }

    // Robot: pop parent stack now that children are built.
    #[cfg(feature = "robot")]
    if robot_id.is_some() {
        crate::robot::pop_parent();
    }

    result
}

// =============================================================================
// Per-variant dispatch shims.
//
// Each `dispatch_*` takes the full `Element` by value, destructures
// the one variant it owns, and forwards to that variant's submodule
// `build(...)` helper. `#[inline(never)]` is the load-bearing
// annotation — without it, rustc would re-inline these back into
// `build_inner` and we'd re-bloat the frame.
//
// The `let-else { unreachable!() }` pattern keeps the variant-known
// destructure cheap: in release builds LLVM proves the else branch
// is dead given the caller's match-on-discriminant, so there's no
// runtime check or panic infrastructure. (We use safe `unreachable!`
// rather than `unreachable_unchecked!` because the cost is zero
// after optimization and the safety story stays simple.)
//
// Each function's job is exactly one variant's destructure +
// argument-marshalling. They're individually small (~few hundred
// bytes of stack each) and called from at most one site, so the
// code-size cost of `#[inline(never)]` is negligible.
// =============================================================================

#[inline(never)]
fn dispatch_text<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Text { source, style, ref_fill, accessibility, .. } = node else { unreachable!() };
    text::build(backend, source, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_view<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::View {
        children, style, ref_fill, safe_area_sides, on_touch, on_wheel, on_hover, on_file_drop, preserves_focus, is_container, accessibility, ..
    } = node
    else { unreachable!() };
    view::build(
        backend, children, style, ref_fill, safe_area_sides, on_touch, on_wheel, on_hover, on_file_drop,
        preserves_focus, is_container, accessibility,
    )
}

#[cfg(feature = "prim-navigator")]
thread_local! {
    /// Stack of active outlet-capture cells. `build_layout_with_outlet`
    /// (walker::navigator) pushes a `Rc<RefCell<Option<B::Node>>>`
    /// (type-erased as `Box<dyn Any>`) before building a navigator's
    /// author layout and pops it after; `dispatch_navigator_outlet`
    /// writes the built outlet node into the TOP cell. A stack (not a
    /// single slot) so a nested navigator building its own layout inside
    /// a parent's layout captures into its own cell, never the parent's.
    static OUTLET_CAPTURE: RefCell<Vec<Box<dyn std::any::Any>>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that pushes an outlet-capture cell for the duration of an
/// author-layout build. On drop it pops the cell and yields the captured
/// node (if the layout contained a `NavigatorOutlet`). Used by
/// `walker::navigator`'s `build_layout_with_outlet`.
#[cfg(feature = "prim-navigator")]
pub(crate) struct OutletCaptureGuard<N: Clone + 'static> {
    cell: Rc<RefCell<Option<N>>>,
}

#[cfg(feature = "prim-navigator")]
impl<N: Clone + 'static> OutletCaptureGuard<N> {
    pub(crate) fn push() -> Self {
        let cell: Rc<RefCell<Option<N>>> = Rc::new(RefCell::new(None));
        OUTLET_CAPTURE.with(|s| s.borrow_mut().push(Box::new(cell.clone())));
        OutletCaptureGuard { cell }
    }

    /// The captured outlet node, or `None` if the built layout contained
    /// no `NavigatorOutlet` (author forgot to splat `{nav.outlet}`).
    pub(crate) fn take(self) -> Option<N> {
        // `self` drops after this returns, popping the stack.
        self.cell.borrow_mut().take()
    }
}

#[cfg(feature = "prim-navigator")]
impl<N: Clone + 'static> Drop for OutletCaptureGuard<N> {
    fn drop(&mut self) {
        OUTLET_CAPTURE.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

#[inline(never)]
#[cfg(feature = "prim-navigator")]
fn dispatch_navigator_outlet<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: Element,
) -> B::Node {
    let Element::NavigatorOutlet { style, ref_fill, accessibility } = node else { unreachable!() };
    // The outlet is an empty container view; the SDK handler swaps the
    // active screen in as its only child. `is_container = true` so it
    // participates in layout like a plain View.
    //
    // Style-less outlets get the fill default (`outlet_fill_rules`): screens
    // assume a bounded, fillable region, and a bare hug-content outlet broke
    // the zero-config path on every backend. An author style on the outlet
    // (`ctx.outlet.with_style(...)`) replaces the default entirely.
    let style = style
        .or_else(|| Some(crate::primitives::navigator::shared::default_outlet_style()));
    let n = view::build(
        backend,
        Vec::new(),
        style,
        ref_fill,
        crate::SafeAreaSides::NONE,
        None,
        None,
        None,
        None,
        false,
        true,
        accessibility,
    );
    // Record into the innermost active capture cell so the enclosing
    // navigator's handler can address this node for screen swaps.
    OUTLET_CAPTURE.with(|s| {
        if let Some(top) = s.borrow().last() {
            if let Some(cell) = top.downcast_ref::<Rc<RefCell<Option<B::Node>>>>() {
                *cell.borrow_mut() = Some(n.clone());
            }
        }
    });
    n
}

#[inline(never)]
fn dispatch_pressable<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Pressable { children, on_click, style, ref_fill, disabled, preserves_focus, accessibility, .. } = node
    else { unreachable!() };
    pressable::build(backend, children, on_click, style, ref_fill, disabled, preserves_focus, accessibility)
}

#[inline(never)]
fn dispatch_button<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Button { label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility, .. } = node
    else { unreachable!() };
    button::build(backend, label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-image")]
fn dispatch_image<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Image { src, alt, alt_fn, on_load, on_error, style, ref_fill, asset, accessibility, .. } = node
    else { unreachable!() };
    image::build(backend, src, alt, alt_fn, on_load, on_error, style, ref_fill, asset, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-icon")]
fn dispatch_icon<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Icon { data, data_fn, color, stroke, draw_in, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    icon::build(backend, data, data_fn, color, stroke, draw_in, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-text-input")]
fn dispatch_text_input<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::TextInput { value, on_change, on_key_down, on_blur, on_focus, placeholder, secure, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    text_input::build_text_input(backend, value, on_change, on_key_down, on_blur, on_focus, placeholder, secure, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-text-input")]
fn dispatch_text_area<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::TextArea { value, on_change, on_key_down, placeholder, wrap, min_rows, max_rows, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    text_input::build_text_area(backend, value, on_change, on_key_down, placeholder, wrap, min_rows, max_rows, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-toggle")]
fn dispatch_toggle<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Toggle { value, on_change, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    toggle::build(backend, value, on_change, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_scroll_view<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::ScrollView { children, horizontal, style, ref_fill, safe_area_sides, on_scroll, accessibility, .. } = node
    else { unreachable!() };
    scroll_view::build(backend, children, horizontal, style, ref_fill, safe_area_sides, on_scroll, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-slider")]
fn dispatch_slider<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Slider { value, on_change, min, max, step, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    slider::build(backend, value, on_change, min, max, step, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-activity")]
fn dispatch_activity_indicator<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::ActivityIndicator { size, size_fn, color, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    activity_indicator::build(backend, size, size_fn, color, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-virtualizer")]
fn dispatch_virtualizer<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Virtualizer {
        item_count, item_key, item_size, render_item, row_template,
        row_index_signal_id, overscan, layout, style, ref_fill, accessibility, ..
    } = node else { unreachable!() };
    virtualizer::build(
        backend, item_count, item_key, item_size, render_item, row_template,
        row_index_signal_id, overscan, layout, style, ref_fill, accessibility,
    )
}

/// Fallback for a primitive whose dispatch was compiled out by a disabled
/// `prim-*` feature. Reaching it means the element arrived at runtime
/// despite the authoring fn being gated — a wire-received subtree from a
/// runtime-server, or a hand-built `Element`. Renders the backend's native
/// "unsupported external" placeholder (the same one an unregistered
/// `Element::External` gets), with the feature name in the label so the
/// remedy is visible on screen. Compile-time uses never get here: the
/// `flat_list` / `virtualizer` builder fns are gated out with the feature.
#[cfg(not(feature = "prim-virtualizer"))]
fn dispatch_virtualizer<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Virtualizer { accessibility, .. } = node else { unreachable!() };
    struct GatedOffVirtualizer;
    let payload: Rc<dyn std::any::Any> = Rc::new(());
    backend.borrow_mut().create_external(
        std::any::TypeId::of::<GatedOffVirtualizer>(),
        "virtualizer (compiled out: enable runtime-core feature `prim-virtualizer`)",
        &payload,
        &accessibility,
    )
}

#[inline(never)]
#[cfg(feature = "prim-graphics")]
fn dispatch_graphics<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Graphics { on_ready, on_resize, on_lost, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    graphics::build(backend, on_ready, on_resize, on_lost, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_when<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::When { cond, then, otherwise, style } = node else { unreachable!() };
    when_switch::build_when(backend, cond, then, otherwise, style)
}

#[inline(never)]
fn dispatch_switch<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Switch { discriminant, arms, default, style } = node else { unreachable!() };
    when_switch::build_switch(backend, discriminant, arms, default, style)
}

#[inline(never)]
fn dispatch_each<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Each { snapshot, style } = node else { unreachable!() };
    each::build(backend, snapshot, style)
}

#[inline(never)]
fn dispatch_dynamic<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Dynamic { build } = node else { unreachable!() };
    dynamic::build_dynamic(backend, build)
}

#[inline(never)]
fn dispatch_fragment<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Fragment { children } = node else { unreachable!() };
    // Standalone fragment — returned from a `when`/`switch`/`presence`
    // branch or used as a mount root, where there's no parent children
    // list to splice into. Host the children under a layout-transparent
    // reactive anchor (`display:contents` on web) so they still render as
    // a flat group without a layout box. In a children list the fragment
    // never reaches here: `insert_children` splices it inline with no node.
    let mut anchor = backend.borrow_mut().create_reactive_anchor();
    view::insert_children(backend, &mut anchor, children);
    anchor
}

#[inline(never)]
fn dispatch_link<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Link { children, route, url, url_fn, make_params, kind, target, external, style, ref_fill, accessibility } = node
    else { unreachable!() };
    link::build(backend, children, route, url, url_fn, make_params, kind, target, external, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_external<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::External { type_id, type_name, payload, children, style, ref_fill, on_touch, on_hover, accessibility } = node
    else { unreachable!() };
    external::build(backend, type_id, type_name, payload, children, style, ref_fill, on_touch, on_hover, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-navigator")]
fn dispatch_navigator<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Navigator { type_id, type_name, presentation, config, style, slot_styles, ref_fill, accessibility } = node
    else { unreachable!() };
    // Publish this navigator's screen paths to the SSG route-collector
    // (if enabled). Live backends never enable it; the call is a
    // thread-local check + branch when off. See
    // `primitives::navigator::shared::record_routes` for the rationale.
    crate::primitives::navigator::record_routes(&config);
    navigator::build(backend, type_id, type_name, presentation, config, style, slot_styles, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-portal")]
fn dispatch_portal<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Portal { children, target, on_dismiss, trap_focus, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    portal::build(backend, children, target, on_dismiss, trap_focus, style, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-presence")]
fn dispatch_presence<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Presence { child, present, enter, exit, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    presence::build(backend, child, present, enter, exit, ref_fill, accessibility)
}

#[inline(never)]
#[cfg(feature = "prim-lazy")]
fn dispatch_lazy<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Lazy { loader, on_state, placeholder, error, style, ref_fill, accessibility } = node
    else { unreachable!() };
    lazy::build(backend, loader, on_state, placeholder, error, style, ref_fill, accessibility)
}

#[cfg(all(test, feature = "prim-navigator"))]
mod outlet_capture_tests {
    //! Unit coverage for the outlet-capture mechanism the new author-layout
    //! navigation model (`SwapContext`/`build_layout_with_outlet`) relies on:
    //! building an `Element::NavigatorOutlet` under an active
    //! [`OutletCaptureGuard`] records its node into the guard's cell, and the
    //! capture is scoped to the *innermost* guard so a nested navigator never
    //! writes into a parent's cell. The end-to-end Select-swap / Link→Select
    //! behavior is covered by the swap-navigator wire test.
    use super::*;
    use crate::primitives::navigator::navigator_outlet;

    /// Minimal `Backend` (Node = u32) that stays on the web-like
    /// (`handles_states_natively = true`) path so an empty container outlet
    /// builds without the native inline-size layout machinery.
    struct CaptureStub {
        next: RefCell<u32>,
    }
    impl CaptureStub {
        fn new() -> Rc<RefCell<Self>> {
            Rc::new(RefCell::new(Self { next: RefCell::new(0) }))
        }
        fn mint(&self) -> u32 {
            let id = *self.next.borrow();
            *self.next.borrow_mut() = id + 1;
            id
        }
    }
    impl Backend for CaptureStub {
        type Node = u32;
        fn handles_states_natively(&self) -> bool {
            true
        }
        fn create_view(&mut self, _a11y: &crate::accessibility::AccessibilityProps) -> u32 {
            self.mint()
        }
        fn create_text(
            &mut self,
            _content: &str,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> u32 {
            self.mint()
        }
        fn create_button(
            &mut self,
            _label: &str,
            _on_click: &crate::Action,
            _leading: Option<&crate::primitives::icon::IconData>,
            _trailing: Option<&crate::primitives::icon::IconData>,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> u32 {
            self.mint()
        }
        fn insert(&mut self, _parent: &mut u32, _child: u32) {}
        fn update_text(&mut self, _node: &u32, _content: &str) {}
        fn clear_children(&mut self, _node: &u32) {}
        fn apply_style(&mut self, _node: &u32, _style: &Rc<crate::style::StyleRules>) {}
        fn execute_batch(&mut self, batch: crate::BackendBatch) -> Vec<u32> {
            (0..batch.node_count).map(|_| self.mint()).collect()
        }
        fn insert_many(&mut self, _parent: &mut u32, _children: Vec<u32>) {}
        fn finish(&mut self, _root: u32) {}
    }

    #[test]
    fn navigator_outlet_is_captured_during_layout_build() {
        let backend = CaptureStub::new();
        let guard = OutletCaptureGuard::<u32>::push();
        let (root, _scope) = build_detached(&backend, navigator_outlet(), None);
        let outlet = guard.take();
        // A lone outlet is its own root, so the captured node IS the root.
        assert_eq!(outlet, Some(root), "the built outlet node must be captured");
    }

    #[test]
    fn capture_is_scoped_to_innermost_guard() {
        let backend = CaptureStub::new();
        let outer = OutletCaptureGuard::<u32>::push();
        {
            let inner = OutletCaptureGuard::<u32>::push();
            let _ = build_detached(&backend, navigator_outlet(), None);
            assert!(inner.take().is_some(), "inner guard captures the outlet");
        }
        // The outlet was written to the inner cell only; the outer guard —
        // which never enclosed an outlet build of its own — stays empty. This
        // is what keeps a nested navigator from stealing its parent's outlet.
        assert!(outer.take().is_none(), "outer guard must not see the inner outlet");
    }

    #[test]
    fn no_capture_without_a_guard_is_safe() {
        // Building an outlet with no active guard must not panic (the arm's
        // `if let Some(top)` is a no-op when the capture stack is empty).
        let backend = CaptureStub::new();
        let (_root, _scope) = build_detached(&backend, navigator_outlet(), None);
    }
}

/// Shared fallback for primitives whose dispatch was compiled out by a
/// disabled `prim-*` feature: render the backend's native "unsupported
/// external" placeholder with the feature name in the label. See
/// `dispatch_virtualizer`'s gated-off doc comment for the full rationale;
/// every gated family funnels through this one code path (regression-tested
/// once, via `tests/prim_gating.rs`).
#[allow(dead_code)]
fn gated_off_placeholder<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    label: &'static str,
) -> B::Node {
    struct GatedOffPrimitive;
    let payload: Rc<dyn std::any::Any> = Rc::new(());
    backend.borrow_mut().create_external(
        std::any::TypeId::of::<GatedOffPrimitive>(),
        label,
        &payload,
        &crate::accessibility::AccessibilityProps::default(),
    )
}

#[cfg(not(feature = "prim-icon"))]
fn dispatch_icon<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "icon (compiled out: enable runtime-core feature `prim-icon`)",
    )
}

#[cfg(not(feature = "prim-image"))]
fn dispatch_image<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "image (compiled out: enable runtime-core feature `prim-image`)",
    )
}

#[cfg(not(feature = "prim-text-input"))]
fn dispatch_text_input<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "text_input (compiled out: enable runtime-core feature `prim-text-input`)",
    )
}

#[cfg(not(feature = "prim-text-input"))]
fn dispatch_text_area<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "text_area (compiled out: enable runtime-core feature `prim-text-input`)",
    )
}

#[cfg(not(feature = "prim-toggle"))]
fn dispatch_toggle<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "toggle (compiled out: enable runtime-core feature `prim-toggle`)",
    )
}

#[cfg(not(feature = "prim-slider"))]
fn dispatch_slider<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "slider (compiled out: enable runtime-core feature `prim-slider`)",
    )
}

#[cfg(not(feature = "prim-activity"))]
fn dispatch_activity_indicator<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "activity_indicator (compiled out: enable runtime-core feature `prim-activity`)",
    )
}

#[cfg(not(feature = "prim-portal"))]
fn dispatch_portal<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "portal (compiled out: enable runtime-core feature `prim-portal`)",
    )
}

#[cfg(not(feature = "prim-presence"))]
fn dispatch_presence<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "presence (compiled out: enable runtime-core feature `prim-presence`)",
    )
}

#[cfg(not(feature = "prim-graphics"))]
fn dispatch_graphics<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "graphics (compiled out: enable runtime-core feature `prim-graphics`)",
    )
}

#[cfg(not(feature = "prim-navigator"))]
fn dispatch_navigator<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "navigator (compiled out: enable runtime-core feature `prim-navigator`)",
    )
}

#[cfg(not(feature = "prim-navigator"))]
fn dispatch_navigator_outlet<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: Element,
) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "navigator outlet (compiled out: enable runtime-core feature `prim-navigator`)",
    )
}

#[cfg(not(feature = "prim-lazy"))]
fn dispatch_lazy<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    drop(node);
    gated_off_placeholder::<B>(
        backend,
        "lazy (compiled out: enable runtime-core feature `prim-lazy`)",
    )
}
