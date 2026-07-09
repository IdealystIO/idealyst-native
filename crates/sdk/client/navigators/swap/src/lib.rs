//! First-party **Swap** navigator SDK — a flat set of co-equal screens
//! the user switches between (`Select`), with **author-supplied chrome**.
//!
//! This replaces the separate `tab` and `drawer` navigators. There is no
//! push/pop depth: selecting a screen swaps the one visible screen. What
//! used to be a "tab bar" or a "drawer panel" is now just ordinary author
//! layout wrapped around the navigator's single **outlet** — the analog
//! of react-router's `<Outlet/>`:
//!
//! ```ignore
//! swap_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<SwapHandle> = Ref::new();
//!
//! SwapNavigator::new(&home)
//!     .screen(home.clone(), |_| Screen::new(/* … */))
//!     .screen(settings.clone(), |_| Screen::new(/* … */))
//!     // The layout OWNS the tree and splats `{nav.outlet}`. "Tab bar" =
//!     // wrap the outlet in a bar; "drawer" = wrap it in an idea-ui Drawer.
//!     .layout(|nav| ui! {
//!         Column {
//!             { nav.outlet }
//!             TabBar(active = nav.active_route, on_select = nav.on_select) { /* … */ }
//!         }
//!     })
//!     .bind(nav.clone());
//! ```
//!
//! # One backend-neutral handler
//!
//! Because chrome is author layout, the handler drives everything through
//! the framework's [`NavigatorHost`] callbacks
//! ([`build_layout_with_outlet`](NavigatorHost::build_layout_with_outlet),
//! [`mount_screen`](NavigatorHost::mount_screen),
//! [`insert_node`](NavigatorHost::insert_node),
//! [`clear_children`](NavigatorHost::clear_children)) plus
//! [`schedule_microtask`](runtime_core::schedule_microtask). So ONE
//! [`SwapHandler`] serves every backend — there are no per-backend twins to
//! drift apart (the bug that made the old tab navigator panic on web). The
//! per-backend modules below only submit the self-registration inventory
//! entry.
//!
//! Selecting a screen dispatches `NavCommand::Select`; a `Link` inside a
//! swap screen is rewritten to `Select` by the installed link activator
//! (so links switch, never push).

#![deny(missing_docs)]

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    navigator_outlet, MountResult, NavCommand, NavigatorConfig, NavigatorControl, NavigatorHandle,
    NavigatorHandler, NavigatorHost, NavigatorOps, Route, RouteEntry, RouteParams, Screen,
    ScreenBuilder, SwapContext,
};
use runtime_core::{Backend, Bound, Element, IdealystSchema, Ref, RefFill, StyleSource};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// Value types
// =============================================================================

/// When a swap screen's subtree is materialized and whether it survives a
/// switch away. The default ([`MountPolicy::LazyPersistent`]) matches the
/// React-Navigation tab default — mount on first visit, keep mounted.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum MountPolicy {
    /// Mount every screen at navigator creation; keep all mounted.
    EagerPersistent,
    /// Mount a screen on first activation; keep it mounted (detached, not
    /// disposed) across switches — the default.
    #[default]
    LazyPersistent,
    /// Mount a screen on first activation; drop its scope (and background
    /// work) when switched away — re-mounts fresh on return.
    LazyDisposing,
}

/// Per-swap-screen options. Empty today (swap screens draw no navigator
/// chrome of their own), kept as a named type so per-screen metadata can
/// be added without an API break.
#[derive(Default, Clone, IdealystSchema)]
pub struct SwapScreenOptions {}

impl SwapScreenOptions {
    /// Empty options (`Default`).
    pub fn new() -> Self {
        Self::default()
    }
}

/// The author `.layout(|nav| …)` closure — receives the [`SwapContext`]
/// (outlet + reactive nav state + `on_select`) and returns the chrome tree.
type LayoutBuilder = Rc<dyn Fn(SwapContext) -> Element>;

// =============================================================================
// SwapPresentation — SDK's typed payload
// =============================================================================

/// The SDK's typed payload that rides on the `Element::Navigator` produced
/// by [`SwapNavigator::new`]. Its `TypeId` is the registry key the
/// backend-neutral [`SwapHandler`] is registered under (see [`register`]).
pub struct SwapPresentation {
    /// The author layout closure (`None` ⇒ the outlet fills the navigator
    /// with no surrounding chrome).
    pub layout: Option<LayoutBuilder>,
    /// Screen mount lifecycle — see [`MountPolicy`].
    pub mount_policy: MountPolicy,
}

impl Default for SwapPresentation {
    fn default() -> Self {
        Self { layout: None, mount_policy: MountPolicy::default() }
    }
}

// =============================================================================
// SwapHandle — typed handle for `.bind(...)`
// =============================================================================

