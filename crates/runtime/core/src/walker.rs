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
    backend.borrow_mut().finish(root);
    Owner { scope }
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
                actions: meta.actions,
                parent,
                children: Vec::new(),
            });
            // Link child → parent.
            if let Some(pid) = parent {
                robot::add_child(pid, id);
            }
            robot::push_parent(id);
            Some(id)
        } else {
            None
        }
    };

    let result = match node {
        Element::Text { source, style, ref_fill, accessibility, .. } => {
            text::build(backend, source, style, ref_fill, accessibility)
        }
        Element::View { children, style, ref_fill, safe_area_sides, on_touch, accessibility, .. } => {
            view::build(backend, children, style, ref_fill, safe_area_sides, on_touch, accessibility)
        }
        Element::Pressable { children, on_click, style, ref_fill, disabled, accessibility, .. } => {
            pressable::build(backend, children, on_click, style, ref_fill, disabled, accessibility)
        }
        Element::Button { label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility, .. } => {
            button::build(
                backend, label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, accessibility,
            )
        }
        Element::Image { src, alt, style, ref_fill, asset, accessibility, .. } => {
            image::build(backend, src, alt, style, ref_fill, asset, accessibility)
        }
        Element::Icon { data, color, stroke, draw_in, style, ref_fill, accessibility, .. } => {
            icon::build(backend, data, color, stroke, draw_in, style, ref_fill, accessibility)
        }
        Element::TextInput { value, on_change, on_key_down, placeholder, style, ref_fill, accessibility, .. } => {
            text_input::build_text_input(
                backend, value, on_change, on_key_down, placeholder, style, ref_fill, accessibility,
            )
        }
        Element::TextArea { value, on_change, on_key_down, placeholder, style, ref_fill, accessibility, .. } => {
            text_input::build_text_area(
                backend, value, on_change, on_key_down, placeholder, style, ref_fill, accessibility,
            )
        }
        Element::Toggle { value, on_change, style, ref_fill, accessibility, .. } => {
            toggle::build(backend, value, on_change, style, ref_fill, accessibility)
        }
        Element::ScrollView { children, horizontal, style, ref_fill, safe_area_sides, on_scroll, accessibility, .. } => {
            scroll_view::build(backend, children, horizontal, style, ref_fill, safe_area_sides, on_scroll, accessibility)
        }
        Element::Slider { value, on_change, min, max, step, style, ref_fill, accessibility, .. } => {
            slider::build(backend, value, on_change, min, max, step, style, ref_fill, accessibility)
        }
        Element::ActivityIndicator { size, color, style, ref_fill, accessibility, .. } => {
            activity_indicator::build(backend, size, color, style, ref_fill, accessibility)
        }
        Element::Virtualizer {
            item_count,
            item_key,
            item_size,
            render_item,
            row_template,
            row_index_signal_id,
            overscan,
            horizontal,
            style,
            ref_fill,
            accessibility,
            ..
        } => virtualizer::build(
            backend,
            item_count,
            item_key,
            item_size,
            render_item,
            row_template,
            row_index_signal_id,
            overscan,
            horizontal,
            style,
            ref_fill,
            accessibility,
        ),
        Element::Graphics { on_ready, on_resize, on_lost, style, ref_fill, accessibility, .. } => {
            graphics::build(backend, on_ready, on_resize, on_lost, style, ref_fill, accessibility)
        }
        Element::When { cond, then, otherwise, style } => {
            when_switch::build_when(backend, cond, then, otherwise, style)
        }
        Element::Switch { discriminant, arms, default, style } => {
            when_switch::build_switch(backend, discriminant, arms, default, style)
        }
        Element::Each { snapshot, style } => {
            each::build(backend, snapshot, style)
        }
        Element::Link {
            children,
            route,
            url,
            make_params,
            kind,
            target,
            external,
            style,
            ref_fill,
            accessibility,
        } => link::build(backend, children, route, url, make_params, kind, target, external, style, ref_fill, accessibility),
        Element::External {
            type_id,
            type_name,
            payload,
            style,
            ref_fill,
            accessibility,
            ..
        } => external::build(backend, type_id, type_name, payload, style, ref_fill, accessibility),
        Element::Navigator {
            type_id,
            type_name,
            presentation,
            config,
            style,
            slot_styles,
            ref_fill,
            accessibility,
        } => navigator::build(
            backend,
            type_id,
            type_name,
            presentation,
            config,
            style,
            slot_styles,
            ref_fill,
            accessibility,
        ),
        Element::Portal {
            children,
            target,
            on_dismiss,
            trap_focus,
            style,
            ref_fill,
            accessibility,
            ..
        } => portal::build(backend, children, target, on_dismiss, trap_focus, style, ref_fill, accessibility),
        Element::Presence { child, present, enter, exit, ref_fill, accessibility, .. } => {
            presence::build(backend, child, present, enter, exit, ref_fill, accessibility)
        }
        Element::Lazy {
            loader,
            on_state,
            placeholder,
            style,
            ref_fill,
            accessibility,
        } => lazy::build(backend, loader, on_state, placeholder, style, ref_fill, accessibility),
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
    };

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
        let backend_for_frame = backend.clone();
        let backend_for_abs = backend.clone();
        crate::robot::attach_frame_actions(
            id,
            Rc::new(move || backend_for_frame.borrow().frame(&node_for_frame)),
            Rc::new(move || backend_for_abs.borrow().absolute_frame(&node_for_abs)),
        );
    }

    // Robot: pop parent stack now that children are built.
    #[cfg(feature = "robot")]
    if robot_id.is_some() {
        crate::robot::pop_parent();
    }

    result
}
