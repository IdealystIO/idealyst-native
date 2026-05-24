//! `Primitive::Navigator`, `TabNavigator`, and `DrawerNavigator`
//! build paths.
//!
//! Each `build_*` entry point destructures the primitive's options
//! struct, builds the native container via the corresponding
//! `Backend::create_*_navigator` hook, wires the per-screen scope
//! registry + ambient-nav guards, mounts the initial screen, runs
//! the optional layout, attaches header/title/button/etc. slot
//! styles, and installs a `*HandleCleanup` so the backend's
//! `release_*` hook fires when the surrounding scope drops.
//!
//! Helpers shared by all three:
//! - [`invoke_layout_and_attach`] — runs the optional `build_layout`
//!   closure and hands the resulting subtree to the backend via
//!   `attach_navigator_layout`. Runs OUTSIDE the borrow window the
//!   `create_*_navigator` call took; the closure re-enters the
//!   walker and a nested re-borrow would panic.
//! - [`attach_navigator_color_callback`] — bridges a
//!   `.header(idea_header(...))`-style color callback into a per-slot
//!   `apply_navigator_*_style` reactive Effect so the toolbar follows
//!   theme swaps.
//! - [`attach_navigator_slot_style`] — generic slot-style attach for
//!   navigator chrome (header, title, button, tab bar, sidebar,
//!   scrim, etc.). Same cohort-based theme reactivity as
//!   `attach_style_static`, just routed to a custom apply function.

use super::cleanup::{
    DrawerNavigatorHandleCleanup, NavigatorHandleCleanup, TabNavigatorHandleCleanup,
};
use super::debug::time_backend_create;
use super::style::attach_style;
use super::theme_cohort::{
    install_theme_cohort_driver, theme_cohort_register, theme_cohort_unregister,
};
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitive::Primitive;
use crate::primitives;
use crate::reactive::{self, Effect, Ref, Signal};
use crate::sources::StyleSource;
use crate::style::{resolve as resolve_style, StyleRules};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// ----- Dispatcher entry points (called from `build_inner`) ------------------

pub(super) fn build_navigator_dispatch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    nav: Box<primitives::navigator::Navigator>,
) -> B::Node {
    let primitives::navigator::Navigator {
        initial,
        initial_path,
        screens,
        layout,
        default_options,
        style,
        header_style,
        title_style,
        button_style,
        ref_fill,
    } = *nav;
    let header_bg_cb = default_options
        .as_ref()
        .and_then(|o| o.header_background.clone());
    let title_color_cb = default_options
        .as_ref()
        .and_then(|o| o.title_color.clone());
    let header_tint_cb = default_options
        .as_ref()
        .and_then(|o| o.header_tint.clone());
    let n = build_navigator(
        backend,
        initial,
        initial_path,
        screens,
        layout,
        default_options,
        ref_fill,
    );
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(s) = header_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_header_style(node, rules);
        });
    }
    if let Some(s) = title_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_title_style(node, rules);
        });
    }
    if let Some(s) = button_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_button_style(node, rules);
        });
    }
    if let Some(cb) = header_bg_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.background = Some(c.into()),
            |b, node, rules| { b.apply_navigator_header_style(node, rules); },
        );
    }
    if let Some(cb) = title_color_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_title_style(node, rules); },
        );
    }
    if let Some(cb) = header_tint_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_button_style(node, rules); },
        );
    }
    // Cleanup: when the surrounding scope drops, this empty
    // Effect drops, dropping the `NavigatorHandleCleanup`,
    // which tells the backend to tear down its native stack.
    // Same pattern as Virtualizer / Graphics.
    {
        let cleanup = NavigatorHandleCleanup {
            backend: backend.clone(),
            node: n.clone(),
        };
        let _e = Effect::new(move || {
            let _ = &cleanup.node;
        });
    }
    n
}

pub(super) fn build_tab_navigator_dispatch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    nav: Box<primitives::navigator::TabNavigator>,
) -> B::Node {
    let primitives::navigator::TabNavigator {
        initial,
        initial_path,
        tab_order,
        screens,
        layout,
        placement,
        mount_policy,
        default_options,
        style,
        header_style,
        title_style,
        button_style,
        tab_bar_style,
        tab_icon_style,
        tab_label_style,
        ref_fill,
    } = *nav;
    let header_bg_cb = default_options
        .as_ref()
        .and_then(|o| o.header_background.clone());
    let title_color_cb = default_options
        .as_ref()
        .and_then(|o| o.title_color.clone());
    let header_tint_cb = default_options
        .as_ref()
        .and_then(|o| o.header_tint.clone());
    let n = build_tab_navigator(
        backend,
        initial,
        initial_path,
        tab_order,
        screens,
        layout,
        placement,
        mount_policy,
        default_options,
        ref_fill,
    );
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(s) = header_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_header_style(node, rules);
        });
    }
    if let Some(s) = title_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_title_style(node, rules);
        });
    }
    if let Some(s) = button_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_button_style(node, rules);
        });
    }
    if let Some(s) = tab_bar_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_tab_bar_style(node, rules);
        });
    }
    if let Some(s) = tab_icon_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_tab_icon_style(node, rules);
        });
    }
    if let Some(s) = tab_label_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_tab_label_style(node, rules);
        });
    }
    if let Some(cb) = header_bg_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.background = Some(c.into()),
            |b, node, rules| { b.apply_navigator_header_style(node, rules); },
        );
    }
    if let Some(cb) = title_color_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_title_style(node, rules); },
        );
    }
    if let Some(cb) = header_tint_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_button_style(node, rules); },
        );
    }
    {
        let cleanup = TabNavigatorHandleCleanup {
            backend: backend.clone(),
            node: n.clone(),
        };
        let _e = Effect::new(move || {
            let _ = &cleanup.node;
        });
    }
    n
}

