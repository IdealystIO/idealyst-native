//! The old-core (walker/`Element::Navigator`) implementation — the
//! entire pre-P6 crate body, byte-moved. Compiled unless the `new-core`
//! feature selects the vocabulary-backed surface in `newcore.rs` (the
//! two are mutually exclusive because they define the same public
//! names — one core per build, the same contract as the macro
//! lowering switch).

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    navigator_fill_rules, navigator_outlet, MountResult, NavCommand, NavigatorConfig,
    NavigatorControl, NavigatorHandle, NavigatorHandler, NavigatorHost, NavigatorOps, Route,
    RouteEntry, RouteParams, Screen, ScreenBuilder, SwapContext,
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

/// Builds a route's `(url, typed_params)` for a bare-name selection
/// ([`SwapContext::on_select`]). `None` when the route's params can't be
/// constructed without path segments — such routes need
/// [`SwapHandle::select`] with typed params.
pub type SelectArgs = Rc<dyn Fn() -> Option<(String, Box<dyn Any>)>>;

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
    /// Per-route [`SelectArgs`] builders, recorded by [`SwapBuilder::screen`].
    /// `SwapContext::on_select` gets only a route NAME from chrome (a tab
    /// bar), but `NavCommand::Select` carries the url the substrate writes
    /// into `nav_state.active_path` and the typed params the screen builder
    /// downcasts — dispatching placeholder `""`/`()` values corrupted the
    /// path mirror and panicked on non-unit-params routes.
    pub select_args: HashMap<&'static str, SelectArgs>,
}

