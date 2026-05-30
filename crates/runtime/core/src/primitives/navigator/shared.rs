//! Shared navigator substrate — the kind-agnostic core every navigator
//! SDK builds on.
//!
//! The framework owns the substrate (routing, screen scopes, ambient
//! capture, command queue, reactive nav state, per-screen state stack).
//! SDK crates own everything kind-specific (chrome, animations, gestures,
//! typed handles, typed screen options). No kind names appear in this
//! module — `Stack` / `Tab` / `Drawer` are SDK concepts, not framework
//! concepts.
//!
//! What lives here:
//!
//! - `Route<P>` + `RouteParams` — typed route declaration + URL ⇄ params.
//! - `ScreenBuilder` / `RouteEntry` / `ParamsFromSegments` — type-erased
//!   per-route registry the framework walks.
//! - `Screen` + `MountResult` — what a screen builder returns, what
//!   `mount_screen` hands back. SDK-defined options ride as
//!   `Box<dyn Any>`.
//! - `NavCommand` — the command channel. Built-in verbs cover the
//!   common shapes (Push / Pop / Replace / Reset / Select); SDKs add
//!   their own via `NavCommand::Custom(Rc<dyn Any>)`.
//! - `NavigatorControl` — the dispatcher + reactive nav-state bridge.
//! - `NavigatorHandle` — the framework-side handle. Just dispatch +
//!   control accessor; SDK typed handles wrap it.
//! - `NavigatorOps` — the trait the handle's `&dyn NavigatorOps`
//!   points to (currently empty; reserved for backend extension hooks).
//! - `NavState` — reactive `active_route` / `active_path` / `depth` /
//!   `can_go_back` signals layout/chrome subscribes to.
//! - `AmbientNavGuard` / `ambient_navigator()` — thread-local stack
//!   `Link` reads at build time.
//! - `ScreenStateGuard` / `current_screen_state` — per-screen opaque
//!   state stack the screen render closure reads via downcast.
//! - `NavigatorConfig` — the framework-owned routing config (initial
//!   route, screen registry, defer flag). Kind-specific config lives
//!   on the SDK's presentation payload.
//! - `match_pattern` — pure-Rust URL-against-pattern matcher.

use crate::Element;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Ambient navigator stack — Link primitives find their navigator here
// ---------------------------------------------------------------------------

