//! Stack navigator on the **outlet model** — the push/pop sibling of
//! `swap-navigator`.
//!
//! A stack has depth: `push` mounts a screen on top of a back-stack, `pop`
//! removes the top and reveals the one below (whose scope stayed alive, so its
//! state is intact). The visible screen is the top of the stack, swapped into
//! the navigator's single outlet.
//!
//! Chrome is **author layout**: `.layout(|nav| …)` wraps `{nav.outlet}` and can
//! read `nav.active_route` / `nav.can_go_back` / `nav.depth` and call `nav.pop`
//! — e.g. an `idea_ui_nav::StackHeader`. The author derives the title from the
//! active route. This mirrors `swap-navigator`; the only difference is the
//! command vocabulary (Push/Pop/Replace/Reset vs Select) and that lower screens
//! stay mounted beneath the top.
//!
//! ```ignore
//! StackNavigator::new(&HOME)
//!     .screen(HOME, |_| Screen::new(/* … */))
//!     .screen(DETAIL, |p: DetailParams| Screen::new(/* … */))
//!     .layout(|nav| ui! {
//!         view {
//!             StackHeader(
//!                 title = rx!(title_for(nav.active_route.get())),
//!                 show_back = nav.can_go_back,
//!                 on_back = nav.pop.clone(),
//!             )
//!             { nav.outlet }
//!         }
//!     })
//!     .bind(nav);
//! ```
//!
//! # Sizing
//!
//! The navigator's root **fills its container by default** (width/height
//! 100% + `flex-grow: 1` — see `navigator_fill_rules` in `runtime-core`).
//! Override by styling the navigator element itself:
//! `StackNavigator::new(&home)…​.with_style(my_style)`.

#![deny(missing_docs)]

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    navigator_fill_rules, navigator_outlet, HeaderButton, MountResult, NavCommand,
    NavigatorConfig, NavigatorControl, NavigatorHandle, NavigatorHandler, NavigatorHost,
    NavigatorOps, Route, RouteEntry, RouteParams, Screen, ScreenBuilder, StackHeaderState,
};
use runtime_core::{Backend, Bound, Element, Ref, RefFill, Signal, StyleSource};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// =============================================================================
// StackContext — handed to the author `.layout(|nav| …)` closure
// =============================================================================

/// The value a stack navigator hands its `.layout(|nav| …)` closure. Splat
/// [`outlet`](Self::outlet) where the active screen renders; read the reactive
/// nav state to drive a header (title from `active_route`, back arrow gated on
/// `can_go_back`) and call [`pop`](Self::pop) to go back.
///
/// The signals are the walker's scoped nav-state mirrors — reactive, and freed
/// with the navigator. Not `Clone` (the `outlet` is a one-shot value).
pub struct StackContext {
    /// Splat into the layout (`{nav.outlet}`) where the top screen mounts.
    pub outlet: Element,
    /// The active (top) screen's route name.
    pub active_route: Signal<&'static str>,
    /// The active screen's full path.
    pub active_path: Signal<String>,
    /// Stack depth (1 at the root).
    pub depth: Signal<usize>,
    /// Whether a `pop` is possible (depth > 1) — gate the back affordance on it.
    pub can_go_back: Signal<bool>,
    /// Pop the top screen (no-op at the root).
    pub pop: Rc<dyn Fn()>,
    /// The active screen's header slots ([`StackHeaderState`] inside an
    /// `Rc<dyn Any>`), updated on every navigation. Read it via
    /// [`header_state`] and feed the result to an `idea_ui_nav::StackHeader`
    /// (web/desktop). On mobile the native bar renders it and this stays
    /// available for parity.
    pub screen_chrome: Signal<Option<Rc<dyn std::any::Any>>>,
}