pub(super) fn build_drawer_navigator_dispatch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    nav: Box<primitives::navigator::DrawerNavigator>,
) -> B::Node {
    let primitives::navigator::DrawerNavigator {
        initial,
        initial_path,
        screens,
        layout,
        content,
        side,
        drawer_type,
        drawer_width,
        swipe_to_open,
        mount_policy,
        default_options,
        style,
        header_style,
        title_style,
        button_style,
        sidebar_style,
        scrim_style,
        ref_fill,
        background_color,
    } = *nav;
    // Pluck out the ergonomic `.header(idea_header(...))` color
    // callbacks before `default_options` moves into the build
    // call. These are `Fn() -> Color` closures that read
    // `active_theme()` internally — they need a reactive Effect
    // to re-fire on theme swap. See
    // `attach_navigator_color_callback` for the bridge.
    let header_bg_cb = default_options
        .as_ref()
        .and_then(|o| o.header_background.clone());
    let title_color_cb = default_options
        .as_ref()
        .and_then(|o| o.title_color.clone());
    let header_tint_cb = default_options
        .as_ref()
        .and_then(|o| o.header_tint.clone());
    let body_bg_cb = background_color.clone();
    let n = build_drawer_navigator(
        backend,
        initial,
        initial_path,
        screens,
        layout,
        content,
        side,
        drawer_type,
        drawer_width,
        swipe_to_open,
        mount_policy,
        default_options,
        ref_fill,
        background_color,
    );
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    if let Some(s) = header_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_header_style(node, rules);
        });
    }
    if let Some(s) = title_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_title_style(node, rules);
        });
    }
    if let Some(s) = button_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_navigator_button_style(node, rules);
        });
    }
    if let Some(cb) = header_bg_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.background = Some(c.into()),
            |b, node, rules| { b.apply_navigator_header_style(node, rules); },
        );
    }
    if let Some(cb) = title_color_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_title_style(node, rules); },
        );
    }
    if let Some(cb) = header_tint_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.color = Some(c.into()),
            |b, node, rules| { b.apply_navigator_button_style(node, rules); },
        );
    }
    if let Some(cb) = body_bg_cb {
        attach_navigator_color_callback(
            backend, &n, cb,
            |r, c| r.background = Some(c.into()),
            |b, node, rules| { b.apply_navigator_body_style(node, rules); },
        );
    }
    if let Some(s) = sidebar_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_drawer_sidebar_style(node, rules);
        });
    }
    if let Some(s) = scrim_style {
        attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
            b.borrow_mut().apply_drawer_scrim_style(node, rules);
        });
    }
    {
        let cleanup = DrawerNavigatorHandleCleanup {
            backend: backend.clone(),
            node: n.clone(),
        };
        let _e = Effect::new(move || {
            let _ = &cleanup.node;
        });
    }
    n
}

// ----- Helpers --------------------------------------------------------------

/// Run a navigator's optional `build_layout` closure and hand the
/// resulting subtree to the backend via `attach_navigator_layout`.
///
/// Runs OUTSIDE the `borrow_mut` from the just-finished
/// `create_*_navigator` call — the closure re-enters the walker
/// (it builds an entire layout subtree using the same backend Rc),
/// and a nested re-borrow on the outer `RefCell` panics with
/// "already borrowed". Same pattern the walker already uses for
/// `drawer_navigator_attach_sidebar`.
///
/// Backends that don't honor a layout (iOS/Android/Roku) inherit the
/// no-op default and silently drop the call. The web backend's
/// override wires the root into the navigator container and records
/// the outlet so subsequent screen attaches mount inside it. The
/// recording backend (`dev-server`) emits
/// `Command::AttachNavigatorLayout` so the runtime-server wire client can
/// reproduce the same wiring on its side.
fn invoke_layout_and_attach<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    navigator: &B::Node,
    build_layout: Option<Rc<dyn Fn() -> primitives::navigator::LayoutPlan<B::Node>>>,
) {
    let Some(build_layout) = build_layout else {
        return;
    };
    let plan = build_layout();
    // Resolve the outlet's backend `Node` via the `Ref<ViewHandle>`
    // the layout binds during build. If the layout author embedded
    // `LayoutProps::outlet` correctly the handle is present; if not,
    // there's no outlet and the backend gets no `attach` call (so
    // screens would fall back to mounting in the navigator's bare
    // container). Same downcast the web backend's local-render
    // microtask used to do inline.
    let Some(handle) = plan.outlet_ref.get() else {
        return;
    };
    let any_node = handle.as_any();
    let Some(outlet) = any_node.downcast_ref::<B::Node>() else {
        return;
    };
    backend
        .borrow_mut()
        .attach_navigator_layout(navigator, plan.root, outlet.clone());
}