thread_local! {
    static AMBIENT_NAV: RefCell<Vec<Rc<NavigatorControl>>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard that pushes a navigator's control plane onto the ambient
/// stack while a screen is building. The `Link` primitive captures the
/// top of the stack at construction time.
pub struct AmbientNavGuard;

impl AmbientNavGuard {
    pub fn push(control: Rc<NavigatorControl>) -> Self {
        AMBIENT_NAV.with(|s| s.borrow_mut().push(control));
        AmbientNavGuard
    }
}

impl Drop for AmbientNavGuard {
    fn drop(&mut self) {
        AMBIENT_NAV.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// Read the top of the ambient-navigator stack. `None` when called
/// outside any navigator's `mount_screen`.
pub fn ambient_navigator() -> Option<Rc<NavigatorControl>> {
    AMBIENT_NAV.with(|s| s.borrow().last().cloned())
}

// ---------------------------------------------------------------------------
// RouteParams — URL ⇄ typed params
// ---------------------------------------------------------------------------

/// Convert route params to/from URL path segments. Implemented on every
/// type used as a `Route<P>` payload; built-in for `()` (the no-params
/// case). Web/SSR backends use this to map between URLs and typed
/// payloads; native backends ignore the path side.
pub trait RouteParams: 'static + Sized {
    fn to_path(&self, pattern: &str) -> String {
        let _ = self;
        if pattern.contains(':') {
            panic!(
                "RouteParams::to_path default impl can't fill placeholder \
                 segments in pattern '{}'. Implement RouteParams for your \
                 params type to serialize each `:segment`.",
                pattern
            );
        }
        pattern.to_string()
    }

    fn from_segments(_segments: &HashMap<String, String>) -> Option<Self> {
        None
    }
}

impl RouteParams for () {
    fn to_path(&self, pattern: &str) -> String {
        pattern.to_string()
    }

    fn from_segments(_segments: &HashMap<String, String>) -> Option<Self> {
        Some(())
    }
}

// ---------------------------------------------------------------------------
// Route<P> — typed route name + URL pattern
// ---------------------------------------------------------------------------

/// A navigation route. `name` is the in-stack key; `path` is the URL
/// pattern used by web/SSR backends. The phantom `P` is what
/// `handle.push(route, params)` etc. type-check against.
#[derive(Clone)]
pub struct Route<P: RouteParams = ()> {
    name: &'static str,
    path: &'static str,
    _params: PhantomData<P>,
}

impl<P: RouteParams> Route<P> {
    pub const fn new(name: &'static str, path: &'static str) -> Self {
        Self { name, path, _params: PhantomData }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn path(&self) -> &'static str {
        self.path
    }
}

// ---------------------------------------------------------------------------
// ScreenBuilder + RouteEntry — type-erased per-route registry
// ---------------------------------------------------------------------------

pub type ScreenBuilder = Rc<dyn Fn(Box<dyn Any>) -> Screen>;

pub type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

pub struct RouteEntry {
    pub path: &'static str,
    pub build: ScreenBuilder,
    pub from_segments: ParamsFromSegments,
}

// ---------------------------------------------------------------------------
// Screen — what a route's render closure returns
// ---------------------------------------------------------------------------

/// A renderable screen: the body Element plus SDK-defined options.
///
/// Options are opaque to the framework (`Box<dyn Any>`). Each SDK
/// defines its own typed options struct (e.g. `StackScreenOptions`
/// with title + bar buttons; `TabScreenOptions` with icon + label).
/// Authors call SDK-provided builder methods (`.title(…)`, `.left(…)`)
/// which stash a typed value into `Screen.options`. The SDK handler
/// downcasts at apply time.
///
/// `impl From<Element> for Screen` keeps the no-options form
/// ergonomic: `.screen(R, |_| my_body_view().into())`.
pub struct Screen {
    pub primitive: Element,
    pub options: Box<dyn Any>,
}

impl Screen {
    pub fn new(primitive: impl Into<Element>) -> Self {
        Self {
            primitive: primitive.into(),
            options: Box::new(()),
        }
    }

    /// Set this screen's SDK-defined options. Replaces any existing
    /// options. Each SDK defines its own typed options struct and
    /// exposes builder methods (via an extension trait on `Screen`)
    /// that wrap this.
    pub fn with<T: Any + 'static>(mut self, options: T) -> Self {
        self.options = Box::new(options);
        self
    }

    /// Downcast the options to a borrow of `T`. `None` when this
    /// screen has no options or the stored type doesn't match.
    pub fn options_as<T: Any + 'static>(&self) -> Option<&T> {
        self.options.downcast_ref::<T>()
    }
}

impl From<Element> for Screen {
    fn from(p: Element) -> Self {
        Self::new(p)
    }
}

/// Result of mounting a screen. `mount_screen` returns this so the
/// SDK handler has the body node, the framework-owned scope id (used
/// to release the scope later), and the screen's opaque options
/// (downcast inside the handler).
pub struct MountResult<N> {
    pub node: N,
    pub scope_id: u64,
    pub options: Box<dyn Any>,
}

// ---------------------------------------------------------------------------
// Path matching — pure-Rust matcher used by web + future SSR
// ---------------------------------------------------------------------------