impl Default for SwapPresentation {
    fn default() -> Self {
        Self {
            layout: None,
            mount_policy: MountPolicy::default(),
            select_args: HashMap::new(),
        }
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
        // Force-link the platform's registration module so its
        // `inventory::submit!` survives DEV-profile codegen-unit DCE. In dev
        // builds (high codegen-units, no LTO) an unreferenced module's `#[used]`
        // submit is dropped with its object; release (codegen-units=1 + LTO)
        // keeps it. A `black_box`'d reference to `register` — in a code path the
        // app runs when it builds the navigator — pulls the module's object into
        // the link, so registration works in BOTH profiles on every backend
        // (fixes `dev` panicking "SwapPresentation is not registered"). Zero cost.
        #[cfg(target_arch = "wasm32")]
        let _ = core::hint::black_box(web::register as *const ());
        #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(macos::register as *const ());
        #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(ios::register as *const ());
        #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(android::register as *const ());

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
        // Record the bare-name select recipe for `SwapContext::on_select`:
        // url from the route's pattern + params from an empty segment map —
        // `Some` only when `P` is constructible without path segments (`()`,
        // all-optional params). Mirrors `SwapHandle::select`'s url building.
        with_presentation_mut(&mut self, |p| {
            p.select_args.insert(
                route_name,
                Rc::new(move || {
                    P::from_segments(&HashMap::new())
                        .map(|params| (params.to_path(route_path), Box::new(params) as Box<dyn Any>))
                }),
            );
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
    /// Mounted screens keyed by their normalized URL — NOT the route
    /// name. A parameterized route (`/entry/:name`) funnels many
    /// distinct screens through one route name; name-keying made
    /// same-route selects no-ops and served entry A's cached screen for
    /// entry B (the docs-app catalog bug). Persistent policies keep
    /// entries across switches; `LazyDisposing` releases on switch-away.
    mounted: RefCell<HashMap<String, (B::Node, u64)>>,
    /// Currently shown `(route name, normalized url)`.
    active: RefCell<Option<(&'static str, String)>>,
    /// Initial screen the framework mounted (`defer_initial_mount = false`),
    /// stashed until the outlet exists. The `&'static str` is the route the
    /// walker actually RESOLVED for it (a cold-start deep link may resolve a
    /// route other than the configured initial) — captured at attach time so
    /// the screen is cached under the right key.
    pending_initial: RefCell<Option<(&'static str, String, B::Node, u64)>>,
    /// The configured initial route name (the fallback when nothing was
    /// attached — deferred-mount hosts).
    initial_route: &'static str,
    /// The walker's reactive active-route/path mirrors. Read UNTRACKED at
    /// attach time to learn the deep-link-resolved initial route + URL: the
    /// walker sets them BEFORE `mount_screen`/`attach_initial` on the
    /// non-deferred cold-start path (see `walker::navigator`).
    active_route: runtime_core::Signal<&'static str>,
    active_path: runtime_core::Signal<String>,
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

    /// The `(route, url)` the framework actually mounted as the initial
    /// screen: the configured initial, unless a cold-start deep link
    /// resolved elsewhere (the walker writes the resolved route/path into
    /// the mirrors before attaching). Untracked — called from
    /// non-reactive handler paths.
    fn resolved_initial(&self) -> (&'static str, String) {
        runtime_core::untrack(|| (self.active_route.get(), self.active_path.get()))
    }

    /// Trailing-slash-tolerant URL key (`/docs` == `/docs/`), matching
    /// the substrate URL layer's `paths_equal` tolerance.
    fn url_key(url: &str) -> String {
        let t = url.trim_end_matches('/');
        if t.is_empty() { "/".to_string() } else { t.to_string() }
    }

    /// Cache the framework-attached initial screen under its URL and show it.
    fn seat_initial(&self, route: &'static str, url: String, node: B::Node, scope_id: u64) {
        let key = Self::url_key(&url);
        self.mounted
            .borrow_mut()
            .insert(key.clone(), (node.clone(), scope_id));
        self.show_in_outlet(node);
        *self.active.borrow_mut() = Some((route, key));
    }

    /// Resolve the screen for `name` (reuse the cached node for persistent
    /// policies, else mount fresh) and show it. `LazyDisposing` releases the
    /// previously-active screen's scope first.
    fn select(
        &self,
        name: &'static str,
        url: &str,
        params: Box<dyn Any>,
        state: Option<Rc<dyn Any>>,
    ) {
        let key = Self::url_key(url);
        // Already showing this exact URL — no-op. Comparing the URL (not
        // just the route name) is what makes parameterized routes work:
        // `/entry/button` → `/entry/card` shares one route name but MUST
        // swap screens.
        if self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|(_, active_url)| *active_url == key)
        {
            return;
        }

        if self.mount_policy == MountPolicy::LazyDisposing {
            // Copy the key out and END both borrows before `release_screen`:
            // it drops the screen's scope synchronously, running author
            // `on_cleanup` callbacks that may navigate (re-entering this
            // navigator's cells). An if-let on the `borrow_mut()` scrutinee
            // holds the guard through the body — "RefCell already borrowed"
            // (same class as the cache-lookup snapshot below).
            let prev = self.active.borrow().as_ref().map(|(_, u)| u.clone());
            let released = prev
                .and_then(|p| self.mounted.borrow_mut().remove(&p))
                .map(|(_, sid)| sid);
            if let Some(sid) = released {
                (self.release_screen)(sid);
            }
        }

        // Snapshot the lookup so the `borrow()` releases before the miss path
        // takes a `borrow_mut()` (else: "RefCell already borrowed").
        let cached = self.mounted.borrow().get(&key).map(|(n, _)| n.clone());
        let node = if let Some(n) = cached {
            n
        } else {
            let r = (self.mount_screen)(name, params, state);
            self.mounted
                .borrow_mut()
                .insert(key.clone(), (r.node.clone(), r.scope_id));
            r.node
        };
        self.show_in_outlet(node);
        *self.active.borrow_mut() = Some((name, key));
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
        // Fill-the-container default (see `navigator_fill_rules`) — a bare
        // root hugs content, collapsing a viewport-height app. The author's
        // `.with_style(...)` on the navigator element is applied by the
        // walker AFTER init, so it overrides this.
        backend.apply_style(&root, &navigator_fill_rules());

        let NavigatorHost {
            initial_route,
            mount_screen,
            release_screen,
            insert_node,
            clear_children,
            get_node_scroll,
            set_node_scroll,
            control,
            build_layout_with_outlet,
            nav_state,
            ..
        } = host;

        let (layout, mount_policy, select_args) = presentation
            .downcast_ref::<SwapPresentation>()
            .map(|p| (p.layout.clone(), p.mount_policy, p.select_args.clone()))
            .unwrap_or((None, MountPolicy::default(), HashMap::new()));

        install_select_link_activator(&control);

        // Opt into substrate URL sync: on web, `Select`s mirror into
        // browser history (pushState) and back/forward popstates come
        // back as ordinary `Select` dispatches; deep links + scroll
        // restore ride along. No-op on URL-less platforms — the handler
        // itself never touches a URL (backend-neutral by design).
        control.enable_url_sync();

        let shared = Rc::new(SwapShared {
            outlet: RefCell::new(None),
            mounted: RefCell::new(HashMap::new()),
            active: RefCell::new(None),
            pending_initial: RefCell::new(None),
            initial_route,
            active_route: nav_state.active_route,
            active_path: nav_state.active_path,
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
                NavCommand::Select { name, url, params, state } => {
                    shared.select(name, &url, params, state);
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
                        // Build the route's REAL url + typed params (recorded
                        // by `SwapBuilder::screen`) — the substrate writes the
                        // command url into `nav_state.active_path`, and the
                        // screen builder downcasts the params box, so
                        // placeholder `""`/`()` values corrupt the path mirror
                        // and panic on non-unit-params routes. `None` ⇒ the
                        // route is unregistered or needs path params a bare
                        // name can't supply — ignore rather than panic (chrome
                        // taps must not crash; use `SwapHandle::select` for
                        // typed-param routes).
                        let Some((url, params)) =
                            select_args.get(name).and_then(|build| build())
                        else {
                            return;
                        };
                        control.dispatch(NavCommand::Select { name, url, params, state: None });
                    })
                };
                let ctx = SwapContext {
                    outlet: navigator_outlet(),
                    active_route,
                    active_path,
                    on_select,
                };
                // Build the author chrome (or a bare outlet if no layout was
                // set), capturing the outlet node. The producer closure runs
                // inside the framework's retained nav-chrome scope, so an
                // `effect!` in author chrome is owned by the navigator.
                let (layout_root, outlet) =
                    (build_layout_with_outlet)(Box::new(move || match &layout {
                        Some(f) => f(ctx),
                        None => ctx.outlet,
                    }));
                debug_assert!(
                    outlet.is_some(),
                    "swap-navigator: the author `.layout(...)` must splat `{{nav.outlet}}` \
                     exactly once — no outlet was found in the built layout"
                );
                (shared.insert_node)(root, layout_root);
                *shared.outlet.borrow_mut() = outlet;

                // Wire the outlet's scroll into the substrate URL sync so
                // browser back restores the position the user left a
                // screen at (no-op when the outlet isn't a scroll
                // surface, or off web).
                {
                    let get = {
                        let shared = shared.clone();
                        let get_node_scroll = get_node_scroll.clone();
                        Rc::new(move || {
                            shared
                                .outlet
                                .borrow()
                                .clone()
                                .map(|o| get_node_scroll(o))
                                .unwrap_or((0.0, 0.0))
                        })
                    };
                    let set = {
                        let shared = shared.clone();
                        let set_node_scroll = set_node_scroll.clone();
                        Rc::new(move |x, y| {
                            if let Some(o) = shared.outlet.borrow().clone() {
                                set_node_scroll(o, x, y);
                            }
                        })
                    };
                    control_for_ctx.install_scroll_accessor(get, set);
                }

                // Show the framework-mounted initial screen, cached under the
                // route the walker RESOLVED for it (deep link aware).
                let pending: Option<(&'static str, String, B::Node, u64)> =
                    shared.pending_initial.borrow_mut().take();
                if let Some((route, path, node, sid)) = pending {
                    shared.seat_initial(route, path, node, sid);
                } else {
                    // Defensive: no stashed initial (e.g. deferred-mount host) —
                    // mount the configured initial ourselves. The configured
                    // initial's path IS its url (unit-params root route).
                    let (_, path) = shared.resolved_initial();
                    shared.select(shared.initial_route, &path, Box::new(()), None);
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
        //
        // Key it under the route the walker RESOLVED (read from the
        // active-route mirror, which the walker set just before mounting) —
        // NOT the configured initial. On a cold-start deep link the two
        // differ, and caching e.g. the "/settings" screen under "home" shows
        // the wrong screen on every later selection of either route.
        if let Some(shared) = &self.shared {
            let (route, path) = shared.resolved_initial();
            let outlet = shared.outlet.borrow().clone();
            if outlet.is_some() {
                // Rare: outlet already built (microtask ran first) — seat now.
                shared.seat_initial(route, path, screen, scope_id);
            } else {
                *shared.pending_initial.borrow_mut() = Some((route, path, screen, scope_id));
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

// iOS / Android: the SAME backend-neutral handler. The mobile backends key
// their handler map by the node itself (not a stamped attribute like web), so
// dispatch + the bound handle wire up without any extra work — a swap navigator
// there is an outlet swap on `Select`, chrome is author layout, exactly as
// elsewhere. (A native tab-controller surface is intentionally NOT used —
// swap needs no native surface; see the crate docs.)
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios {
    use super::{SwapHandler, SwapPresentation};
    use backend_ios::IosBackend;
    /// Register the swap handler on the iOS backend.
    pub fn register(backend: &mut IosBackend) {
        backend
            .register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<IosBackend>::new()));
    }
    inventory::submit! { backend_ios::IosNavigatorRegistrar(register) }
}
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android {
    use super::{SwapHandler, SwapPresentation};
    use backend_android::AndroidBackend;
    /// Register the swap handler on the Android backend.
    pub fn register(backend: &mut AndroidBackend) {
        backend.register_navigator::<SwapPresentation, _>(|| {
            Box::new(SwapHandler::<AndroidBackend>::new())
        });
    }
    inventory::submit! { backend_android::AndroidNavigatorRegistrar(register) }
}
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
mod fallback {
    use runtime_core::Backend;
    /// No-op swap registration for backends without a dedicated registrar,
    /// so host bootstrap can call `register` unconditionally. Backends with a
    /// generic registration path (SSR/tests) register [`super::SwapHandler`]
    /// directly via their own `register_navigator`.
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
pub use fallback::register;

/// Runtime-server (wire recorder) registration. The outlet model needs
/// NO kind-specific wire commands: the handler drives its screen swaps
/// through the backend-erased `insert_node`/`clear_children`, which the
/// recorder ships as ordinary node ops the dev-client replays like any
/// other subtree change. Registering the SAME backend-neutral handler on
/// the recorder is the entire runtime-server story — contrast the legacy
/// per-kind recording handlers + `CreateTabNavigator`-style ops.
#[cfg(feature = "runtime-server")]
pub mod recording {
    use super::{SwapHandler, SwapPresentation};
    use dev_server::WireRecordingBackend;

    /// Register the swap handler on the runtime-server recorder. Call
    /// from the sidecar bootstrap alongside the other recorder
    /// registrations.
    pub fn register(backend: &mut WireRecordingBackend) {
        backend.register_navigator::<SwapPresentation, _>(|| {
            Box::new(SwapHandler::<WireRecordingBackend>::new())
        });
    }
}

/// Register the swap handler on any backend exposing the GENERIC
/// registry trait — the SSR backend today (`backend_ssr::render_path_with`
/// callers), test backends via their inherent registries. The SAME
/// backend-neutral [`SwapHandler`] as everywhere: SSR renders the author
/// layout + the walker-resolved screen in the outlet, so a server-rendered
/// page carries the real navigation chrome for first paint + crawlers.
pub fn register_generic<B: runtime_core::primitives::navigator::RegisterNavigator>(
    backend: &mut B,
) {
    backend.register_navigator::<SwapPresentation, _>(|| Box::new(SwapHandler::<B>::new()));
}

// =============================================================================
// Prelude
// =============================================================================

/// Convenience re-exports — glob-import to bring the builder, handle,
/// screen options, and value types into scope. Also exports the shared
/// data surface (`Route`, `Screen`, `SwapContext`) so a same-source app
/// can import everything from here on either core (the `new-core`
/// prelude exports the same names).
pub mod prelude {
    pub use super::{
        register, MountPolicy, SwapBuilder, SwapHandle, SwapNavigator, SwapPresentation,
        SwapScreenOptions,
    };
    pub use runtime_core::primitives::navigator::{Route, Screen, SwapContext};
}
