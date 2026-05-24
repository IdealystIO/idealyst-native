//! `Primitive::NavigatorExt` build path — the unified entry point for
//! any registered navigator kind.
//!
//! Mirrors the substrate of `walker::navigator::build_navigator` but
//! routes the final create through the new
//! [`Backend::create_navigator_extension`] hook, which dispatches via
//! the per-backend `NavigatorRegistry` to a registered
//! [`NavigatorHandler`](crate::primitives::navigator::NavigatorHandler).
//!
//! The per-kind walkers in [`super::navigator`] remain in place during
//! the SDK migration; this module is the new path that SDK builders
//! (`stack-navigator`, `tab-navigator`, `drawer-navigator`, third-party)
//! produce primitives for.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitive::Primitive;
use crate::primitives;
use crate::reactive::{self, Effect, Ref, Signal};
use crate::sources::StyleSource;
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    type_id: TypeId,
    type_name: &'static str,
    presentation: Rc<dyn Any>,
    config: Box<primitives::navigator::NavigatorExtConfig>,
    style: Option<StyleSource>,
    slot_styles: Vec<(&'static str, StyleSource)>,
    ref_fill: Option<RefFill>,
    accessibility: AccessibilityProps,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, LayoutPlan, LayoutProps, MountResult, NavState, NavigatorControl,
        NavigatorHost,
    };

    // Destructure the shared config.
    let primitives::navigator::NavigatorExtConfig {
        initial,
        initial_path,
        screens,
        layout,
        default_options: _default_options,
        default_link_kind,
        defer_initial_mount,
    } = *config;

    // Capture this navigator's identity for stable wire ids across rebuilds.
    let nav_identity = crate::current_identity();

    // Per-screen scope registry — framework owns scopes; backend holds opaque ids.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    let screens = Rc::new(screens);

    // Control plane — populated by the registered handler when the backend
    // installs the dispatcher inside `create_navigator_extension`.
    let control = Rc::new(NavigatorControl::new());
    control.set_default_link_kind(default_link_kind);

    // mount_screen: build a screen subtree inside its own scope; return
    // (node, scope_id, options). Weak control to avoid Rc-cycle through
    // dispatcher → mount_screen → control.
    let mount_screen: Rc<
        dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<B::Node>,
    > = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        let control_for_mount = Rc::downgrade(&control);
        Rc::new(move |name, params, state| {
            let entry = screens.get(name).unwrap_or_else(|| {
                panic!("NavigatorExt: route '{}' is not registered", name)
            });
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let control_strong = control_for_mount
                .upgrade()
                .expect("mount_screen called after navigator dropped");
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_strong);
            // Per-screen state stack: pushed for the duration of the
            // screen build so the user's render closure can read it via
            // `current_screen_state::<T>()`.
            let _state_guard = primitives::navigator::shared::ScreenStateGuard::push(state);
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

    // release_screen: drop the scope to free signals/effects scoped to the screen.
    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    // match_path: URL → (route name, typed params). Used by web/SSR.
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

    // Reactive nav-state mirror. The dispatcher updates these on every command.
    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

    // depth_changed: handler reports stack depth changes; we mirror to control
    // + nav_state.depth signal + can_go_back. Weak control to break cycle.
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

    // active_changed: handler reports a non-depth-change route switch (tab
    // tap, drawer item select). Updates the route/path signals.
    let active_changed: Rc<dyn Fn(&'static str, String)> = {
        let route_sig = nav_state.active_route;
        let path_sig = nav_state.active_path;
        Rc::new(move |name, path| {
            route_sig.set(name);
            path_sig.set(path);
        })
    };

    // Layout scope — long-lived owner for any effects inside the layout
    // closure (reactive text in nav bar, themed colors, etc.).
    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));

    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive =
                crate::view(Vec::new()).bind(outlet_ref.clone()).into();
            let on_back: Rc<dyn Fn()> = {
                let control = control.clone();
                Rc::new(move || control.pop())
            };
            let props = LayoutProps {
                outlet: outlet_primitive,
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
            let layout_id = crate::Identity::node(nav_identity, 2, None, None);
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

    // Build the host bundle and hand it to the backend's extension creator.
    let host = NavigatorHost {
        initial_route: initial,
        initial_path,
        defer_initial_mount,
        mount_screen: mount_screen.clone(),
        release_screen,
        match_path,
        build_layout,
        nav_state: nav_state.clone(),
        depth_changed,
        active_changed,
        control: control.clone(),
    };

    let node = time_backend_create(pkind!(NavigatorExt), || {
        backend
            .borrow_mut()
            .create_navigator_extension(type_id, type_name, presentation, host, &accessibility)
    });

    // Mount initial screen — outside the borrow window so the screen's
    // build can re-enter the walker. Skipped when defer_initial_mount is set
    // (handler self-mounts, typically web reading the URL on load).
    if !defer_initial_mount {
        let initial_result = mount_screen(initial, Box::new(()), None);
        // The handler installed via create_navigator_extension owns the
        // attach_initial flow; the backend dispatches via the stored
        // handler. See `Backend::navigator_extension_attach_initial`.
        backend.borrow_mut().navigator_extension_attach_initial(
            &node,
            initial_result.node,
            initial_result.scope_id,
            initial_result.options,
        );
    }

    // Body-level style — same attach_style path every other primitive uses.
    if let Some(style_source) = style {
        attach_style(backend, &node, style_source);
    }

    // SDK-defined slot styles — dispatch each through
    // `apply_navigator_extension_slot_style` which the backend routes to
    // its registered handler's `apply_slot_style`.
    for (slot, style_source) in slot_styles {
        attach_slot_style(backend, &node, slot, style_source);
    }

    // Ref-fill — fires after the navigator's node exists. The handler
    // (via the backend's `make_navigator_extension_handle`) wires the
    // control plane into the returned `NavigatorHandle`.
    if let Some(RefFill::NavigatorExt(fill)) = ref_fill {
        let handle = backend.borrow().make_navigator_extension_handle(&node);
        fill(handle);
    }

    // Keep the layout scope alive for the navigator's lifetime — same
    // pattern as `build_navigator`. The cleanup effect drops when the
    // enclosing scope drops.
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_layout_scope_keepalive;
    });

    node
}

/// Bridge a slot StyleSource into a reactive apply call that fires on
/// `apply_navigator_extension_slot_style`. Matches the shape of
/// [`super::navigator::attach_navigator_slot_style`] minus the
/// theme-cohort dance (SDK-registered slots opt-in to theme reactivity
/// via the standard `Reactive` closure form; full cohort wiring will
/// land alongside the per-kind walkers' eventual removal).
fn attach_slot_style<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    slot: &'static str,
    style: StyleSource,
) {
    use crate::style::resolve as resolve_style;
    match style {
        StyleSource::Static(app) => {
            let rules = resolve_style(&app);
            backend
                .borrow_mut()
                .apply_navigator_extension_slot_style(node, slot, &rules);
        }
        StyleSource::Reactive(f) => {
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let rules = resolve_style(&app);
                backend_c
                    .borrow_mut()
                    .apply_navigator_extension_slot_style(&node_c, slot, &rules);
            });
        }
        StyleSource::SignalClass(spec) => {
            // Fall back to the compute closure (same as the per-kind
            // navigator slot path) — class-apply pathway doesn't reach
            // navigator slots; the reactive closure form gives equivalent
            // re-fire semantics.
            let f = spec.compute_fallback.clone();
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let rules = resolve_style(&app);
                backend_c
                    .borrow_mut()
                    .apply_navigator_extension_slot_style(&node_c, slot, &rules);
            });
        }
    }
}