/// Match `path` against `pattern`. Returns `Some(map)` if segment
/// counts agree and every literal segment matches case-sensitively;
/// `:placeholder` segments become entries in the returned map.
///
/// Trailing slashes are tolerated; empty path is treated as `/`.
pub fn match_pattern(path: &str, pattern: &str) -> Option<HashMap<String, String>> {
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if path_segs.len() != pat_segs.len() {
        return None;
    }
    let mut out = HashMap::new();
    for (p, pat) in path_segs.iter().zip(pat_segs.iter()) {
        if let Some(name) = pat.strip_prefix(':') {
            out.insert(name.to_string(), (*p).to_string());
        } else if *p != *pat {
            return None;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// NavigatorHandle — framework-side handle. Just dispatch + control accessor.
// SDK typed handles wrap it with kind-specific methods.
// ---------------------------------------------------------------------------

/// The handle the framework hands to `Ref<H>` bindings. Carries an
/// opaque node, a `&'static dyn NavigatorOps`, and an optional
/// `Rc<NavigatorControl>` for dispatch.
///
/// **No kind-specific methods here.** `push` / `pop` / `select` /
/// drawer open/close live on the SDK's typed handle (e.g.
/// `StackHandle`, `DrawerHandle`), which wraps `NavigatorHandle` and
/// dispatches via `self.dispatch(NavCommand::…)`.
#[derive(Clone)]
pub struct NavigatorHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn NavigatorOps,
    control: Option<Rc<NavigatorControl>>,
}

impl NavigatorHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn NavigatorOps) -> Self {
        Self { node, ops, control: None }
    }

    pub fn with_control(
        node: Rc<dyn Any>,
        ops: &'static dyn NavigatorOps,
        control: Rc<NavigatorControl>,
    ) -> Self {
        Self { node, ops, control: Some(control) }
    }

    /// Access the underlying control plane. SDK typed handles use this
    /// to dispatch their kind-specific commands.
    pub fn control(&self) -> Option<&Rc<NavigatorControl>> {
        self.control.as_ref()
    }

    /// Dispatch a NavCommand against this navigator. Silent no-op when
    /// the handle has no control (pre-mount).
    pub fn dispatch(&self, cmd: NavCommand) {
        if let Some(c) = &self.control {
            c.dispatch(cmd);
        }
    }

    /// Cached depth — set by the SDK handler via
    /// `NavigatorControl::set_depth`. Cheap; doesn't reach the SDK.
    pub fn depth(&self) -> usize {
        self.control.as_ref().map(|c| c.depth()).unwrap_or(0)
    }

    /// Type-erased access to the navigator's opaque node payload. SDK
    /// typed handles use this to look up SDK-owned per-instance state.
    pub fn node_as_any(&self) -> &dyn Any {
        &*self.node
    }

    /// The static ops pointer. Currently unused (`NavigatorOps` has no
    /// methods); reserved for future per-backend hooks the handle
    /// might want to dispatch through.
    #[allow(dead_code)]
    pub(crate) fn ops(&self) -> &'static dyn NavigatorOps {
        self.ops
    }
}

/// Backend hook trait the handle's `&dyn NavigatorOps` points to.
/// Reserved for backend extension methods that need to dispatch
/// through the handle's static vtable. Currently empty — the dispatch
/// path goes through `NavigatorControl` directly.
pub trait NavigatorOps {}

// ---------------------------------------------------------------------------
// NavigatorControl — dispatcher + reactive nav-state bridge
// ---------------------------------------------------------------------------

/// The shared control plane between framework substrate and SDK
/// handler. Wraps the command dispatcher closure the SDK installs at
/// `init` time, a depth cache the handle reads, and the reactive
/// `NavState` mirror chrome subscribes to.
pub struct NavigatorControl {
    dispatch: RefCell<Option<Box<dyn Fn(NavCommand)>>>,
    depth: RefCell<usize>,
    nav_state: RefCell<Option<NavState>>,
    /// Optional SDK-installed link activation builder. Maps the
    /// triple `(route_name, url, params)` to a `NavCommand`. The
    /// `Link` primitive calls this on activation to pick the right
    /// dispatch verb for the enclosing navigator — stack SDKs install
    /// one that builds `Push`; tab/drawer SDKs install one that builds
    /// `Select`. When not installed, `Link` defaults to `Push`.
    link_activator: RefCell<
        Option<Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand>>,
    >,
}

impl NavigatorControl {
    pub fn new() -> Self {
        Self {
            dispatch: RefCell::new(None),
            depth: RefCell::new(1),
            nav_state: RefCell::new(None),
            link_activator: RefCell::new(None),
        }
    }

    /// Wire the framework's reactive nav-state mirror. Called once
    /// from `walker::navigator::build` before `install`.
    pub fn attach_nav_state(&self, nav_state: NavState) {
        *self.nav_state.borrow_mut() = Some(nav_state);
    }