/// Bridge a `header_background` / `title_color` / `header_tint`
/// callback (set via the ergonomic `.header(idea_header(...))` path on
/// `ScreenOptions.default_options`) into one of the navigator's
/// `apply_navigator_*_style` Backend hooks.
///
/// Without this bridge, the closure is only invoked once per screen
/// mount (when `attach_toolbar_to_body` reads the resolved color),
/// so the toolbar's colors freeze at mount time. Wrapping it in an
/// `Effect` here makes the closure re-run whenever the signals it
/// reads change — `active_theme()` in the common idea-ui case — and
/// dispatches a synthetic single-field `StyleRules` through the
/// same Backend hook the explicit `header_style(...)` setter uses,
/// so platform impls only have to wire one input.
///
/// The Effect's lifetime is the active scope at the navigator's
/// build site, so it auto-drops on navigator unmount.
fn attach_navigator_color_callback<B, FApply, FSet>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    callback: Rc<dyn Fn() -> crate::Color>,
    set_field: FSet,
    apply_fn: FApply,
) where
    B: Backend + 'static,
    FApply: Fn(&mut B, &B::Node, &Rc<StyleRules>) + 'static,
    FSet: Fn(&mut StyleRules, crate::Color) + 'static,
{
    let backend_c = backend.clone();
    let node_c = node.clone();
    let _e = Effect::new(move || {
        let color = callback();
        let mut rules = StyleRules::default();
        set_field(&mut rules, color);
        let rc_rules: Rc<StyleRules> = Rc::new(rules);
        apply_fn(&mut backend_c.borrow_mut(), &node_c, &rc_rules);
    });
}

/// Attach a style to a navigator sub-component (header, title, etc.).
/// Same cohort-based theme reactivity as `attach_style_static`, but
/// calls a custom apply function instead of `backend.apply_style`.
fn attach_navigator_slot_style<B, F>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: StyleSource,
    apply_fn: F,
) where
    B: Backend + 'static,
    F: Fn(&Rc<RefCell<B>>, &B::Node, &Rc<StyleRules>) + Clone + 'static,
{
    match style {
        StyleSource::Static(app) => {
            let resolved = resolve_style(&app);
            apply_fn(backend, node, &resolved);

            install_theme_cohort_driver(backend);
            let backend_c = backend.clone();
            let node_c = node.clone();
            let app_rc = Rc::new(app);
            let apply_fn_c = apply_fn.clone();
            let cohort_id = theme_cohort_register(Box::new(move || {
                let resolved = resolve_style(&app_rc);
                apply_fn_c(&backend_c, &node_c, &resolved);
            }));
            struct SlotGuard(super::theme_cohort::CohortId);
            impl Drop for SlotGuard {
                fn drop(&mut self) {
                    theme_cohort_unregister(self.0);
                }
            }
            reactive::adopt_guard_into_active_scope(SlotGuard(cohort_id));
        }
        StyleSource::Reactive(f) => {
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let resolved = resolve_style(&app);
                apply_fn(&backend_c, &node_c, &resolved);
            });
        }
        StyleSource::SignalClass(spec) => {
            // Navigator-slot styles don't go through the standard
            // class-apply path; fall back to the compute closure so
            // signal-class semantics still hold (just without the
            // JS-side dispatcher).
            let f = spec.compute_fallback.clone();
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let resolved = resolve_style(&app);
                apply_fn(&backend_c, &node_c, &resolved);
            });
        }
    }
}

// ----- build_navigator (stack) ----------------------------------------------