/// Read the current [`StackHeaderState`] out of a [`StackContext::screen_chrome`]
/// signal. Reactive: call it inside `rx!` / a component read so the header
/// re-renders when the active screen (hence its slots) changes.
pub fn header_state(screen_chrome: &Signal<Option<Rc<dyn std::any::Any>>>) -> Option<StackHeaderState> {
    screen_chrome
        .get()
        .and_then(|any| any.downcast_ref::<StackHeaderState>().cloned())
}

// =============================================================================
// Per-screen header slots
// =============================================================================

/// Per-screen header options for a stack navigator: the title and optional
/// leading/trailing [`HeaderButton`] slots, plus a `hide_header` toggle. Set
/// them via [`StackScreenExt`] on the `Screen` a `.screen(...)` builder returns.
/// The active screen's options drive the native bar (mobile) and are surfaced
/// to the author `StackHeader` (web/desktop) via [`StackContext::screen_chrome`].
#[derive(Clone, Default)]
pub struct StackScreenOptions {
    /// The screen title.
    pub title: Option<String>,
    /// Leading header slot.
    pub header_left: Option<HeaderButton>,
    /// Trailing header slot.
    pub header_right: Option<HeaderButton>,
    /// Hide the header entirely for this screen.
    pub hide_header: bool,
}

impl StackScreenOptions {
    fn to_state(&self, native: bool) -> StackHeaderState {
        StackHeaderState {
            title: self.title.clone().unwrap_or_default(),
            left: self.header_left.clone(),
            right: self.header_right.clone(),
            hidden: self.hide_header,
            native,
        }
    }
}

/// Fluent per-screen header setters on the `Screen` a `.screen(...)` render
/// closure returns: `Screen::new(...).title("Detail").header_right(btn)`.
pub trait StackScreenExt {
    /// Set the screen title (native bar title on mobile).
    fn title(self, t: impl Into<String>) -> Self;
    /// Set the leading header slot.
    fn header_left(self, btn: HeaderButton) -> Self;
    /// Set the trailing header slot.
    fn header_right(self, btn: HeaderButton) -> Self;
    /// Hide the header for this screen.
    fn hide_header(self, hide: bool) -> Self;
}

fn with_stack_options<F: FnOnce(&mut StackScreenOptions)>(screen: Screen, f: F) -> Screen {
    let mut opts = screen
        .options_as::<StackScreenOptions>()
        .cloned()
        .unwrap_or_default();
    f(&mut opts);
    screen.with(opts)
}

impl StackScreenExt for Screen {
    fn title(self, t: impl Into<String>) -> Self {
        with_stack_options(self, |o| o.title = Some(t.into()))
    }
    fn header_left(self, btn: HeaderButton) -> Self {
        with_stack_options(self, |o| o.header_left = Some(btn))
    }
    fn header_right(self, btn: HeaderButton) -> Self {
        with_stack_options(self, |o| o.header_right = Some(btn))
    }
    fn hide_header(self, hide: bool) -> Self {
        with_stack_options(self, |o| o.hide_header = hide)
    }
}

// =============================================================================
// Presentation / handle / value types
// =============================================================================

type LayoutBuilder = Rc<dyn Fn(StackContext) -> Element>;

/// The SDK payload riding on `Element::Navigator`; its `TypeId` keys the
/// per-backend [`StackHandler`] registration.
pub struct StackPresentation {
    /// The author layout closure (`None` ⇒ a bare outlet, no chrome).
    pub layout: Option<LayoutBuilder>,
}

impl Default for StackPresentation {
    fn default() -> Self {
        Self { layout: None }
    }
}

/// Typed handle to a live stack navigator, filled into the `Ref` passed to
/// [`StackBuilder::bind`].
#[derive(Clone)]
pub struct StackHandle {
    inner: NavigatorHandle,
}

