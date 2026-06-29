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
        current_nav_base, join_path, match_pattern, match_prefix, MountResult, NavBaseGuard,
        NavState, NavigatorControl, NavigatorHost,
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

    // This navigator's hierarchy base prefix — the URL the parent screen it's
    // nested in is mounted at (empty for the root). Read from the thread-local
    // set by the parent's `mount_screen`. Route patterns in `screens` are
    // RELATIVE to this base; the control composes `base + url` on dispatch and
    // `match_path` strips it before matching. A root navigator has base "" so
    // every join/strip is a no-op.
    let my_base = current_nav_base();
    control.set_base(my_base.clone());

    // Reactive nav-state mirror, created BEFORE `mount_screen` so the latter
    // can capture `active_route` and `provide` it (as `ScreenNav`) into each
    // screen's scope — that's how a screen's portals learn to hide themselves
    // when their screen isn't the active route (see `walker::portal`).
    //
    // `active_path` is the FULL hierarchical path (base + this navigator's
    // initial relative path); root base "" leaves the configured initial path
    // unchanged.
    //
    // These signals are created inside a DEDICATED scope owned by the
    // `control` (an `Rc` that lives as long as the navigator), NOT the
    // ambient build scope. A nested navigator is frequently built inside a
    // transient dispatch/microtask scope (e.g. a stack hung under a drawer
    // screen, reached via a sidebar `on_select`); if `nav_state` were owned
    // by that scope its signals would be freed when it drops, and a later
    // `active_route.set(...)` from `mount_internal` / `on_popstate` would hit
    // a recycled arena slot ("signal used after its scope was dropped" /
    // type-mismatch — the QuillEMR forward/back nested-stack crash). Owning
    // the scope on the control frees them on navigator teardown instead —
    // leak-free, and never before the control itself drops.
    let mut nav_scope = Box::new(reactive::Scope::new());
    let nav_state = reactive::with_scope(&mut nav_scope, || NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(join_path(&my_base, initial_path)),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    });
    control.retain_scope(nav_scope);
    control.attach_nav_state(nav_state.clone());

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
        let base_for_mount = my_base.clone();
        let active_route_for_mount = nav_state.active_route;
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
            let _route_guard = primitives::navigator::shared::ScreenRouteGuard::push(name);
            // Publish the base prefix for any navigator nested in THIS screen:
            // our base + this route's (relative) path. A child navigator's
            // `current_nav_base()` reads this. No-op-shaped for the root.
            let _base_guard = NavBaseGuard::push(join_path(&base_for_mount, entry.path));
            let screen_id = crate::Identity::node(
                nav_identity,
                0,
                None,
                Some(crate::hash_key(name)),
            );
            let (node, options) = reactive::with_scope(&mut scope, || {
                // Publish this screen's nav context so any portal built in the
                // subtree (now, or later in a reactive `when`) can hide itself
                // when this screen isn't the active route — the fix for an
                // overlay opened here surviving a navigation to another screen
                // (the portal escapes to the window, and a persistent screen's
                // scope keeps it alive). See `ScreenNav` / `walker::portal`.
                reactive::provide(primitives::navigator::ScreenNav {
                    active_route: active_route_for_mount,
                    route: name,
                });
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

    // match_path: full URL → (route name, typed params) for THIS navigator.
    // Used by web/SSR. Strips this navigator's base prefix first, then matches
    // the remainder against the navigator's RELATIVE route patterns (root base
    // "" leaves the path unchanged, so single-navigator apps are unaffected).
    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        let base = my_base.clone();
        Rc::new(move |path| {
            let rel = match match_prefix(path, &base) {
                Some((_, rem)) => rem,
                None => return None, // path isn't under this navigator's base
            };
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(&rel, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    // resolve_entry: full URL → best PREFIX-matching (route, params, remainder)
    // for THIS navigator. Strips the base, then picks the route whose relative
    // pattern consumes the MOST segments (so a specific route beats an index
    // `""`), returning the unconsumed tail for a nested navigator to resolve.
    // The hierarchical / deep-link entry point.
    let resolve_entry: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>> = {
        let screens = screens.clone();
        let base = my_base.clone();
        Rc::new(move |path| {
            let rel = match_prefix(path, &base).map(|(_, rem)| rem)?;
            let mut best: Option<(&'static str, Box<dyn Any>, String, usize)> = None;
            for (name, entry) in screens.iter() {
                if let Some((segs, rem)) = match_prefix(&rel, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        let pat_len = entry.path.split('/').filter(|s| !s.is_empty()).count();
                        let better = best.as_ref().map(|(_, _, _, l)| pat_len > *l).unwrap_or(true);
                        if better {
                            best = Some((*name, params, rem, pat_len));
                        }
                    }
                }
            }
            best.map(|(n, p, r, _)| (n, p, r))
        })
    };

    // Retained clone of the PREFIX resolver for the non-deferred initial-mount
    // / deep-link consult below. The original `resolve_entry` is moved into the
    // host; clone it before the move so the synchronous (native/SSR/headless)
    // mount path can resolve THIS navigator's slice of a cold-start launch URL.
    let resolve_entry_for_initial = resolve_entry.clone();

    // Register this navigator in the global registry so the robot bridge can
    // enumerate it and report its route/depth/back-stack. The navigator's own
    // robot `ElementId` is the current parent (pushed in `build_inner` before
    // this dispatch), giving the inspector a `get_children` link to the current
    // screen's elements. Deregister on the same scope-drop that removes the
    // robot element entry, keeping the two registries consistent.
    #[cfg(feature = "robot")]
    {
        let element_id = crate::robot::current_parent().map(|e| e.0);
        let nav_id = primitives::navigator::register_navigator(&control, type_name, element_id);
        control.set_nav_id(nav_id);
        crate::reactive::on_cleanup(move || {
            primitives::navigator::deregister_navigator(nav_id)
        });
    }

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

    // Builder-taking variant: runs the `Element` construction INSIDE the chrome
    // scope so component bodies' standalone effects are owned by it (see
    // `NavigatorHost::build_node_scoped`). Identical to `build_node` except the
    // builder is invoked within `with_scope` rather than receiving a
    // pre-constructed `Element`.
    let build_node_scoped: Rc<dyn Fn(Box<dyn FnOnce() -> Element>) -> B::Node> = {
        let backend = backend.clone();
        let scopes_slot = nav_chrome_scopes.clone();
        let chrome_identity = crate::Identity::node(nav_identity, 2, None, None);
        let control_for_chrome = control.clone();
        Rc::new(move |builder| {
            let mut scope = Box::new(reactive::Scope::new());
            let node = reactive::with_scope(&mut scope, || {
                crate::with_current_identity(chrome_identity, || {
                    // Publish THIS navigator as the ambient one while the
                    // chrome (drawer sidebar / header) builds, exactly like
                    // `mount_screen` does for screen content. Without it,
                    // `link(route = …)` elements in a sidebar capture
                    // `ambient_navigator() == None` and their `on_activate`
                    // no-ops — taps register (a native button even plays its
                    // click sound) but no navigation dispatches. Held across
                    // `super::build` because the link's `on_activate` snapshots
                    // the ambient navigator as the walker builds it.
                    let _nav_guard = primitives::navigator::AmbientNavGuard::push(
                        control_for_chrome.clone(),
                    );
                    let prim = builder();
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
        resolve_entry,
        base: my_base.clone(),
        nav_state: nav_state.clone(),
        depth_changed,
        active_changed,
        control: control.clone(),
        build_node,
        build_node_scoped,
        build_node_into,
        build_in_screen,
    };

    let node = time_backend_create(pkind!(Navigator), || {
        backend
            .borrow_mut()
            .create_navigator(type_id, type_name, presentation, host, &accessibility)
    });

    // Centralize the post-navigation layout pass in the abstraction: register
    // the backend's scheduler once, here, so `NavigatorControl::dispatch`
    // guarantees a relayout after EVERY command — on every backend, for every
    // navigator kind — without each SDK handler having to remember to call it.
    // (Backends that auto-relayout, e.g. web reflow, default to a no-op.)
    control.install_request_layout(Box::new(|| B::schedule_layout_pass()));

    if !defer_initial_mount {
        // Native / SSR / headless initial mount with hierarchical deep-link
        // resolution. A launch / server-requested path may have been set
        // (iOS/Android cold-start deep link, or SSR render-at-path). Resolve
        // THIS navigator's slice of it via the PREFIX resolver — stripping our
        // base and picking the most-specific relative route — and mount THAT
        // screen as the initial instead of the hardcoded `initial`.
        //
        // PEEK, don't take: each navigator in this synchronous mount cascade
        // (a drawer whose screen nests a stack, etc.) independently consults the
        // SAME full URL and strips ITS OWN base via `resolve_entry`. Consuming
        // (the old `take`) would starve nested navigators. The root navigator
        // (base "") clears the slot once its whole subtree has mounted — see
        // below.
        //
        // The resolved screen is what `attach_initial` carries — the SSR /
        // primitive-chrome contract (it renders exactly the attached screen,
        // no navigation), and also the on-screen-top for live drawer/tab/stack.
        // STACK back-stack reconstruction (so Back returns to the index after a
        // cold deep-link) is the stack SDK handler's job: it sees
        // `host.initial_route` vs `host.nav_state.active_route` and, when they
        // differ, seats the configured initial BELOW the resolved screen. Only
        // the stack knows it's a stack, so that reconstruction can't live here.
        //
        // Live web backends never set the initial path; they read the platform
        // URL in the SDK handler layer (deferred mount).
        let (route, params) = primitives::navigator::peek_initial_path()
            .and_then(|path| {
                resolve_entry_for_initial(&path).map(|(name, params, _rem)| {
                    // Compose the matched screen's FULL hierarchical path
                    // (base + this navigator's matched relative pattern) so
                    // chrome reads the right route. The resolver gives us the
                    // route name; reconstruct the relative path from its pattern.
                    let rel = screens.get(name).map(|e| e.path).unwrap_or("");
                    nav_state.active_route.set(name);
                    nav_state.active_path.set(join_path(&my_base, rel));
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

        // Root navigator: the entire nested subtree has now mounted
        // synchronously (mounting in this path is synchronous), so every nested
        // navigator has already peeked the launch URL and stripped its base.
        // Clear the slot so a later rebuild / non-deep-link mount isn't poisoned.
        if my_base.is_empty() {
            primitives::navigator::set_initial_path(None);
        }
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

    // Keep nav chrome scopes AND the per-screen `scopes` map alive for the
    // navigator's lifetime — owned by the FRAMEWORK, not left to whether an
    // SDK handler happens to retain the `NavigatorHost`.
    //
    // The per-screen `scopes` map owns each mounted screen's reactive scope,
    // and those scopes own the screen content's robot-registry entries (via
    // the `on_cleanup(deregister)` the walker anchors per element). The map's
    // only other owners are the `Rc` clones captured by `mount_screen` /
    // `release_screen` / `build_in_screen` and moved into the `NavigatorHost`.
    // A handler that drops the host (e.g. the SSR primitive-chrome handler)
    // — or any backend that doesn't retain the control — lets the map's
    // refcount hit zero when `build` returns, dropping every screen scope and
    // wiping the current screen's elements from the robot registry, so
    // `Robot::snapshot()` shows a bare `Navigator` with no children. Retaining
    // the map here makes screen-scope lifetime deterministic across every
    // backend; `release_screen` still removes individual screens on pop, so
    // popped screens deregister at the right time (this also fixes the stale
    // "popped screen still in the registry" leak). Regression:
    // `stack-navigator/tests/robot_screen_tree` (run with `--features robot`).
    // The `control` itself must be retained for the same reason. It owns the
    // `nav_state` scope (`retain_scope` stashes it in `owning_scope`), so
    // `active_route`/`active_path`/`depth`/`can_go_back` live exactly as long
    // as `control`. But after `build` returns, the SDK handler's `init` has
    // already dropped the `NavigatorHost` it took by value (handlers don't
    // store it), so the ONLY strong ref left can be a transient clone an SDK
    // handler captured — e.g. the macOS drawer defers its sidebar build into a
    // `schedule_microtask` whose `control` clone is consumed when the builder
    // closure returns, BEFORE the walker builds the sidebar's reactive styles.
    // A sidebar that reads `active_route` reactively (a nav item's active-
    // highlight) but doesn't itself capture a `control`-bearing closure
    // (it uses framework `link(route=…)` rather than the slot's `on_select`)
    // then drops `control`'s last ref mid-build, freeing `active_route` out
    // from under the style effect → "signal used after its scope was dropped".
    // Anchoring `control` here makes nav-state lifetime deterministic across
    // every backend, independent of what a handler's sidebar happens to
    // capture. Regression: `reactive_nav_state_survives_handler_dropping_host`.
    let _chrome_keepalive = nav_chrome_scopes.clone();
    let _screen_scopes_keepalive = scopes.clone();
    let _control_keepalive = control.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_chrome_keepalive;
        let _ = &_screen_scopes_keepalive;
        let _ = &_control_keepalive;
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