/// Build a Navigator. Stands up the per-screen scope registry, builds
/// the `NavigatorCallbacks` bundle, wires the user-facing handle's
/// control plane, mounts the initial screen, and returns the native
/// container node. Mirrors `build_virtualizer` — both manage a set of
/// nested scopes that map 1:1 with a backend-owned UI container.
fn build_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    _default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, LayoutPlan, LayoutProps, MountResult, NavState, NavigatorCallbacks,
        NavigatorControl,
    };

    // Capture this navigator's Identity now — see
    // `build_drawer_navigator` for the full rationale. Closures that
    // fire later (mount_screen, build_layout) re-establish this as
    // their parent so per-route content gets stable, distinct wire
    // ids across rebuilds.
    let nav_identity = crate::current_identity();

    // Per-screen scope registry. The framework owns the scopes — the
    // backend stores opaque scope ids alongside its native cells and
    // calls `release_screen(id)` to drop the matching scope. Same
    // discipline as Virtualizer.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    // Screen table is `Rc`'d so the mount + match closures can clone it.
    // Each entry holds the route's path pattern + typed builder + segment-parser
    // (see `RouteEntry`).
    let screens = Rc::new(screens);

    // Control plane — handed to the handle now; populated by the
    // backend's `create_navigator` impl.
    let control = Rc::new(NavigatorControl::new());

    // mount_screen: look up the screen builder, build the screen
    // inside a fresh per-screen Scope, return (node, scope_id).
    // Panics on unregistered route — declaring routes is the
    // navigator's contract.
    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        // Hold the control as a Weak — a strong clone here would form a
        // cycle (control owns the dispatcher closure, the dispatcher
        // closure captures this `mount` Rc, and `mount` captures the
        // control). That cycle keeps the control's per-screen `scopes`
        // alive past `release_screen`/`drop_subtree`, leaving theme
        // cohort entries pointing at freed Taffy slots.
        let control_for_mount = Rc::downgrade(&control);
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("Navigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let control_strong = control_for_mount
                .upgrade()
                .expect("mount_screen called after navigator dropped");
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_strong);
            // Screen channel = slot 0 of the navigator; route name as
            // the key so per-route subtrees don't alias.
            let screen_id = crate::Identity::node(
                nav_identity,
                0,
                None,
                Some(crate::hash_key(name)),
            );
            let (node, options) = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(screen_id, || {
                    let screen = builder(params);
                    let n = super::build(&backend, 0, screen.primitive);
                    (n, screen.options)
                })
            });
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    // release_screen: drop the scope. The Drop impl on `Scope` frees
    // every signal/effect/ref scoped to the screen, including the
    // child subtree's `Effect`s.
    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    // match_path: pure function from URL → (route name, typed params).
    // Walks the screen table and tries each pattern in registration
    // order; returns the first match whose segments parse cleanly.
    // The web backend calls this on mount + popstate; an SSR backend
    // would call it once per request.
    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    // Reactive nav-state signals. The dispatcher updates them on
    // every commit; layout effects subscribe to whichever they care
    // about. Initial values match the about-to-mount initial route.
    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    // Hand the state to the control plane so `dispatch(...)` can
    // update the signals before the backend's dispatcher runs.
    control.attach_nav_state(nav_state.clone());

    // depth_changed: backend reports stack depth after each commit.
    // We update both the control plane (so `handle.depth()` is a
    // cheap probe) and the `nav_state.depth` signal (so reactive
    // layouts re-render). `can_go_back` is derived from depth.
    //
    // Weak — same cycle concern as `control_for_mount`: this closure
    // ends up captured by the backend's dispatcher, which is owned
    // by the control. A strong clone would close the loop and pin
    // the navigator forever.
    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = Rc::downgrade(&control);
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            if let Some(c) = control.upgrade() {
                c.set_depth(d);
            }
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    // Layout-scope. Layouts contain reactive effects (e.g. a
    // `Text { format!("{}", active_route.get()) }` in the chrome)
    // that must keep firing on every navigation. Without a scope
    // owner, those effects would free immediately when the
    // `Effect` handle drops at the end of `build()` — because the
    // layout is built from a microtask (web) which runs detached
    // from the navigator's enclosing render scope, the
    // thread-local active-scope stack is empty at build time.
    //
    // The fix: give the layout its own long-lived scope. We own
    // it here in `build_navigator`; it stays alive as long as the
    // navigator does, and effects registered during the layout
    // build attach to it. Dropping the scope tears down every
    // layout effect — handled by the cleanup `Effect` the walker
    // installs around `Primitive::Navigator` (it lives in the
    // surrounding scope; when *that* drops, this navigator and
    // its layout_scope go with it).
    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));

    // build_layout: invoked by backends that render through a
    // user-supplied layout (web). The framework runs the layout
    // closure with a freshly-created outlet `View` (whose ref the
    // backend later uses to find the outlet's native node), builds
    // the resulting `Primitive` into a native node via the standard
    // build walker — wrapped in `with_scope(layout_scope)` so
    // layout effects survive past the build call.
    //
    // **Borrow safety**: this closure calls `build(&backend, 0, ...)`
    // which does `backend.borrow_mut()`. Backends must only invoke
    // build_layout *outside* the `create_navigator` borrow window —
    // typically from a microtask scheduled during create, the same
    // pattern web uses for `mount_screen`.
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            let on_back: Rc<dyn Fn()> = {
                let control = control.clone();
                Rc::new(move || control.pop())
            };
            let props = LayoutProps {
                outlet: outlet_primitive,
                // Stack navigators don't have sidebars. Hand the
                // layout an empty View so authors don't have to
                // write a None-case branch — they can embed it
                // unconditionally or ignore it.
                sidebar: crate::view(Vec::new()).into(),
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            // Layouts may contain `Link`s in their chrome (a nav bar
            // with a "Home" link, etc.). Push this navigator's
            // control plane onto the ambient stack BEFORE invoking
            // the layout closure — the `link()` constructor calls
            // `ambient_navigator()` at construction time, which is
            // *during* the layout closure's run. If the guard fires
            // after layout_fn returns, every Link in the chrome
            // captures `None` and clicking them silently no-ops.
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            let root_primitive = layout_fn(props);
            // Build the layout subtree inside its dedicated scope.
            // Every Effect created during the build (reactive
            // text, button state, etc.) attaches to this scope and
            // stays alive across navigation. Without this wrap,
            // those effects would drop immediately because the
            // layout build runs detached from any active scope.
            let mut scope = Box::new(reactive::Scope::new());
            // Layout channel = slot 2 of the navigator's identity.
            let layout_id =
                crate::Identity::node(nav_identity, 2, None, None);
            let root = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(layout_id, || {
                    super::build(&backend, 0, root_primitive)
                })
            });
            // Stash the scope on the slot so it stays alive for the
            // navigator's lifetime. The slot itself is dropped in
            // `release_navigator` via the cleanup effect, which
            // drops `layout_scope` along with everything else.
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    let callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state: nav_state.clone(),
        depth_changed,
        // Framework-driven build: the walker calls `mount_screen` +
        // `navigator_attach_initial` directly below, and backends
        // that auto-mount on URL match (web) keep doing so.
        defer_initial_mount: false,
    };

    // Create the native navigator. The backend stores the callbacks,
    // installs a dispatcher on `control`, but DOES NOT call
    // `mount_screen` synchronously (would re-enter the backend's
    // borrow_mut → panic). The framework handles initial mount below.
    let mount_screen_for_initial = callbacks.mount_screen.clone();
    let build_layout_after_create = callbacks.build_layout.clone();
    let node = time_backend_create(pkind!(Navigator), || {
        backend.borrow_mut().create_navigator(
            callbacks,
            control.clone(),
            &AccessibilityProps::default(),
        )
    });

    invoke_layout_and_attach(backend, &node, build_layout_after_create);

    // Mount the initial screen *after* `create_navigator` returns —
    // i.e. outside the borrow_mut window. The screen build
    // re-enters the build walker which itself does `borrow_mut`, so
    // it MUST run outside any active backend borrow. The result is
    // handed to the backend via `navigator_attach_initial`, which
    // is a thin "stick this screen into the container" hook with no
    // borrow contention (it doesn't call back into build).
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::Navigator(fill)) = ref_fill {
        // The default handle the trait builds is a no-op (`control: None`).
        // For backends that override `make_navigator_handle` and wire up
        // the control plane, the user gets the live handle. Default-no-op
        // backends produce a handle whose calls are silent no-ops —
        // matching every other "primitive ref that the backend doesn't
        // support yet" path in the framework.
        let handle = backend.borrow().make_navigator_handle(&node);
        fill(handle);
    }

    // See `build_drawer_navigator` for the rationale — backends that
    // don't store `build_layout` in their callbacks would otherwise
    // drop the layout scope (and every reactive style closure in the
    // layout) as soon as this function returns.
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_layout_scope_keepalive;
    });

    node
}