impl StackHandle {
    /// Wrap a raw handle (called by the backend glue).
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Push `route` onto the stack, building its URL from typed `params`.
    pub fn push<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Push {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Pop the top screen (no-op at the root).
    pub fn pop(&self) {
        self.inner.dispatch(NavCommand::Pop);
    }

    /// Replace the top screen with `route`.
    pub fn replace<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Replace {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Reset the whole stack to a single `route`.
    pub fn reset<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.inner.dispatch(NavCommand::Reset {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Borrow the underlying kind-agnostic handle.
    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

struct StackOps;
impl NavigatorOps for StackOps {}
static STACK_OPS: StackOps = StackOps;

// =============================================================================
// Builder
// =============================================================================

/// The stack-navigator builder. [`StackNavigator::new`] starts one.
pub struct StackNavigator {
    config: NavigatorConfig,
    presentation: StackPresentation,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
}

impl StackNavigator {
    /// Start a stack whose root screen is `initial`.
    pub fn new(initial: &Route<()>) -> Bound<StackHandle> {
        let nav = Self {
            config: NavigatorConfig::new(initial.name(), initial.path()),
            presentation: StackPresentation::default(),
            style: None,
            ref_fill: None,
        };
        Bound::new(nav.into_element())
    }

    fn into_element(self) -> Element {
        // Force-link the platform registration module so its
        // `inventory::submit!` survives dev-profile codegen-unit DCE (see the
        // same note in swap-navigator).
        #[cfg(target_arch = "wasm32")]
        let _ = core::hint::black_box(web::register as *const ());
        #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(macos::register as *const ());
        #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(ios::register as *const ());
        #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
        let _ = core::hint::black_box(android::register as *const ());

        let StackNavigator { config, presentation, style, ref_fill } = self;
        Element::Navigator {
            type_id: TypeId::of::<StackPresentation>(),
            type_name: std::any::type_name::<StackPresentation>(),
            presentation: Rc::new(presentation) as Rc<dyn Any>,
            config: Box::new(config),
            style,
            slot_styles: Vec::new(),
            ref_fill,
            accessibility: Default::default(),
        }
    }
}

fn with_navigator_prim<F: FnOnce(&mut Element)>(b: &mut Bound<StackHandle>, f: F) {
    f(b.primitive_mut());
}

fn with_presentation_mut<F: FnOnce(&mut StackPresentation)>(b: &mut Bound<StackHandle>, f: F) {
    if let Element::Navigator { presentation, .. } = b.primitive_mut() {
        let pres = Rc::get_mut(presentation)
            .expect("stack-navigator: presentation Rc already shared (builder misuse)");
        if let Some(typed) = (pres as &mut dyn Any).downcast_mut::<StackPresentation>() {
            f(typed);
        }
    }
}

/// Fluent builder methods on [`Bound<StackHandle>`].
pub trait StackBuilder: Sized {
    /// Register a screen: its route and the closure building it from params.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set the author layout — wraps `{nav.outlet}` with chrome.
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(StackContext) -> Element + 'static;
    /// Bind a `Ref<StackHandle>` for imperative push/pop.
    fn bind(self, r: Ref<StackHandle>) -> Self;
}

impl StackBuilder for Bound<StackHandle> {
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
                        .expect("stack-navigator: route params type mismatch");
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
        F: Fn(StackContext) -> Element + 'static,
    {
        with_presentation_mut(&mut self, |p| p.layout = Some(Rc::new(f)));
        self
    }

    fn bind(mut self, r: Ref<StackHandle>) -> Self {
        with_navigator_prim(&mut self, |p| {
            if let Element::Navigator { ref_fill, .. } = p {
                *ref_fill = Some(RefFill::Navigator(Box::new(move |handle| {
                    r.fill(StackHandle::from_inner(handle));
                })));
            }
        });
        self
    }
}

// =============================================================================
// The backend-neutral handler
// =============================================================================

/// One mounted screen: its route, node, and scope id.
struct StackEntry<N> {
    route: &'static str,
    path: String,
    node: N,
    scope_id: u64,
    options: StackScreenOptions,
}

/// The framework-attached initial screen, held until the outlet exists.
/// `route`/`path` are the walker-RESOLVED values read at attach time — a
/// cold-start deep link may resolve a route other than the configured
/// initial (see [`SharedStack::seat_initial`]).
struct PendingInitial<N> {
    route: &'static str,
    path: String,
    node: N,
    scope_id: u64,
    options: StackScreenOptions,
}

struct SharedStack<B: Backend> {
    outlet: RefCell<Option<B::Node>>,
    stack: RefCell<Vec<StackEntry<B::Node>>>,
    pending_initial: RefCell<Option<PendingInitial<B::Node>>>,
    initial_route: &'static str,
    initial_path: String,
    /// The walker's reactive route/path mirrors, read UNTRACKED at attach
    /// time: on the non-deferred cold-start path the walker resolves a deep
    /// link and writes the resolved route/path into them BEFORE mounting the
    /// initial screen (see `walker::navigator`).
    active_route: Signal<&'static str>,
    active_path: Signal<String>,
    mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<B::Node>>,
    release_screen: Rc<dyn Fn(u64)>,
    insert_node: Rc<dyn Fn(B::Node, B::Node)>,
    clear_children: Rc<dyn Fn(B::Node)>,
    depth_changed: Rc<dyn Fn(usize)>,
    active_changed: Rc<dyn Fn(&'static str, String)>,
    /// The scoped host slot for the active screen's header state (see
    /// `NavigatorHost::screen_chrome`). This handler is backend-neutral / non-
    /// native, so it publishes `native = false`; a native mobile handler would
    /// render the bar itself and publish `native = true`.
    screen_chrome: Signal<Option<Rc<dyn Any>>>,
}

/// Downcast a `MountResult`'s opaque options to `StackScreenOptions` (default
/// when the screen set none).
fn options_of<N>(r: &MountResult<N>) -> StackScreenOptions {
    r.options
        .downcast_ref::<StackScreenOptions>()
        .cloned()
        .unwrap_or_default()
}

impl<B: Backend> SharedStack<B> {
    /// Show the top entry's node in the outlet.
    fn show_top(&self) {
        let top = self.stack.borrow().last().map(|e| e.node.clone());
        if let (Some(outlet), Some(node)) = (self.outlet.borrow().clone(), top) {
            (self.clear_children)(outlet.clone());
            (self.insert_node)(outlet, node);
        }
    }

    /// Publish the top screen's header slots into the scoped `screen_chrome`
    /// signal so the author `StackHeader` re-renders for the current screen.
    fn sync_chrome(&self) {
        let state = self
            .stack
            .borrow()
            .last()
            .map(|e| e.options.to_state(false));
        self.screen_chrome
            .set(state.map(|s| Rc::new(s) as Rc<dyn Any>));
    }

    /// Seat the framework-attached initial screen. When a cold-start deep
    /// link resolved a route DIFFERENT from the configured initial, the
    /// walker delegates back-stack reconstruction to the stack SDK (see the
    /// non-deferred mount in `walker::navigator`): mount the configured
    /// initial and seat it BELOW the resolved screen, so Back returns to the
    /// index instead of being impossible.
    fn seat_initial(
        &self,
        route: &'static str,
        path: String,
        node: B::Node,
        scope_id: u64,
        options: StackScreenOptions,
    ) {
        if route != self.initial_route {
            let r = (self.mount_screen)(self.initial_route, Box::new(()), None);
            let base_options = options_of(&r);
            self.stack.borrow_mut().push(StackEntry {
                route: self.initial_route,
                path: self.initial_path.clone(),
                node: r.node,
                scope_id: r.scope_id,
                options: base_options,
            });
        }
        self.stack.borrow_mut().push(StackEntry {
            route,
            path,
            node,
            scope_id,
            options,
        });
        self.show_top();
        let len = self.stack.borrow().len();
        (self.depth_changed)(len);
        self.sync_chrome();
    }

    fn push(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        let r = (self.mount_screen)(name, params, state);
        let options = options_of(&r);
        self.stack.borrow_mut().push(StackEntry {
            route: name,
            path: url,
            node: r.node,
            scope_id: r.scope_id,
            options,
        });
        self.show_top();
        // Copy the length out so no `stack` borrow is held across the
        // callback — it sets signals whose subscribers may re-enter this
        // navigator (same hardening as `pop`).
        let len = self.stack.borrow().len();
        (self.depth_changed)(len);
        self.sync_chrome();
    }

    fn pop(&self) {
        // Never pop the root.
        if self.stack.borrow().len() <= 1 {
            return;
        }
        let popped = self.stack.borrow_mut().pop();
        if let Some(entry) = popped {
            (self.release_screen)(entry.scope_id);
        }
        self.show_top();
        let len = self.stack.borrow().len();
        (self.depth_changed)(len);
        // Pop carries no new route name through the substrate — update the
        // active mirror to the revealed screen ourselves. Copy the pair out
        // so no `stack` borrow is held across the callback.
        let revealed = self
            .stack
            .borrow()
            .last()
            .map(|top| (top.route, top.path.clone()));
        if let Some((route, path)) = revealed {
            (self.active_changed)(route, path);
        }
        self.sync_chrome();
    }

    fn replace(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        let r = (self.mount_screen)(name, params, state);
        let options = options_of(&r);
        let old = self.stack.borrow_mut().pop();
        if let Some(entry) = old {
            (self.release_screen)(entry.scope_id);
        }
        self.stack.borrow_mut().push(StackEntry {
            route: name,
            path: url,
            node: r.node,
            scope_id: r.scope_id,
            options,
        });
        self.show_top();
        // Borrow-free callback, same hardening as `push`/`pop`.
        let len = self.stack.borrow().len();
        (self.depth_changed)(len);
        self.sync_chrome();
    }

    fn reset(&self, name: &'static str, params: Box<dyn Any>, state: Option<Rc<dyn Any>>, url: String) {
        // Release the whole stack, then seat the new single screen.
        let old: Vec<_> = self.stack.borrow_mut().drain(..).collect();
        for entry in old {
            (self.release_screen)(entry.scope_id);
        }
        let r = (self.mount_screen)(name, params, state);
        let options = options_of(&r);
        self.stack.borrow_mut().push(StackEntry {
            route: name,
            path: url,
            node: r.node,
            scope_id: r.scope_id,
            options,
        });
        self.show_top();
        (self.depth_changed)(1);
        self.sync_chrome();
    }
}

/// The one stack handler for every backend.
pub struct StackHandler<B: Backend> {
    control: Option<Rc<NavigatorControl>>,
    shared: Option<Rc<SharedStack<B>>>,
}

impl<B: Backend> StackHandler<B> {
    /// A fresh, uninitialized handler.
    pub fn new() -> Self {
        Self { control: None, shared: None }
    }
}

impl<B: Backend> Default for StackHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Backend + 'static> NavigatorHandler<B> for StackHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        let a11y = AccessibilityProps::default();
        let root = backend.create_view(&a11y);
        // Fill-the-container default (see `navigator_fill_rules`) — a bare
        // root hugs content, collapsing a viewport-height app. The author's
        // `.with_style(...)` on the navigator element is applied by the
        // walker AFTER init, so it overrides this.
        backend.apply_style(&root, &navigator_fill_rules());

        let NavigatorHost {
            initial_route,
            initial_path,
            mount_screen,
            release_screen,
            insert_node,
            clear_children,
            control,
            build_layout_with_outlet,
            nav_state,
            screen_chrome,
            depth_changed,
            active_changed,
            base,
            ..
        } = host;

        let layout = presentation
            .downcast_ref::<StackPresentation>()
            .and_then(|p| p.layout.clone());

        let shared = Rc::new(SharedStack::<B> {
            outlet: RefCell::new(None),
            stack: RefCell::new(Vec::new()),
            pending_initial: RefCell::new(None),
            initial_route,
            initial_path: runtime_core::primitives::navigator::join_path(&base, initial_path),
            active_route: nav_state.active_route,
            active_path: nav_state.active_path,
            mount_screen,
            release_screen,
            insert_node,
            clear_children,
            depth_changed,
            active_changed,
            screen_chrome,
        });

        // Push/Pop/Replace/Reset dispatcher. The substrate already updated the
        // active-route mirror for the depth-increasing commands; we mount/swap
        // and keep depth + the pop's active mirror in sync.
        control.install({
            let shared = shared.clone();
            Box::new(move |cmd| match cmd {
                NavCommand::Push { name, params, state, url } => shared.push(name, params, state, url),
                NavCommand::Pop => shared.pop(),
                NavCommand::Replace { name, params, state, url } => {
                    shared.replace(name, params, state, url)
                }
                NavCommand::Reset { name, params, state, url } => {
                    shared.reset(name, params, state, url)
                }
                // A stack never receives Select (no link activator installed).
                NavCommand::Select { .. } | NavCommand::Custom(_) => {}
            })
        });

        // Defer the author-layout build past this borrow (re-borrows the
        // backend). Build StackContext, capture the outlet, splice chrome into
        // `root`, and seat the framework-mounted initial screen as depth 1.
        {
            let shared = shared.clone();
            let root = root.clone();
            let control_for_ctx = control.clone();
            let active_route = nav_state.active_route;
            let active_path = nav_state.active_path;
            let depth = nav_state.depth;
            let can_go_back = nav_state.can_go_back;
            runtime_core::schedule_microtask(move || {
                let pop: Rc<dyn Fn()> = {
                    let control = control_for_ctx.clone();
                    Rc::new(move || control.dispatch(NavCommand::Pop))
                };
                let ctx = StackContext {
                    outlet: navigator_outlet(),
                    active_route,
                    active_path,
                    depth,
                    can_go_back,
                    pop,
                    screen_chrome: shared.screen_chrome,
                };
                let layout_elem = match &layout {
                    Some(f) => f(ctx),
                    None => ctx.outlet,
                };
                let (layout_root, outlet) = (build_layout_with_outlet)(layout_elem);
                debug_assert!(
                    outlet.is_some(),
                    "stack-navigator: the author `.layout(...)` must splat `{{nav.outlet}}`"
                );
                (shared.insert_node)(root, layout_root);
                *shared.outlet.borrow_mut() = outlet;

                // Seat the framework-attached initial screen (deep-link
                // aware: a resolved-elsewhere route gets the configured
                // initial reconstructed beneath it — see `seat_initial`).
                let pending: Option<PendingInitial<B::Node>> =
                    shared.pending_initial.borrow_mut().take();
                match pending {
                    Some(p) => shared.seat_initial(p.route, p.path, p.node, p.scope_id, p.options),
                    None => {
                        // Defensive: nothing attached (deferred-mount host) —
                        // mount the configured initial ourselves.
                        let r = (shared.mount_screen)(shared.initial_route, Box::new(()), None);
                        let options = options_of(&r);
                        shared.seat_initial(
                            shared.initial_route,
                            shared.initial_path.clone(),
                            r.node,
                            r.scope_id,
                            options,
                        );
                    }
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
        options: Box<dyn Any>,
    ) {
        if let Some(shared) = &self.shared {
            let opts = options
                .downcast_ref::<StackScreenOptions>()
                .cloned()
                .unwrap_or_default();
            // The walker resolved a cold-start deep link BEFORE mounting this
            // screen and wrote the resolved route/path into the nav-state
            // mirrors — capture them NOW so the entry is labeled with what it
            // actually shows (not the configured initial).
            let route = runtime_core::untrack(|| shared.active_route.get());
            let path = runtime_core::untrack(|| shared.active_path.get());
            let outlet_built = shared.outlet.borrow().is_some();
            if outlet_built {
                // Rare: outlet already built (microtask ran first) and the
                // microtask's defensive branch self-seated the configured
                // initial — replace it with the framework-mounted screen
                // rather than stashing a copy that would never be seated
                // (mirrors swap's outlet-already-built branch).
                let old: Vec<StackEntry<B::Node>> =
                    shared.stack.borrow_mut().drain(..).collect();
                for entry in old {
                    (shared.release_screen)(entry.scope_id);
                }
                shared.seat_initial(route, path, screen, scope_id, opts);
            } else {
                *shared.pending_initial.borrow_mut() = Some(PendingInitial {
                    route,
                    path,
                    node: screen,
                    scope_id,
                    options: opts,
                });
            }
        }
    }

    fn on_system_back(&mut self, _backend: &mut B) -> bool {
        // Consume a system back if we can pop; otherwise let the platform handle
        // it (exit / bubble to a parent navigator).
        if let (Some(control), Some(shared)) = (&self.control, &self.shared) {
            if shared.stack.borrow().len() > 1 {
                control.dispatch(NavCommand::Pop);
                return true;
            }
        }
        false
    }

    fn make_handle(&self) -> NavigatorHandle {
        match &self.control {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &STACK_OPS, c.clone()),
            None => NavigatorHandle::new(Rc::new(()), &STACK_OPS),
        }
    }
}

// =============================================================================
// Per-backend registration
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{StackHandler, StackPresentation};
    use backend_web::WebBackend;
    /// Register the stack handler on the web backend.
    pub fn register(backend: &mut WebBackend) {
        backend.register_navigator::<StackPresentation, _>(|| {
            Box::new(StackHandler::<WebBackend>::new())
        });
    }
    inventory::submit! { backend_web::WebNavigatorRegistrar(register) }
}
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos {
    use super::{StackHandler, StackPresentation};
    use backend_macos::MacosBackend;
    /// Register the stack handler on the macOS backend.
    pub fn register(backend: &mut MacosBackend) {
        backend.register_navigator::<StackPresentation, _>(|| {
            Box::new(StackHandler::<MacosBackend>::new())
        });
    }
    inventory::submit! { backend_macos::MacosNavigatorRegistrar(register) }
}
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

// iOS / Android: the SAME backend-neutral handler (outlet swap on push/pop,
// author-layout header). The mobile backends key their handler map by the node
// itself, so dispatch + the bound handle wire up with no extra work. NOTE: this
// is the outlet-swap stack — it does NOT (yet) use a native UINavigationController
// / FragmentManager push surface, so there's no OS push animation or edge-swipe
// back gesture on mobile. That native-surface integration (bar hidden, author
// header) is a further, device-verified step.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios {
    use super::{StackHandler, StackPresentation};
    use backend_ios::IosBackend;
    /// Register the stack handler on the iOS backend.
    pub fn register(backend: &mut IosBackend) {
        backend.register_navigator::<StackPresentation, _>(|| {
            Box::new(StackHandler::<IosBackend>::new())
        });
    }
    inventory::submit! { backend_ios::IosNavigatorRegistrar(register) }
}
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android {
    use super::{StackHandler, StackPresentation};
    use backend_android::AndroidBackend;
    /// Register the stack handler on the Android backend.
    pub fn register(backend: &mut AndroidBackend) {
        backend.register_navigator::<StackPresentation, _>(|| {
            Box::new(StackHandler::<AndroidBackend>::new())
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
    /// No-op registration for backends without a dedicated registrar.
    pub fn register<B: Backend>(_backend: &mut B) {}
}
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
pub use fallback::register;

/// Convenience re-exports.
pub mod prelude {
    pub use super::{
        header_state, register, StackBuilder, StackContext, StackHandle, StackNavigator,
        StackPresentation, StackScreenExt, StackScreenOptions,
    };
    pub use runtime_core::primitives::navigator::{HeaderButton, StackHeaderState};
}