/// Typed runtime handle to a live swap navigator, filled into the [`Ref`]
/// passed to [`SwapBuilder::bind`]. Use it to switch screens with typed
/// params. Cheap to clone.
#[derive(Clone)]
pub struct SwapHandle {
    inner: NavigatorHandle,
}

impl SwapHandle {
    /// Wrap a raw [`NavigatorHandle`] in the typed handle. Called by the
    /// backend `register` glue; authors get one from [`SwapBuilder::bind`].
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Switch to `route`, building its URL from typed `params`. Selecting
    /// the already-active screen is a no-op at the handler.
    pub fn select<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Select {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Borrow the underlying kind-agnostic [`NavigatorHandle`].
    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

struct SwapOps;
impl NavigatorOps for SwapOps {}
static SWAP_OPS: SwapOps = SwapOps;

// =============================================================================
// Builder
// =============================================================================

/// The swap-navigator builder. [`SwapNavigator::new`] starts one; the
/// fluent methods on the [`SwapBuilder`] trait register screens, set the
/// author layout, and bind the `Ref`. The result is a [`Bound<SwapHandle>`]
/// you drop into a `ui!` tree.
pub struct SwapNavigator {
    config: NavigatorConfig,
    presentation: SwapPresentation,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl SwapNavigator {
    /// Start a swap navigator whose initial (selected) screen is `initial`.
    pub fn new(initial: &Route<()>) -> Bound<SwapHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: SwapPresentation::default(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_element())
    }

    fn into_element(self) -> Element {
        let SwapNavigator { config, presentation, style, ref_fill } = self;
        Element::Navigator {
            type_id: TypeId::of::<SwapPresentation>(),
            type_name: std::any::type_name::<SwapPresentation>(),
            presentation: Rc::new(presentation) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles: Vec::new(),
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

fn with_navigator_prim<F: FnOnce(&mut Element)>(b: &mut Bound<SwapHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation_mut<F: FnOnce(&mut SwapPresentation)>(b: &mut Bound<SwapHandle>, f: F) {
    if let Element::Navigator { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("swap-navigator: presentation Rc already shared (builder misuse)");
        if let Some(typed) = (pres as &mut dyn Any).downcast_mut::<SwapPresentation>() {
            f(typed);
        }
    }
}

/// Fluent builder methods for the swap navigator, implemented on
/// [`Bound<SwapHandle>`]. A trait (not inherent methods) because `Bound`
/// lives in `runtime-core` — the app `use`s the trait to gain the methods.
pub trait SwapBuilder: Sized {
    /// Register a screen: its route and the closure that builds the screen
    /// from typed params.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set the author layout — the closure owns the chrome tree and splats
    /// `{nav.outlet}` where the active screen renders.
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static;
    /// Set the screen mount lifecycle — see [`MountPolicy`].
    fn mount_policy(self, policy: MountPolicy) -> Self;
    /// Bind a [`Ref<SwapHandle>`] so the app can switch screens imperatively.
    fn bind(self, r: Ref<SwapHandle>) -> Self;
}

impl SwapBuilder for Bound<SwapHandle> {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        let route_name = route.name();
        let route_path = route.path();
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { config, .. } = p {
                let builder: ScreenBuilder = Rc::new(move |any_params: Box<dyn Any>| {
                    let typed: Box<P> = any_params
                        .downcast::<P>()
                        .expect("swap-navigator: route params type mismatch");
                    render(*typed).into()
                });
                let from_segments = Rc::new(
                    |segs: &HashMap<String, String>| -> Option<Box<dyn Any>> {
                        P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
                    },
                );
                config.screens.insert(
                    route_name,
                    RouteEntry { path: route_path, build: builder, from_segments },
                );
            }
        });
        self
    }

    fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static,
    {
        with_presentation_mut(&mut self, |p| p.layout = Some(Rc::new(f)));
        self
    }

    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        with_presentation_mut(&mut self, |p| p.mount_policy = policy);
        self
    }

    fn bind(mut self, r: Ref<SwapHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(SwapHandle::from_inner(handle));
                })));
            }
        });
        self
    }
}

// =============================================================================
// The backend-neutral handler
// =============================================================================

/// Shared per-navigator state, held behind an `Rc` so the deferred
/// layout-build microtask, the `Select` dispatcher installed on the control
/// plane, and `attach_initial` all coordinate on the same cells.
struct SwapShared<B: Backend> {
    /// The active-screen outlet, captured from the author layout in the
    /// deferred build microtask. `None` until then.
    outlet: RefCell<Option<B::Node>>,
    /// Mounted screens by route: `(node, scope_id)`. Persistent policies
    /// keep entries across switches; `LazyDisposing` releases on switch-away.
    mounted: RefCell<HashMap<&'static str, (B::Node, u64)>>,
    /// Currently shown route.
    active: RefCell<Option<&'static str>>,
    /// Initial screen the framework mounted (`defer_initial_mount = false`),
    /// stashed until the outlet exists.
    pending_initial: RefCell<Option<(B::Node, u64)>>,
    /// The initial route name (for caching the stashed initial screen).
    initial_route: &'static str,
    mount_policy: MountPolicy,
    mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<B::Node>>,
    insert_node: Rc<dyn Fn(B::Node, B::Node)>,
    clear_children: Rc<dyn Fn(B::Node)>,
    release_screen: Rc<dyn Fn(u64)>,
}