// ----- build_tab_navigator --------------------------------------------------

/// Build a TabNavigator. Shares the per-screen scope registry and
/// ambient-nav wiring with `build_navigator`; differs in the
/// callbacks bundle (carries tab metadata + mount policy) and the
/// backend hook called (`create_tab_navigator`).
#[allow(clippy::too_many_arguments)]
fn build_tab_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    tab_order: Vec<(&'static str, primitives::navigator::TabSpec)>,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    placement: primitives::navigator::TabPlacement,
    mount_policy: primitives::navigator::MountPolicy,
    _default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, DefaultLinkKind, LayoutPlan, LayoutProps, MountResult, NavState,
        NavigatorCallbacks, NavigatorControl, TabNavigatorCallbacks, TabRegistration,
    };

    // Capture this navigator's Identity for the screen-mount closure
    // — see `build_drawer_navigator` for the rationale.
    let nav_identity = crate::current_identity();

    // Per-screen scope registry — same discipline as stack.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
    let screens = Rc::new(screens);
    let control = Rc::new(NavigatorControl::new());
    control.set_default_link_kind(DefaultLinkKind::Select);

    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        // Weak — see comment on the stack navigator's `control_for_mount`.
        let control_for_mount = Rc::downgrade(&control);
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("TabNavigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let control_strong = control_for_mount
                .upgrade()
                .expect("mount_screen called after navigator dropped");
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_strong);
            // Screen channel = slot 0 of the tab navigator; route
            // name as the key so per-tab subtrees stay distinct.
            let screen_id = crate::Identity::node(
                nav_identity,
                0,
                None,
                Some(crate::hash_key(name)),
            );
            let (node, options) = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(screen_id, || {
                    let screen = builder(params);
                    let n = super::build(&backend, 0, screen.primitive);
                    (n, screen.options)
                })
            });
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        // Tabs don't have stack depth; pin to 1 so layouts that
        // read `depth` see a sensible value (effectively "we're at
        // the root of the active tab"). Backends with nested stacks
        // inside tabs report the *active tab's* stack depth via
        // their own depth_changed; until then, 1 is correct.
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

    // Weak — see the stack navigator's `depth_changed` for the
    // cycle-avoidance rationale.
    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = Rc::downgrade(&control);
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            if let Some(c) = control.upgrade() {
                c.set_depth(d);
            }
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    // Active-changed callback. Backends fire this after the tap
    // commits (e.g. for analytics); the reactive nav-state signals
    // have already been updated by `control.dispatch(...)`.
    let active_changed: Rc<dyn Fn(&'static str)> = Rc::new(|_name| {});

    // Layout slot — same shape as stack's. Tabs may want a top app
    // bar that spans tabs (e.g. a search field that lives above the
    // tab bar); the layout closure renders the chrome and embeds
    // the outlet where the active tab's content goes.
    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            // Tabs don't have a back-button — `on_back` is a no-op.
            // Layout authors who need one should hide it via the
            // `can_go_back` signal, which stays false for pure tab
            // navigators.
            let on_back: Rc<dyn Fn()> = Rc::new(|| {});
            let props = LayoutProps {
                outlet: outlet_primitive,
                // Tab navigators don't have sidebars either.
                sidebar: crate::view(Vec::new()).into(),
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            let root_primitive = layout_fn(props);
            let mut scope = Box::new(reactive::Scope::new());
            // Layout channel = slot 2 of the tab navigator's identity.
            let layout_id =
                crate::Identity::node(nav_identity, 2, None, None);
            let root = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(layout_id, || {
                    super::build(&backend, 0, root_primitive)
                })
            });
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    // Translate the `Vec<(name, TabSpec)>` author input into the
    // `Vec<TabRegistration>` shape backends receive. Same data,
    // flat structure (no nested tuples).
    let tabs: Vec<TabRegistration> = tab_order
        .into_iter()
        .map(|(route, spec)| TabRegistration {
            route,
            label: spec.label,
            icon: spec.icon,
            badge: spec.badge,
        })
        .collect();

    let nav_callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state,
        depth_changed,
        defer_initial_mount: false,
    };
    let callbacks = TabNavigatorCallbacks {
        navigator: nav_callbacks,
        tabs,
        placement,
        mount_policy,
        active_changed,
    };

    let mount_screen_for_initial = callbacks.navigator.mount_screen.clone();
    let build_layout_after_create = callbacks.navigator.build_layout.clone();
    let node = time_backend_create(pkind!(TabNavigator), || {
        backend.borrow_mut().create_tab_navigator(
            callbacks,
            control.clone(),
            &AccessibilityProps::default(),
        )
    });

    invoke_layout_and_attach(backend, &node, build_layout_after_create);

    // Mount the initial tab's screen after create_tab_navigator
    // returns (outside the borrow_mut window). Same pattern as the
    // stack navigator's `navigator_attach_initial`. Backends that
    // defer initial mount to a microtask (web) leave the default
    // no-op; backends that mount synchronously (Android) implement
    // `tab_navigator_attach_initial`.
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .tab_navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::TabNavigator(fill)) = ref_fill {
        let handle = backend.borrow().make_tab_navigator_handle(&node);
        fill(handle);
    }

    // See `build_drawer_navigator` for the rationale.
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_layout_scope_keepalive;
    });

    node
}

