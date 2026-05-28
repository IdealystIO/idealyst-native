//! `Element::Navigator` build path.
//!
//! Builds the framework substrate (routing, scopes, control plane,
//! reactive nav state) and hands the bundle to the SDK handler the
//! backend's registry resolves from the presentation TypeId. The
//! framework never sees the SDK's kind-specific config — it lives
//! on the presentation payload, which is `Rc<dyn Any>`.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives;
use crate::reactive::{self, Effect, Signal};
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
    config: Box<primitives::navigator::NavigatorConfig>,
    style: Option<StyleSource>,
    slot_styles: Vec<(&'static str, StyleSource)>,
    ref_fill: Option<RefFill>,
    accessibility: AccessibilityProps,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, MountResult, NavState, NavigatorControl, NavigatorHost,
    };

    let primitives::navigator::NavigatorConfig {
        initial,
        initial_path,
        screens,
        defer_initial_mount,
    } = *config;

    // Capture this navigator's identity for stable wire ids across rebuilds.
    let nav_identity = crate::current_identity();

    // Per-screen scope registry — framework owns scopes; SDK handler
    // holds opaque scope ids.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    let screens = Rc::new(screens);

    // Control plane — populated by the SDK handler when it installs
    // its dispatcher inside `init`.
    let control = Rc::new(NavigatorControl::new());

    // mount_screen: build a screen subtree inside its own scope; return
    // (node, scope_id, opaque options). Weak control to avoid Rc-cycle
    // through dispatcher → mount_screen → control.
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
                panic!("Navigator: route '{}' is not registered", name)
            });
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let control_strong = control_for_mount
                .upgrade()
                .expect("mount_screen called after navigator dropped");
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_strong);
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

    // Retained clone for the SSR initial-path consult below — the
    // original `match_path` is moved into the host.
    let match_path_for_ssr = match_path.clone();

    // Reactive nav-state mirror.
    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

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

    let active_changed: Rc<dyn Fn(&'static str, String)> = {
        let route_sig = nav_state.active_route;
        let path_sig = nav_state.active_path;
        Rc::new(move |name, path| {
            route_sig.set(name);
            path_sig.set(path);
        })
    };

    // SDK-chrome scope retention. Both `build_node` (navigator-scoped)
    // and `build_in_screen` (screen-scoped) need their built subtrees'
    // effects to outlive the build call. For nav-scoped chrome, we
    // accumulate scopes here and keep them alive via the cleanup
    // effect at the bottom. For screen-scoped chrome, the scope rides
    // on the per-screen `scopes` map (already established).
    let nav_chrome_scopes: Rc<RefCell<Vec<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let build_node: Rc<dyn Fn(Element) -> B::Node> = {
        let backend = backend.clone();
        let scopes_slot = nav_chrome_scopes.clone();
        let chrome_identity = crate::Identity::node(nav_identity, 2, None, None);
        Rc::new(move |prim| {
            let mut scope = Box::new(reactive::Scope::new());
            let node = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(chrome_identity, || {
                    super::build(&backend, 0, prim)
                })
            });
            scopes_slot.borrow_mut().push(scope);
            node
        })
    };

    // `build_node` + insert-into-parent, deferred-safe. A handler can
    // hand this to a microtask (which has no backend reference) so it
    // can attach chrome built post-borrow into an existing slot —
    // without reaching into a backend's node internals. Same
    // must-run-outside-the-outer-borrow rule as `build_node`.
    let build_node_into: Rc<dyn Fn(B::Node, Element)> = {
        let backend = backend.clone();
        let build_node = build_node.clone();
        Rc::new(move |mut parent, prim| {
            let node = build_node(prim);
            backend.borrow_mut().insert(&mut parent, node);
        })
    };

    let build_in_screen: Rc<dyn Fn(u64, Element) -> B::Node> = {
        let backend = backend.clone();
        let scopes_map = scopes.clone();
        let screen_chrome_identity = crate::Identity::node(nav_identity, 3, None, None);
        Rc::new(move |scope_id, prim| {
            // Build inside the named screen's existing scope so this
            // chrome's effects drop alongside the screen body when
            // `release_screen(scope_id)` fires.
            let mut map = scopes_map.borrow_mut();
            let scope = map.get_mut(&scope_id).unwrap_or_else(|| {
                panic!(
                    "build_in_screen: unknown scope_id {} — the screen \
                     was never mounted or has already been released",
                    scope_id
                )
            });
            reactive::with_scope(scope, || {
                crate::with_current_identity(screen_chrome_identity, || {
                    super::build(&backend, 0, prim)
                })
            })
        })
    };

    let host = NavigatorHost {
        initial_route: initial,
        initial_path,
        defer_initial_mount,
        mount_screen: mount_screen.clone(),
        release_screen,
        match_path,
        nav_state: nav_state.clone(),
        depth_changed,
        active_changed,
        control: control.clone(),
        build_node,
        build_node_into,
        build_in_screen,
    };

    let node = time_backend_create(pkind!(Navigator), || {
        backend
            .borrow_mut()
            .create_navigator(type_id, type_name, presentation, host, &accessibility)
    });

    if !defer_initial_mount {
        // Headless render-at-path (SSR): if a server-requested path was
        // set and resolves to a registered route, mount THAT screen with
        // its parsed params instead of the hardcoded `initial`, and sync
        // nav-state so chrome reads the right route. Backend-agnostic —
        // live backends never set this (they read the path from their
        // own platform in the SDK handler layer).
        let (route, params) = primitives::navigator::take_initial_path()
            .and_then(|path| {
                match_path_for_ssr(&path).map(|(name, params)| {
                    nav_state.active_route.set(name);
                    nav_state.active_path.set(path);
                    (name, params)
                })
            })
            .unwrap_or((initial, Box::new(())));
        let initial_result = mount_screen(route, params, None);
        backend.borrow_mut().navigator_attach_initial(
            &node,
            initial_result.node,
            initial_result.scope_id,
            initial_result.options,
        );
    }

    if let Some(style_source) = style {
        attach_style(backend, &node, style_source);
    }

    for (slot, style_source) in slot_styles {
        attach_slot_style(backend, &node, slot, style_source);
    }

    if let Some(RefFill::Navigator(fill)) = ref_fill {
        let handle = backend.borrow().make_navigator_handle(&node);
        fill(handle);
    }

    // Keep nav chrome scopes alive for the navigator's lifetime.
    let _chrome_keepalive = nav_chrome_scopes.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_chrome_keepalive;
    });

    node
}

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
                .apply_navigator_slot_style(node, slot, &rules);
        }
        StyleSource::Reactive(f) => {
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let rules = resolve_style(&app);
                backend_c
                    .borrow_mut()
                    .apply_navigator_slot_style(&node_c, slot, &rules);
            });
        }
        StyleSource::SignalClass(spec) => {
            let f = spec.compute_fallback.clone();
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let rules = resolve_style(&app);
                backend_c
                    .borrow_mut()
                    .apply_navigator_slot_style(&node_c, slot, &rules);
            });
        }
    }
}