    /// Install the SDK's command dispatcher closure. Called once from
    /// the SDK handler's `init`.
    pub fn install(&self, dispatch: Box<dyn Fn(NavCommand)>) {
        *self.dispatch.borrow_mut() = Some(dispatch);
    }

    /// Install the SDK's `Link` activation builder. Optional; if not
    /// set, `Link` defaults to `NavCommand::Push`. Stack-like SDKs
    /// typically don't install (Push is the default); tab/drawer SDKs
    /// install one that returns `Select`.
    pub fn install_link_activator(
        &self,
        f: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand>,
    ) {
        *self.link_activator.borrow_mut() = Some(f);
    }

    /// Build the activation command for a `Link` activating against
    /// this navigator. Falls back to `Push` when no activator was
    /// installed.
    pub fn build_link_command(
        &self,
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    ) -> NavCommand {
        if let Some(f) = self.link_activator.borrow().as_ref() {
            f(name, url, params)
        } else {
            NavCommand::Push { name, url, params, state: None }
        }
    }

    /// Update the cached depth. SDK handler calls this when stack
    /// depth changes so `handle.depth()` stays in sync.
    pub fn set_depth(&self, d: usize) {
        *self.depth.borrow_mut() = d;
    }

    pub fn depth(&self) -> usize {
        *self.depth.borrow()
    }

    /// Dispatch a NavCommand against this navigator. Updates the
    /// reactive nav-state mirror (for commands that change the active
    /// route) before forwarding to the SDK's installed dispatcher.
    pub fn dispatch(&self, cmd: NavCommand) {
        // Update the active route/path signals before the SDK sees
        // the command, so any effect reading them re-fires while the
        // SDK is still committing the change. Pop and Custom don't
        // carry a new route name — the SDK is responsible for
        // updating signals via `active_changed` after committing.
        if let Some(state) = self.nav_state.borrow().as_ref() {
            match &cmd {
                NavCommand::Push { name, url, .. }
                | NavCommand::Replace { name, url, .. }
                | NavCommand::Reset { name, url, .. }
                | NavCommand::Select { name, url, .. } => {
                    state.active_route.set(name);
                    state.active_path.set(url.clone());
                }
                NavCommand::Pop | NavCommand::Custom(_) => {}
            }
        }
        if let Some(f) = self.dispatch.borrow().as_ref() {
            f(cmd);
        }
    }
}