impl<B: Backend> SwapShared<B> {
    /// Insert `node` as the outlet's sole child (clearing the prior screen's
    /// node — its scope survives in `mounted` for persistent policies).
    fn show_in_outlet(&self, node: B::Node) {
        if let Some(outlet) = self.outlet.borrow().clone() {
            (self.clear_children)(outlet.clone());
            (self.insert_node)(outlet, node);
        }
    }

    /// Resolve the screen for `name` (reuse the cached node for persistent
    /// policies, else mount fresh) and show it. `LazyDisposing` releases the
    /// previously-active screen's scope first.
    fn select(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>) {
        if self.active.borrow().as_deref() == Some(name) {
            return; // already active — no-op
        }

        if self.mount_policy == MountPolicy::LazyDisposing {
            if let Some(prev) = *self.active.borrow() {
                if let Some((_, sid)) = self.mounted.borrow_mut().remove(prev) {
                    (self.release_screen)(sid);
                }
            }
        }

        let node = if let Some((n, _)) = self.mounted.borrow().get(name) {
            n.clone()
        } else {
            let r = (self.mount_screen)(name, params, state);
            self.mounted.borrow_mut().insert(name, (r.node.clone(), r.scope_id));
            r.node
        };
        self.show_in_outlet(node);
        *self.active.borrow_mut() = Some(name);
    }
}

/// The one navigator handler for every backend. Builds the author layout
/// (deferred past the `init` borrow), captures the outlet, and swaps the
/// active screen into it on `Select`.
pub struct SwapHandler<B: Backend> {
    control: Option<Rc<NavigatorControl>>,
    shared: Option<Rc<SwapShared<B>>>,
}

impl<B: Backend> SwapHandler<B> {
    /// A fresh, uninitialized handler. `init` wires the rest.
    pub fn new() -> Self {
        Self { control: None, shared: None }
    }
}

impl<B: Backend> Default for SwapHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the `Link`→`Select` activator: a `Link` inside a swap screen
/// switches (not pushes). Wired once here in the shared handler, so it can
/// never drift out of sync per backend (the old tab bug).
fn install_select_link_activator(control: &Rc<NavigatorControl>) {
    let activator: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand> =
        Rc::new(|name, url, params| NavCommand::Select { name, url, params, state: None });
    control.install_link_activator(activator);
}