// ----- build_drawer_navigator -----------------------------------------------

/// Build a DrawerNavigator. Same per-screen scope machinery as the
/// stack and tab navigators; additionally exposes an `is_open`
/// signal the backend's dispatcher flips on
/// `OpenDrawer`/`CloseDrawer`/`ToggleDrawer` commands.
#[allow(clippy::too_many_arguments)]
fn build_drawer_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    content: Option<primitives::navigator::ContentBuilder>,
    side: primitives::navigator::DrawerSide,
    drawer_type: primitives::navigator::DrawerType,
    drawer_width: f32,
    swipe_to_open: bool,
    mount_policy: primitives::navigator::MountPolicy,
    default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
    background_color: Option<Rc<dyn Fn() -> crate::Color>>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, DefaultLinkKind, DrawerContentProps, DrawerNavigatorCallbacks, LayoutPlan,
        LayoutProps, MountResult, NavState, NavigatorCallbacks, NavigatorControl,
    };

    // Capture this navigator's Identity now so the screen-mount and
    // sidebar-content closures (which fire later, outside the
    // walker's main pass and therefore with `current_identity()`
    // reset to UNIDENTIFIED) can re-establish the navigator's scope
    // as the parent for everything they build. Each channel inside
    // the navigator (screen / sidebar / layout) uses a different
    // slot so they don't alias. Per-route content uses the route
    // name as a key so different screens at "slot 0 of the screen
    // channel" get distinct identities — otherwise route A's
    // top-level Text and route B's top-level Button would land on
    // the same wire NodeId after the first swap.
    let nav_identity = crate::current_identity();

    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
    let screens = Rc::new(screens);
    let control = Rc::new(NavigatorControl::new());
    control.set_default_link_kind(DefaultLinkKind::Select);
    let default_options = Rc::new(default_options);

    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        // Weak — see comment on the stack navigator's `control_for_mount`.
        let control_for_mount = Rc::downgrade(&control);
        let defaults_for_mount = default_options.clone();
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("DrawerNavigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let control_strong = control_for_mount
                .upgrade()
                .expect("mount_screen called after navigator dropped");
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_strong);
            // Screen channel = slot 0 of the navigator's identity;
            // route name as the key so per-route content gets its
            // own subtree of identities.
            let screen_id = crate::Identity::node(
                nav_identity,
                0,
                None,
                Some(crate::hash_key(name)),
            );
            let (node, screen_options) = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(screen_id, || {
                    let screen = builder(params);
                    let n = super::build(&backend, 0, screen.primitive);
                    (n, screen.options)
                })
            });
            // Layer the screen's per-screen options on top of the
            // drawer's navigator-level defaults — `merge` keeps any
            // field the per-screen options explicitly set and falls
            // back to the navigator default otherwise. This is how
            // `.header_background(...)` on the navigator becomes the
            // implicit bar color for every screen without each
            // `Screen::new(...)` having to repeat it.
            let mut options = match defaults_for_mount.as_ref() {
                Some(d) => d.clone().merge(&screen_options),
                None => screen_options,
            };
            // Default drawer-toggle hamburger when the author didn't
            // specify a left header button. Drawer-rooted screens
            // almost always want this, so making it the default
            // means the per-screen wiring stays focused on the
            // screen's body — the framework knows the screen sits
            // under a drawer and knows what icon + action belongs
            // there. Override is still trivial: pass any
            // `header_left(...)` from the author closure.
            if options.header_left.is_none() {
                // Capture as Weak — this on_press is reachable from the
                // drawer's own `scopes` storage (via the screen's options
                // → header_left), so a strong clone would close the
                // ownership cycle that `control_for_mount` exists to
                // avoid.
                let control = control_for_mount.clone();
                options.header_left = Some(primitives::navigator::HeaderButton::new(
                    "line.3.horizontal",
                    move || {
                        if let Some(c) = control.upgrade() {
                            c.dispatch(primitives::navigator::NavCommand::ToggleDrawer);
                        }
                    },
                ));
            }
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

    // Reactive drawer-open signal. The backend's dispatcher flips
    // this in response to `OpenDrawer`/`CloseDrawer`/`ToggleDrawer`
    // commands; layout closures subscribe to it to drive the
    // hamburger icon's open/close visual.
    let is_open = Signal::new(false);

    // Weak — see the stack navigator's `depth_changed` for the
    // cycle-avoidance rationale.
    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = Rc::downgrade(&control);
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            if let Some(c) = control.upgrade() {
                c.set_depth(d);
            }
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    let active_changed: Rc<dyn Fn(&'static str)> = Rc::new(|_name| {});
    let open_changed: Rc<dyn Fn(bool)> = Rc::new(move |open| is_open.set(open));

    // Wrap the user's `.content(...)` closure in a factory that
    // constructs a fresh `DrawerContentProps` each call and returns
    // the rendered Primitive. We need the factory shape because
    // both `build_layout` (web) and `build_content` (native) call
    // the closure, and they may be invoked at different times in
    // different scopes.
    let make_content_primitive: Option<Rc<dyn Fn() -> Primitive>> = content.map(|content_fn| {
        let active_route = nav_state.active_route;
        let is_open = is_open;
        let control_for_select = control.clone();
        let control_for_close = control.clone();
        let f: Rc<dyn Fn() -> Primitive> = Rc::new(move || {
            let on_select: Rc<dyn Fn(&'static str)> = {
                let control = control_for_select.clone();
                Rc::new(move |name: &'static str| {
                    // The `Select` URL doesn't matter to native
                    // (native ignores URLs); on web the dispatch
                    // path resolves it from the route's `path()`,
                    // but we don't have a Route<()> here — pass
                    // an empty URL. The web dispatcher reads URL
                    // from the active_path signal at activation
                    // time, so this is fine.
                    control.dispatch(primitives::navigator::NavCommand::Select {
                        name,
                        url: String::new(),
                        params: Box::new(()),
                    });
                })
            };
            let on_close: Rc<dyn Fn()> = {
                let control = control_for_close.clone();
                Rc::new(move || {
                    control.dispatch(primitives::navigator::NavCommand::CloseDrawer)
                })
            };
            let props = DrawerContentProps {
                active_route,
                is_open,
                on_select,
                on_close,
            };
            content_fn(props)
        });
        f
    });

    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let make_content = make_content_primitive.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            // Drawer's `on_back` toggles the drawer — back action
            // semantics on a drawer-rooted screen. Layout authors
            // who want stack-style back can hold a separate handle.
            let on_back: Rc<dyn Fn()> = {
                let control = control.clone();
                Rc::new(move || control.dispatch(primitives::navigator::NavCommand::ToggleDrawer))
            };
            // Push the ambient nav BEFORE building the drawer
            // content so any `Link`s in it capture this drawer.
            // The guard covers both the content build and the
            // layout closure's run; dropped at end of this scope.
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            // Build the drawer-content Primitive (or empty View if
            // no `.content(...)` was registered). Either way,
            // LayoutProps carries a Primitive — the layout author
            // embeds it unconditionally.
            let sidebar_primitive: Primitive = match make_content.as_ref() {
                Some(f) => f(),
                None => crate::view(Vec::new()).into(),
            };
            let props = LayoutProps {
                outlet: outlet_primitive,
                sidebar: sidebar_primitive,
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            let root_primitive = layout_fn(props);
            let mut scope = Box::new(reactive::Scope::new());
            // Layout channel = slot 2 of the navigator's identity.
            // Distinct from screen (slot 0) and sidebar (slot 1) so
            // top-level nodes in each channel get distinct wire ids.
            let layout_id =
                crate::Identity::node(nav_identity, 2, None, None);
            let root = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(layout_id, || {
                    super::build(&backend, 0, root_primitive)
                })
            });
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    // `build_content` — used by native backends that render the
    // drawer panel themselves (iOS/Android drawer shells). The
    // closure builds the user's content Primitive into a backend
    // Node inside a dedicated scope so reactive effects in the
    // panel survive across drawer state changes.
    let content_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_content: Option<Rc<dyn Fn() -> B::Node>> = make_content_primitive
        .as_ref()
        .map(|make_content| {
            let make_content = make_content.clone();
            let backend = backend.clone();
            let control = control.clone();
            let content_scope_slot = content_scope.clone();
            let f: Rc<dyn Fn() -> B::Node> = Rc::new(move || {
                // Same ambient-nav posture as build_layout: Links
                // in the panel capture this drawer's control.
                let _ambient_guard =
                    primitives::navigator::AmbientNavGuard::push(control.clone());
                let primitive = make_content();
                let mut scope = Box::new(reactive::Scope::new());
                // Sidebar channel = slot 1 of the navigator's
                // identity. Keeps sidebar nodes from aliasing with
                // screen nodes (slot 0) on the wire.
                let sidebar_id =
                    crate::Identity::node(nav_identity, 1, None, None);
                let node = reactive::with_scope(&mut scope, || {
                    crate::with_current_identity(sidebar_id, || {
                        super::build(&backend, 0, primitive)
                    })
                });
                *content_scope_slot.borrow_mut() = Some(scope);
                node
            });
            f
        });

    let nav_callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state,
        depth_changed,
        defer_initial_mount: false,
    };
    let callbacks = DrawerNavigatorCallbacks {
        navigator: nav_callbacks,
        side,
        drawer_type,
        drawer_width,
        swipe_to_open,
        mount_policy,
        is_open,
        build_content,
        active_changed,
        open_changed,
        background_color,
    };

    let mount_screen_for_initial = callbacks.navigator.mount_screen.clone();
    // Capture the content builder before moving `callbacks` into
    // the backend's `create_drawer_navigator` — we need it after
    // the create call returns (when the backend's borrow_mut is
    // released) to build the content Node and hand it back via
    // `drawer_navigator_attach_sidebar`. Web backends ignore this
    // path (they build the content via `build_layout`).
    let build_content_after_create = callbacks.build_content.clone();
    let build_layout_after_create = callbacks.navigator.build_layout.clone();
    let node = time_backend_create(pkind!(DrawerNavigator), || {
        backend.borrow_mut().create_drawer_navigator(
            callbacks,
            control.clone(),
            &AccessibilityProps::default(),
        )
    });

    // Build the layout subtree (if registered) and hand the resulting
    // root + outlet to the backend. Runs OUTSIDE any active
    // `borrow_mut` window — the closure re-enters the walker, which
    // also `borrow_mut`s, and a nested re-borrow on the same `RefCell`
    // panics with "already borrowed". Backends that don't render
    // through a layout (iOS/Android/Roku) inherit the default no-op
    // `attach_navigator_layout`. The web backend's local-render
    // microtask used to invoke `build_layout` itself; that path has
    // moved here so the runtime-server recording backend (which produces wire
    // commands) sees the same call shape — its
    // `attach_navigator_layout` override emits
    // `Command::AttachNavigatorLayout` so the wire client can wire
    // the same outlet up on its side.
    invoke_layout_and_attach(backend, &node, build_layout_after_create);

    // Build the drawer panel content (if registered) and hand the
    // resulting Node to the backend. Runs outside any active
    // borrow_mut window because the build re-enters the walker,
    // which also borrow_muts.
    if let Some(build_content) = build_content_after_create {
        let content_node = build_content();
        backend
            .borrow_mut()
            .drawer_navigator_attach_sidebar(&node, content_node);
    }

    // Mount the initial drawer screen — same pattern as the tab
    // navigator. Backends that mount via microtask (web) leave the
    // default no-op; backends that mount synchronously (Android)
    // implement `drawer_navigator_attach_initial`.
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .drawer_navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::DrawerNavigator(fill)) = ref_fill {
        let handle = backend.borrow().make_drawer_navigator_handle(&node);
        fill(handle);
    }

    // Keep the sidebar's and layout's reactive scopes alive for as
    // long as the navigator stays mounted. The build closures stash
    // a `Box<Scope>` into these Rcs; without this keepalive Effect
    // the only Rc references are the local vars (dropped on return)
    // and the closures' captures (dropped when `callbacks` and
    // `build_content_after_create` go out of scope), freeing the
    // scope and unsubscribing every reactive style closure in the
    // content/layout (e.g. content item highlights stop updating on
    // Select). Capturing the Rcs in an Effect that registers with
    // the surrounding render scope ties their lifetime to the
    // navigator's enclosing scope.
    let _content_scope_keepalive = content_scope.clone();
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_content_scope_keepalive;
        let _ = &_layout_scope_keepalive;
    });

    node
}
