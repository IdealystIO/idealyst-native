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

mod activity_indicator;
mod button;
mod cleanup;
mod debug;
mod each;
mod external;
mod graphics;
mod icon;
mod image;
mod lazy;
mod link;
mod navigator;
mod portal;
mod presence;
mod pressable;
#[cfg(feature = "robot")]
mod robot;
mod scroll_view;
mod slider;
mod style;
mod text;
mod text_input;
mod theme_cohort;
mod toggle;
mod view;
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
///     let phase = signal!(0u8);
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
        crate::robot::bridge::start_auto_polling(
            crate::robot::bridge::DEFAULT_PORT,
        );
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
        // Both the tree constructor and the build walk run inside the
        // same root scope. Reactive primitives created during
        // construction adopt this scope and are freed on `Owner`
        // drop alongside the per-build effects that wire them up.
        let tree = tree_fn();
        build(&backend, 0, tree)
    });
    // SSR hydration: drain the navigator SDK's deferred chrome/screen
    // microtasks NOW — adoption window still open (`finish` not yet run),
    // no backend borrow held — so they adopt the server's DOM instead of
    // firing post-`finish` and rebuilding fresh. Mirrors SSR's own
    // post-`mount` drain. No-op off hydration.
    if backend.borrow().is_hydrating() {
        crate::scheduling::drain_buffered_microtasks();
    }
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
    // Robot: a `#[component]` with `methods!` wraps its root primitive in
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
        Element::Link { .. } => dispatch_link::<B>,
        Element::External { .. } => dispatch_external::<B>,
        Element::Navigator { .. } => dispatch_navigator::<B>,
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
        children, style, ref_fill, safe_area_sides, on_touch, is_container, accessibility, ..
    } = node
    else { unreachable!() };
    view::build(
        backend, children, style, ref_fill, safe_area_sides, on_touch, is_container, accessibility,
    )
}

#[inline(never)]
fn dispatch_pressable<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Pressable { children, on_click, style, ref_fill, disabled, accessibility, .. } = node
    else { unreachable!() };
    pressable::build(backend, children, on_click, style, ref_fill, disabled, accessibility)
}

#[inline(never)]
fn dispatch_button<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Button { label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility, .. } = node
    else { unreachable!() };
    button::build(backend, label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility)
}

#[inline(never)]
fn dispatch_image<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Image { src, alt, style, ref_fill, asset, accessibility, .. } = node
    else { unreachable!() };
    image::build(backend, src, alt, style, ref_fill, asset, accessibility)
}

#[inline(never)]
fn dispatch_icon<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Icon { data, color, stroke, draw_in, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    icon::build(backend, data, color, stroke, draw_in, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_text_input<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::TextInput { value, on_change, on_key_down, placeholder, secure, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    text_input::build_text_input(backend, value, on_change, on_key_down, placeholder, secure, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_text_area<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::TextArea { value, on_change, on_key_down, placeholder, wrap, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    text_input::build_text_area(backend, value, on_change, on_key_down, placeholder, wrap, style, ref_fill, accessibility)
}

#[inline(never)]
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
fn dispatch_slider<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Slider { value, on_change, min, max, step, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    slider::build(backend, value, on_change, min, max, step, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_activity_indicator<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::ActivityIndicator { size, color, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    activity_indicator::build(backend, size, color, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_virtualizer<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Virtualizer {
        item_count, item_key, item_size, render_item, row_template,
        row_index_signal_id, overscan, horizontal, style, ref_fill, accessibility, ..
    } = node else { unreachable!() };
    virtualizer::build(
        backend, item_count, item_key, item_size, render_item, row_template,
        row_index_signal_id, overscan, horizontal, style, ref_fill, accessibility,
    )
}

#[inline(never)]
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
fn dispatch_link<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Link { children, route, url, make_params, kind, target, external, style, ref_fill, accessibility } = node
    else { unreachable!() };
    link::build(backend, children, route, url, make_params, kind, target, external, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_external<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::External { type_id, type_name, payload, children, style, ref_fill, accessibility } = node
    else { unreachable!() };
    external::build(backend, type_id, type_name, payload, children, style, ref_fill, accessibility)
}

#[inline(never)]
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
fn dispatch_portal<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Portal { children, target, on_dismiss, trap_focus, style, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    portal::build(backend, children, target, on_dismiss, trap_focus, style, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_presence<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Presence { child, present, enter, exit, ref_fill, accessibility, .. } = node
    else { unreachable!() };
    presence::build(backend, child, present, enter, exit, ref_fill, accessibility)
}

#[inline(never)]
fn dispatch_lazy<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Element) -> B::Node {
    let Element::Lazy { loader, on_state, placeholder, style, ref_fill, accessibility } = node
    else { unreachable!() };
    lazy::build(backend, loader, on_state, placeholder, style, ref_fill, accessibility)
}