impl<B: Backend + 'static> NavigatorHandler<B> for SwapHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        let a11y = AccessibilityProps::default();
        // Bare root container returned synchronously; the author layout is
        // spliced in by the deferred microtask (it re-borrows the backend,
        // so it can't run inside this `init` borrow).
        let root = backend.create_view(&a11y);

        let NavigatorHost {
            initial_route,
            mount_screen,
            release_screen,
            insert_node,
            clear_children,
            control,
            build_layout_with_outlet,
            nav_state,
            ..
        } = host;

        let (layout, mount_policy) = presentation
            .downcast_ref::<SwapPresentation>()
            .map(|p| (p.layout.clone(), p.mount_policy))
            .unwrap_or((None, MountPolicy::default()));

        install_select_link_activator(&control);

        let shared = Rc::new(SwapShared {
            outlet: RefCell::new(None),
            mounted: RefCell::new(HashMap::new()),
            active: RefCell::new(None),
            pending_initial: RefCell::new(None),
            initial_route,
            mount_policy,
            mount_screen,
            insert_node,
            clear_children,
            release_screen,
        });

        // Select dispatcher — the substrate has already updated
        // `active_route`/`active_path` by the time this runs; we mount/reuse
        // the screen and swap the outlet.
        control.install({
            let shared = shared.clone();
            Box::new(move |cmd| match cmd {
                NavCommand::Select { name, params, state, .. } => {
                    shared.select(name, params, state);
                }
                // Swap navigators have no stack; a stray push/pop/replace is
                // an author error routed here only if a `Link` bypassed the
                // Select activator. Ignore rather than panic (the old tab
                // handler panicked — the regression we're fixing).
                _ => {}
            })
        });

        // Defer the author-layout build past this borrow. It captures the
        // outlet, splices the chrome into `root`, and shows the initial
        // screen the framework stashed via `attach_initial`.
        {
            let shared = shared.clone();
            let root = root.clone();
            let control_for_ctx = control.clone();
            let active_route = nav_state.active_route;
            let active_path = nav_state.active_path;
            runtime_core::schedule_microtask(move || {
                let on_select: Rc<dyn Fn(&'static str)> = {
                    let control = control_for_ctx.clone();
                    Rc::new(move |name| {
                        control.dispatch(NavCommand::Select {
                            name,
                            url: String::new(),
                            params: Box::new(()),
                            state: None,
                        });
                    })
                };
                let ctx = SwapContext {
                    outlet: navigator_outlet(),
                    active_route,
                    active_path,
                    on_select,
                };
                // Build the author chrome (or a bare outlet if no layout was
                // set), capturing the outlet node.
                let layout_elem = match &layout {
                    Some(f) => f(ctx),
                    None => ctx.outlet,
                };
                let (layout_root, outlet) = (build_layout_with_outlet)(layout_elem);
                debug_assert!(
                    outlet.is_some(),
                    "swap-navigator: the author `.layout(...)` must splat `{{nav.outlet}}` \
                     exactly once — no outlet was found in the built layout"
                );
                (shared.insert_node)(root, layout_root);
                *shared.outlet.borrow_mut() = outlet;

                // Show the framework-mounted initial screen.
                let pending: Option<(B::Node, u64)> = shared.pending_initial.borrow_mut().take();
                if let Some((node, sid)) = pending {
                    shared
                        .mounted
                        .borrow_mut()
                        .insert(shared.initial_route, (node.clone(), sid));
                    shared.show_in_outlet(node);
                    *shared.active.borrow_mut() = Some(shared.initial_route);
                } else {
                    // Defensive: no stashed initial (e.g. deferred-mount host) —
                    // mount the configured initial ourselves.
                    shared.select(shared.initial_route, Box::new(()), None);
                }
            });
        }

        self.control = Some(control);
        self.shared = Some(shared);
        root
    }

    fn attach_initial(
        &mut self,
        _backend: &mut B,
        screen: B::Node,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        // The outlet isn't built yet (it comes from the deferred layout
        // microtask), so stash the framework-mounted initial screen; the
        // microtask inserts it once the outlet exists.
        if let Some(shared) = &self.shared {
            if let Some(outlet) = shared.outlet.borrow().clone() {
                // Rare: outlet already built (microtask ran first) — insert now.
                (shared.clear_children)(outlet.clone());
                (shared.insert_node)(outlet, screen.clone());
                shared
                    .mounted
                    .borrow_mut()
                    .insert(shared.initial_route, (screen, scope_id));
                *shared.active.borrow_mut() = Some(shared.initial_route);
            } else {
                *shared.pending_initial.borrow_mut() = Some((screen, scope_id));
            }
        }
    }

    fn make_handle(&self) -> NavigatorHandle {
        match &self.control {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &SWAP_OPS, c.clone()),
            None => NavigatorHandle::new(Rc::new(()), &SWAP_OPS),
        }
    }
}

// =============================================================================
// Per-backend registration (concrete `register` + self-registration inventory)
// =============================================================================
//
// The one generic `SwapHandler<B>` serves every backend, but the real
// backends expose `register_navigator` as an *inherent* method (only the
// SSR backend implements the `RegisterNavigator` trait), so each platform
// gets a concrete `register` fn. `pub use` selects the right one per target.

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{SwapHandler, SwapPresentation};
    use backend_web::WebBackend;
    /// Register the swap handler on the web backend.
    pub fn register(backend: &mut WebBackend) {
        backend
            .register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<WebBackend>::new()));
    }
    inventory::submit! { backend_web::WebNavigatorRegistrar(register) }
}
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos {
    use super::{SwapHandler, SwapPresentation};
    use backend_macos::MacosBackend;
    /// Register the swap handler on the macOS backend.
    pub fn register(backend: &mut MacosBackend) {
        backend.register_navigator::<SwapPresentation, _>(|| {
            Box::new(SwapHandler::<MacosBackend>::new())
        });
    }
    inventory::submit! { backend_macos::MacosNavigatorRegistrar(register) }
}
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

#[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
mod fallback {
    use runtime_core::Backend;
    /// No-op swap registration for backends without a dedicated registrar,
    /// so host bootstrap can call `register` unconditionally. Backends with a
    /// generic registration path (SSR/tests) register [`super::SwapHandler`]
    /// directly via their own `register_navigator`.
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
pub use fallback::register;

// =============================================================================
// Prelude
// =============================================================================

/// Convenience re-exports — glob-import to bring the builder, handle,
/// screen options, and value types into scope.
pub mod prelude {
    pub use super::{
        register, MountPolicy, SwapBuilder, SwapHandle, SwapNavigator, SwapPresentation,
        SwapScreenOptions,
    };
}