impl Default for NavigatorControl {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// NavCommand — the framework command vocabulary
// ---------------------------------------------------------------------------

/// Commands that flow through `NavigatorControl::dispatch`. The built-in
/// verbs cover the common navigation shapes; SDKs with novel verbs
/// (drawer open/close, multi-pane focus, etc.) use `Custom`.
///
/// The `state` field on stack-shaped variants is an SDK/author-opaque
/// payload riding alongside the typed `params`. The screen builder can
/// read it via [`current_screen_state`].
///
/// SDK handlers receive every dispatched command via their installed
/// dispatcher closure. Handlers that don't understand a variant
/// should silently no-op or panic according to their own contract.
pub enum NavCommand {
    Push {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        state: Option<Rc<dyn Any>>,
    },
    Pop,
    Replace {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        state: Option<Rc<dyn Any>>,
    },
    Reset {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        state: Option<Rc<dyn Any>>,
    },
    /// Switch the active screen by name without changing stack depth.
    /// Used by tab- and drawer-style SDKs.
    Select {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
        state: Option<Rc<dyn Any>>,
    },
    /// SDK-specific command. The payload is downcast by the SDK
    /// handler's dispatcher to its expected type. Used for verbs the
    /// built-in variants don't cover (drawer Open/Close/Toggle, a
    /// multi-pane SDK's SplitFocus, etc.). Wire-protocol-aware SDKs
    /// register a serde pair via `register_navigator_command_serde`
    /// so `Custom` payloads round-trip across dev-mode wire frames.
    Custom(Rc<dyn Any>),
}

// ---------------------------------------------------------------------------
// Per-screen state stack — author-opaque payload pushed at
// dispatch time, readable inside the screen's render via
// `current_screen_state::<T>()`.
// ---------------------------------------------------------------------------

thread_local! {
    static SCREEN_STATE: RefCell<Vec<Option<Rc<dyn Any>>>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard the framework pushes around each screen build. SDK
/// handlers don't construct these directly — `host.mount_screen`
/// pushes one for the duration of the build.
pub struct ScreenStateGuard;

impl ScreenStateGuard {
    pub fn push(state: Option<Rc<dyn Any>>) -> Self {
        SCREEN_STATE.with(|s| s.borrow_mut().push(state));
        ScreenStateGuard
    }
}

impl Drop for ScreenStateGuard {
    fn drop(&mut self) {
        SCREEN_STATE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// Read the current screen's opaque `state` payload, downcast to `T`.
/// `None` when called outside a screen build, when no state was
/// passed at navigation time, or when the stored type isn't `T`.
pub fn current_screen_state<T: Any>() -> Option<Rc<T>> {
    SCREEN_STATE.with(|s| {
        s.borrow()
            .last()
            .and_then(|opt| opt.clone())
            .and_then(|rc| Rc::downcast::<T>(rc).ok())
    })
}

// ---------------------------------------------------------------------------
// Per-screen route name stack — pushed by the walker at mount time
// so author code inside a screen build can ask "what route am I?"
// without plumbing the name through every component.
// ---------------------------------------------------------------------------

thread_local! {
    static SCREEN_ROUTE: RefCell<Vec<&'static str>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard pushed around each screen build alongside
/// [`ScreenStateGuard`]. SDK handlers don't construct these directly;
/// the framework's `mount_screen` does.
pub struct ScreenRouteGuard;

impl ScreenRouteGuard {
    pub fn push(name: &'static str) -> Self {
        SCREEN_ROUTE.with(|s| s.borrow_mut().push(name));
        ScreenRouteGuard
    }
}

impl Drop for ScreenRouteGuard {
    fn drop(&mut self) {
        SCREEN_ROUTE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// Return the route name being built right now. `None` when called
/// outside a screen build. Author code uses this together with
/// [`ambient_navigator`] (and its `nav_state.active_route`) to derive
/// a per-screen focus signal — see [`use_focus`].
pub fn current_screen_route() -> Option<&'static str> {
    SCREEN_ROUTE.with(|s| s.borrow().last().copied())
}

/// Returns a function `() -> bool` that reads as `true` whenever the
/// current screen is the navigator's active route. Call inside a
/// screen render to wire focus-driven behavior (pause/resume an
/// embedded `host_wgpu::IosHostHandle`, mute a video, stop a poll,
/// rebind a keyboard shortcut, etc.).
///
/// The returned closure is reactive — read it inside an `effect!`
/// block (or any reactive context) and the effect re-runs whenever
/// focus changes:
///
/// ```ignore
/// use runtime_core::primitives::navigator::use_focus;
///
/// let is_focused = use_focus();
/// effect!(move || {
///     if is_focused() {
///         handle.resume();
///     } else {
///         handle.pause();
///     }
/// });
/// ```
///
/// Returns `|| false` when called outside a screen build (no ambient
/// navigator or no current route). Authors who need to distinguish
/// "no navigator" from "not focused" can check
/// [`current_screen_route`] / [`ambient_navigator`] directly.
pub fn use_focus() -> impl Fn() -> bool + 'static {
    let route = current_screen_route();
    // Capture the `active_route` signal at use-time. The signal is
    // an `Rc`, so the clone is cheap and keeps the source alive even
    // if the NavigatorControl itself is later dropped — that means
    // the returned closure stays callable for the rest of the
    // enclosing scope's lifetime.
    let active_route = ambient_navigator()
        .and_then(|n| n.nav_state.borrow().as_ref().map(|s| s.active_route));
    move || match (route, active_route) {
        (Some(r), Some(sig)) => sig.get() == r,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Headless initial-path override (server-side rendering).
//
// A backend rendering headlessly at a specific URL (the SSR backend
// emitting "/about") sets this before `mount`. The navigator walker's
// initial mount consults it once: if the path resolves to a registered
// route, that screen is mounted instead of the hardcoded `initial`, and
// the nav-state is synced so any chrome reads the right route. `take`
// semantics mean the first (root) navigator consumes it — a nested
// navigator won't re-apply the same path.
//
// Live backends (web/iOS/Android) never set this; they read the current
// path from their own platform (window.location, deep-link intent) in
// the SDK handler layer.
// ---------------------------------------------------------------------------

thread_local! {
    static INITIAL_PATH: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the path the next headlessly-mounted navigator should open to.
/// Pass `None` to clear. See module note above.
pub fn set_initial_path(path: Option<String>) {
    INITIAL_PATH.with(|p| *p.borrow_mut() = path);
}

/// Consume the headless initial-path override, if any. Called by the
/// navigator walker at initial mount.
pub fn take_initial_path() -> Option<String> {
    INITIAL_PATH.with(|p| p.borrow_mut().take())
}

// ---------------------------------------------------------------------------
// Route collector — SSG nav-hierarchy discovery
// ---------------------------------------------------------------------------
//
// The SSG driver (in `backend-ssr`) enables this before each
// `render_path` call. Every `Element::Navigator` the walker dispatches
// publishes its `RouteEntry.path` set to the collector. After mount,
// the driver drains discovered paths, queues unrendered literals, and
// loops — so nested navigators (a drawer with a stack inside) get their
// routes harvested when the parent screen mounts.
//
// Live backends never enable this. The hook is a single `with_collector`
// check in `dispatch_navigator`; when no collector is set the call is a
// thread-local borrow + branch, no allocation.

thread_local! {
    static ROUTE_COLLECTOR: RefCell<Option<Vec<&'static str>>> =
        const { RefCell::new(None) };
}

/// Enable the route collector. SSG calls this before each `render_path`
/// to harvest every navigator's screen paths during mount.
pub fn enable_route_collector() {
    ROUTE_COLLECTOR.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
    });
}

/// Disable the collector and return everything pushed since enable.
/// Returns `None` if the collector wasn't enabled.
pub fn take_route_collector() -> Option<Vec<&'static str>> {
    ROUTE_COLLECTOR.with(|c| c.borrow_mut().take())
}

/// Publish a navigator's screen paths to the collector, if one is
/// enabled. Called by `dispatch_navigator` at mount time; a no-op when
/// the collector is off (live backends).
pub fn record_routes(config: &NavigatorConfig) {
    ROUTE_COLLECTOR.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            for entry in config.screens.values() {
                buf.push(entry.path);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// NavState — reactive bundle exposed to layout / chrome
// ---------------------------------------------------------------------------

/// Reactive nav-state mirror. Updated by `NavigatorControl::dispatch`
/// on every command commit (and by SDK handlers via
/// `host.depth_changed` / `host.active_changed` for asynchronous
/// state changes the framework can't see, like native back gestures).
#[derive(Clone)]
pub struct NavState {
    pub active_route: crate::Signal<&'static str>,
    pub active_path: crate::Signal<String>,
    pub depth: crate::Signal<usize>,
    pub can_go_back: crate::Signal<bool>,
}

// ---------------------------------------------------------------------------
// NavigatorConfig — shared, kind-agnostic routing config
// ---------------------------------------------------------------------------

/// The framework-owned routing config carried by every
/// `Element::Navigator`. SDK builders fill this from their
/// `.screen(...)` declarations. Kind-specific config (drawer width,
/// tab placement, sidebar Element, etc.) lives on the SDK's
/// presentation payload, not here.
pub struct NavigatorConfig {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, RouteEntry>,
    /// When `true`, the framework does NOT auto-mount the initial
    /// screen — the SDK handler is expected to self-mount (typically
    /// after reading the current URL on web). Defaults to `false`.
    pub defer_initial_mount: bool,
}

impl NavigatorConfig {
    pub fn new(initial: &'static str, initial_path: &'static str) -> Self {
        Self {
            initial,
            initial_path,
            screens: HashMap::new(),
            defer_initial_mount: false,
        }
    }
}
