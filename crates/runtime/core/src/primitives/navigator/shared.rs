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
// Hierarchical base path — a nested navigator's URL prefix
// ---------------------------------------------------------------------------
//
// Navigators form a tree; each owns a URL PREFIX (its "base"). The root's
// base is empty. When a navigator mounts a screen, it pushes `base +
// route.path()` here for the duration of building that screen's body, so a
// child `Element::Navigator` nested in that screen reads its own base. Route
// patterns are therefore RELATIVE to the navigator they're registered on; the
// framework composes the full URL up the tree (`join_path`) and peels prefixes
// down it (`match_prefix`). A single root navigator (base "") is unaffected:
// `join_path("", p) == p`, so existing apps behave identically.

thread_local! {
    static NAV_BASE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard pushing the base prefix a nested navigator resolves relative
/// to. Held by `mount_screen` while building a screen body.
pub struct NavBaseGuard;

impl NavBaseGuard {
    pub fn push(base: String) -> Self {
        NAV_BASE.with(|s| s.borrow_mut().push(base));
        NavBaseGuard
    }
}

impl Drop for NavBaseGuard {
    fn drop(&mut self) {
        NAV_BASE.with(|s| {
            let _ = s.borrow_mut().pop();
        });
    }
}

/// The base prefix the navigator currently being built resolves its routes
/// relative to. Empty (`""`) for the root navigator.
pub fn current_nav_base() -> String {
    NAV_BASE.with(|s| s.borrow().last().cloned().unwrap_or_default())
}

/// Join a base prefix with a (relative) route path into a full URL path,
/// collapsing duplicate/empty slashes. `join_path("/encounters", "/abc") ==
/// "/encounters/abc"`, `join_path("", "/today") == "/today"`,
/// `join_path("/encounters", "") == "/encounters"`, `join_path("", "") == "/"`.
pub fn join_path(base: &str, rel: &str) -> String {
    let b = base.trim_end_matches('/');
    let r = rel.trim_start_matches('/');
    if r.is_empty() {
        if b.is_empty() {
            "/".to_string()
        } else {
            b.to_string()
        }
    } else if b.is_empty() {
        format!("/{r}")
    } else {
        format!("{b}/{r}")
    }
}

/// Snapshot of the ambient navigator context (nav control, screen
/// state, screen route) at a point in the build. Reactive regions
/// (`when`/`switch`/`for`) capture this when first built — inside the
/// screen's ambient scope — and re-establish it around every rebuild,
/// so a subtree rebuilt by a signal change (e.g. a `link` whose active
/// styling flips) keeps the same ambient navigator it was born with.
/// Without this, a reactively-remounted `link` captures `None` and
/// silently stops navigating.
///
/// The navigator control is held WEAK on purpose: the navigator owns
/// the screen scopes, a screen scope owns the reactive region's Effect,
/// and that Effect would own this snapshot — a strong `Rc` here closes
/// a reference cycle that leaks the whole navigator. `enter()` upgrades;
/// if the navigator is gone (region tearing down) it simply restores
/// nothing.
#[derive(Clone, Default)]
pub struct AmbientNavContext {
    nav: Option<std::rc::Weak<NavigatorControl>>,
    // Outer Option = "was a screen-state guard present"; inner = the
    // state value (itself optional — `()`-param screens push `None`).
    state: Option<Option<Rc<dyn Any>>>,
    route: Option<&'static str>,
}

/// Capture the current ambient context. Call this synchronously while
/// building a reactive region (i.e. while the screen's guards are still
/// on the stack), BEFORE creating the rebuild Effect.
pub fn capture_ambient_nav_context() -> AmbientNavContext {
    AmbientNavContext {
        nav: AMBIENT_NAV.with(|s| s.borrow().last().map(Rc::downgrade)),
        state: SCREEN_STATE.with(|s| s.borrow().last().cloned()),
        route: SCREEN_ROUTE.with(|s| s.borrow().last().copied()),
    }
}

impl AmbientNavContext {
    /// True when there is no navigator context to restore — lets callers
    /// cheaply skip when used outside any navigator.
    pub fn is_empty(&self) -> bool {
        self.nav.is_none() && self.state.is_none() && self.route.is_none()
    }

    /// Re-push the captured context. The returned guard pops all three
    /// stacks on drop. Hold it across the subtree rebuild.
    pub fn enter(&self) -> AmbientNavContextGuard {
        AmbientNavContextGuard {
            _nav: self.nav.as_ref().and_then(|w| w.upgrade()).map(AmbientNavGuard::push),
            _state: self.state.clone().map(ScreenStateGuard::push),
            _route: self.route.map(ScreenRouteGuard::push),
        }
    }
}

/// Drops in field order; each inner guard pops its own (independent)
/// stack, so order is irrelevant for correctness.
pub struct AmbientNavContextGuard {
    _nav: Option<AmbientNavGuard>,
    _state: Option<ScreenStateGuard>,
    _route: Option<ScreenRouteGuard>,
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

/// Match `pattern` against the LEADING segments of `path`. Returns the
/// extracted `:placeholder` segments plus the unconsumed remainder of
/// `path` (a leading-slash string, or empty `""` when fully consumed).
/// `None` when a literal segment differs or `path` has fewer segments
/// than `pattern`.
///
/// This is the hierarchical primitive: a parent navigator matches its
/// route's pattern as a prefix and hands the `remainder` to the child
/// navigator nested in that screen (which prefix-matches in turn). A
/// full URL is resolved by peeling one prefix per level down the active
/// navigator tree. Trailing slashes are tolerated; empty path = `/`.
pub fn match_prefix(path: &str, pattern: &str) -> Option<(HashMap<String, String>, String)> {
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if path_segs.len() < pat_segs.len() {
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
    let remainder_segs = &path_segs[pat_segs.len()..];
    let remainder = if remainder_segs.is_empty() {
        String::new()
    } else {
        format!("/{}", remainder_segs.join("/"))
    };
    Some((out, remainder))
}

/// Match `path` against `pattern` requiring a FULL match (no leftover
/// segments). Returns `Some(map)` when segment counts agree and every
/// literal segment matches case-sensitively; `:placeholder` segments
/// become map entries. Thin wrapper over [`match_prefix`] that rejects
/// any non-empty remainder.
///
/// Trailing slashes are tolerated; empty path is treated as `/`.
pub fn match_pattern(path: &str, pattern: &str) -> Option<HashMap<String, String>> {
    match match_prefix(path, pattern) {
        Some((segs, remainder)) if remainder.is_empty() => Some(segs),
        _ => None,
    }
}

#[cfg(test)]
mod matcher_tests {
    use super::{join_path, match_pattern, match_prefix};

    #[test]
    fn join_path_composes_base_and_relative() {
        assert_eq!(join_path("", "/today"), "/today"); // root base
        assert_eq!(join_path("/encounters", "/abc"), "/encounters/abc");
        assert_eq!(join_path("/encounters", ""), "/encounters"); // index
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/encounters/", "abc"), "/encounters/abc"); // slash tolerance
        // Round-trip: compose then peel returns the relative remainder.
        let full = join_path("/encounters", "/abc");
        let (_, rem) = match_prefix(&full, "/encounters").expect("base prefix");
        assert_eq!(rem, "/abc");
    }

    fn seg(segs: &std::collections::HashMap<String, String>, k: &str) -> Option<String> {
        segs.get(k).cloned()
    }

    #[test]
    fn prefix_consumes_leading_segments_and_returns_remainder() {
        // Parent navigator owns `/encounters`; the child sees `/abc`.
        let (segs, rem) = match_prefix("/encounters/abc", "/encounters").expect("matches");
        assert!(segs.is_empty());
        assert_eq!(rem, "/abc");
    }

    #[test]
    fn prefix_extracts_placeholder_and_remainder() {
        let (segs, rem) = match_prefix("/encounters/abc/notes", "/encounters/:id").expect("matches");
        assert_eq!(seg(&segs, "id").as_deref(), Some("abc"));
        assert_eq!(rem, "/notes");
    }

    #[test]
    fn prefix_full_match_has_empty_remainder() {
        let (segs, rem) = match_prefix("/encounters/abc", "/encounters/:id").expect("matches");
        assert_eq!(seg(&segs, "id").as_deref(), Some("abc"));
        assert_eq!(rem, "");
    }

    #[test]
    fn prefix_rejects_shorter_path_and_literal_mismatch() {
        // Path shorter than pattern.
        assert!(match_prefix("/encounters", "/encounters/:id").is_none());
        // Literal segment differs.
        assert!(match_prefix("/patients/abc", "/encounters/:id").is_none());
    }

    #[test]
    fn pattern_requires_full_match() {
        // Exact match: ok.
        assert!(match_pattern("/encounters/abc", "/encounters/:id").is_some());
        // Leftover segments: rejected (this is the pattern-vs-prefix distinction).
        assert!(match_pattern("/encounters/abc/notes", "/encounters/:id").is_none());
        assert!(match_pattern("/encounters/abc", "/encounters").is_none());
    }

    #[test]
    fn two_level_descent() {
        // Root drawer matches `/encounters` prefix; nested stack matches the rest.
        let (root_segs, rem) = match_prefix("/encounters/abc", "/encounters").expect("root");
        assert!(root_segs.is_empty());
        // Child stack's detail route is `/encounters/:id` *relative to root base* —
        // but the child only ever sees the remainder `/abc`, so its route pattern,
        // expressed relative to the base, is matched against `/abc`.
        let (child_segs, child_rem) = match_prefix(&rem, "/:id").expect("child");
        assert_eq!(seg(&child_segs, "id").as_deref(), Some("abc"));
        assert_eq!(child_rem, "");
    }
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
    /// This navigator's URL prefix in the hierarchy (empty for the root).
    /// Route patterns are registered RELATIVE to this; `dispatch` composes
    /// `base + cmd.url` into the full hierarchical path that chrome and the
    /// platform URL see. Set once at build via [`set_base`](Self::set_base).
    base: RefCell<String>,
    /// Optional SDK-installed link activation builder. Maps the
    /// triple `(route_name, url, params)` to a `NavCommand`. The
    /// `Link` primitive calls this on activation to pick the right
    /// dispatch verb for the enclosing navigator — stack SDKs install
    /// one that builds `Push`; tab/drawer SDKs install one that builds
    /// `Select`. When not installed, `Link` defaults to `Push`.
    link_activator: RefCell<
        Option<Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand>>,
    >,
    /// Backend-provided "schedule a layout pass" hook, registered ONCE by the
    /// navigator walker (`|| B::schedule_layout_pass()`). `dispatch` calls it
    /// after every command so a freshly-mounted screen is always laid out — the
    /// guarantee lives here in the abstraction, not duplicated (and forgettable)
    /// in each navigator×backend handler. `None` until the walker registers it.
    request_layout: RefCell<Option<Box<dyn Fn()>>>,
    /// Reactive scope owning this navigator's `nav_state` signals (and any
    /// other framework-owned per-navigator reactive state). The control is
    /// the navigator's true lifetime anchor — it's an `Rc` held by the
    /// backend instance and the SDK handler, so it outlives the *transient*
    /// build scope that ran `build_navigator`.
    ///
    /// `nav_state` MUST be anchored here, not to the ambient build scope: a
    /// nested navigator (e.g. a stack hung under a drawer screen) is often
    /// built inside a short-lived dispatch/microtask scope. If `nav_state`
    /// were owned by that scope, its signals would be freed when the scope
    /// drops, and a later `active_route.set(...)` from `mount_internal` /
    /// `on_popstate` would hit a recycled arena slot — "signal used after
    /// its scope was dropped" / type-mismatch. Owning the scope here ties
    /// the signals to the navigator's real lifetime: freed when the control
    /// drops on navigator teardown (leak-free), never sooner.
    owning_scope: RefCell<Option<Box<crate::reactive::Scope>>>,
}

impl NavigatorControl {
    pub fn new() -> Self {
        Self {
            dispatch: RefCell::new(None),
            depth: RefCell::new(1),
            nav_state: RefCell::new(None),
            base: RefCell::new(String::new()),
            link_activator: RefCell::new(None),
            request_layout: RefCell::new(None),
            owning_scope: RefCell::new(None),
        }
    }

    /// Retain the reactive scope that owns this navigator's `nav_state`
    /// signals so they live for the control's lifetime, not the transient
    /// build scope's. Called once from `walker::navigator::build` right
    /// after the scope-anchored `nav_state` is constructed. See the
    /// `owning_scope` field doc for why this anchoring is required.
    pub(crate) fn retain_scope(&self, scope: Box<crate::reactive::Scope>) {
        *self.owning_scope.borrow_mut() = Some(scope);
    }

    /// Set this navigator's hierarchy base prefix. Called once at build
    /// from the navigator walker with [`current_nav_base`]. Empty for the
    /// root; e.g. `/encounters` for a stack nested under that drawer screen.
    pub fn set_base(&self, base: String) {
        *self.base.borrow_mut() = base;
    }

    /// This navigator's base prefix.
    pub fn base(&self) -> String {
        self.base.borrow().clone()
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

    /// Register the backend's "schedule a layout pass" hook. Called once by the
    /// navigator walker with `|| B::schedule_layout_pass()`. After this, every
    /// [`dispatch`](Self::dispatch) guarantees a layout pass — so no
    /// navigator×backend handler has to (and none can forget to).
    pub fn install_request_layout(&self, f: Box<dyn Fn()>) {
        *self.request_layout.borrow_mut() = Some(f);
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
        // Compose this navigator's base prefix onto the command's
        // (navigator-relative) url, so the nav-state mirror, chrome, and the
        // platform URL all see the full hierarchical path. For the root
        // navigator (base ""), `join_path("", url) == url` — a no-op, so a
        // single-navigator app is unaffected.
        let base = self.base.borrow().clone();
        let cmd = self.compose_url(&base, cmd);
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
        // Centralized layout guarantee: after the SDK handler commits the
        // command (mounts/swaps the screen), ensure a layout pass is scheduled.
        // This is the ONE place every navigation triggers a relayout, on every
        // backend — replacing the per-handler `schedule_layout_pass()` calls
        // that some backends had and others (Android stack) forgot.
        if let Some(f) = self.request_layout.borrow().as_ref() {
            f();
        }
    }

    /// Rebuild a command with `base + url` as its full hierarchical path.
    fn compose_url(&self, base: &str, cmd: NavCommand) -> NavCommand {
        match cmd {
            NavCommand::Push { name, url, params, state } => {
                NavCommand::Push { name, url: join_path(base, &url), params, state }
            }
            NavCommand::Replace { name, url, params, state } => {
                NavCommand::Replace { name, url: join_path(base, &url), params, state }
            }
            NavCommand::Reset { name, url, params, state } => {
                NavCommand::Reset { name, url: join_path(base, &url), params, state }
            }
            NavCommand::Select { name, url, params, state } => {
                NavCommand::Select { name, url: join_path(base, &url), params, state }
            }
            other => other,
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

/// Returns a function `() -> bool` that reads as `true` when the ambient
/// navigator has a screen to pop back to — i.e. the active screen is NOT the
/// root of its stack. Reactive: read it inside an `effect!` block (or any
/// reactive context) and it re-fires whenever the stack depth changes (push,
/// pop, or a native back gesture).
///
/// ```ignore
/// use runtime_core::primitives::navigator::use_can_go_back;
///
/// let can_go_back = use_can_go_back();
/// // e.g. show a root-only FAB while at the stack root:
/// presence(|| fab()).present(move || !can_go_back());
/// ```
///
/// **Prefer this over [`use_focus`] for "am I the root screen" gating that must
/// survive a native back.** `use_focus` keys off `active_route`, which the
/// framework updates on push/replace/reset but a bare `pop` leaves to the SDK
/// handler's `active_changed` — and the native stack handlers (macOS/iOS/
/// Android) don't all emit it, so `active_route` can read stale after a pop.
/// `can_go_back` is derived from `depth`, which every backend updates on BOTH
/// push and pop via `depth_changed`, so it stays correct.
///
/// Returns `|| false` when called outside a navigator scope (no ambient
/// navigator).
pub fn use_can_go_back() -> impl Fn() -> bool + 'static {
    // Capture the `can_go_back` signal at use-time — cheap `Rc` clone that
    // outlives the `NavigatorControl`, same as [`use_focus`].
    let sig = ambient_navigator()
        .and_then(|n| n.nav_state.borrow().as_ref().map(|s| s.can_go_back));
    move || match sig {
        Some(s) => s.get(),
        None => false,
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

/// Non-consuming PEEK of the headless initial-path override. Unlike
/// [`take_initial_path`], this clones and leaves the slot intact so that
/// EACH navigator in a synchronous (native/SSR) initial-mount cascade can
/// independently consult the same full deep-link URL and strip its own
/// base. The root navigator (detected via `current_nav_base().is_empty()`)
/// clears the slot with `set_initial_path(None)` once its whole subtree —
/// including any nested navigators — has finished mounting.
pub fn peek_initial_path() -> Option<String> {
    INITIAL_PATH.with(|p| p.borrow().clone())
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

#[cfg(test)]
mod nav_state_lifetime_tests {
    //! Regression: a navigator's `nav_state` signals must outlive the
    //! *transient* scope it was built in.
    //!
    //! A nested navigator (e.g. a stack hung under a drawer screen, reached
    //! via a sidebar `on_select`) is built inside a short-lived
    //! dispatch/microtask scope. The walker creates `nav_state` in a DEDICATED
    //! scope retained on the long-lived `NavigatorControl` (an `Rc`) rather
    //! than letting the ambient build scope own it. Before that fix, the
    //! ambient scope owned the signals; when it dropped, a later
    //! `active_route.set(...)` from `mount_internal` / `on_popstate` hit a
    //! freed arena slot and panicked — the QuillEMR forward/back nested-stack
    //! crash ("signal used after its scope was dropped" / type mismatch).

    use super::*;
    use crate::reactive::{with_scope, Scope, Signal};

    fn fresh_nav_state() -> NavState {
        NavState {
            active_route: Signal::new("home"),
            active_path: Signal::new("/".to_string()),
            depth: Signal::new(1),
            can_go_back: Signal::new(false),
        }
    }

    /// THE FIX: `nav_state` anchored to the control's retained scope survives
    /// the transient build scope dropping, and stays writable afterwards.
    #[test]
    fn nav_state_survives_transient_build_scope() {
        let control = NavigatorControl::new();

        // Build INSIDE a transient ambient scope, mirroring the walker: the
        // nav_state lives in its own scope handed to the control, never the
        // ambient one.
        let mut ambient = Box::new(Scope::new());
        let nav_state = with_scope(&mut ambient, || {
            let mut nav_scope = Box::new(Scope::new());
            let st = with_scope(&mut nav_scope, fresh_nav_state);
            control.retain_scope(nav_scope);
            st
        });
        control.attach_nav_state(nav_state.clone());

        // The transient build scope drops, as it does after the
        // dispatch/microtask that triggered the nested-nav build returns.
        drop(ambient);

        // Pre-fix this panicked "signal used after its scope was dropped".
        nav_state.active_route.set("detail");
        nav_state.active_path.set("/detail".to_string());
        assert_eq!(nav_state.active_route.get(), "detail");
        assert_eq!(nav_state.active_path.get(), "/detail");

        // Leak-free: dropping the control frees the retained scope (and with
        // it the nav_state signals). Just assert it doesn't panic.
        drop(control);
    }

    /// COUNTER-TEST pinning the bug: the OLD shape (nav_state owned by the
    /// ambient build scope) is a use-after-free once that scope drops.
    #[test]
    #[should_panic(expected = "signal used after its scope was dropped")]
    fn nav_state_owned_by_build_scope_is_use_after_free() {
        let mut ambient = Box::new(Scope::new());
        let nav_state = with_scope(&mut ambient, fresh_nav_state);
        drop(ambient); // frees the signals — the bug
        nav_state.active_route.set("detail"); // hits a freed slot → panic
    }
}

#[cfg(test)]
mod layout_pass_contract_tests {
    //! The navigator abstraction must schedule a layout pass after EVERY
    //! command, in ONE place — so no navigator×backend handler has to remember
    //! to (the recurring "navigated, but the new screen renders at 0×0" bug;
    //! the Android stack handler forgot it). The walker registers
    //! `|| B::schedule_layout_pass()` as the request-layout hook; this proves
    //! `dispatch` invokes it for every command shape, and that a backend which
    //! opts out (default no-op) is safe.
    use super::*;
    use std::cell::Cell;

    #[test]
    fn dispatch_requests_a_layout_pass_for_every_command() {
        let control = NavigatorControl::new();
        let count = Rc::new(Cell::new(0u32));
        control.install(Box::new(|_cmd| {})); // SDK handler: no-op
        let c = count.clone();
        control.install_request_layout(Box::new(move || c.set(c.get() + 1)));

        control.dispatch(NavCommand::Push {
            name: "a",
            url: "/a".into(),
            params: Box::new(()),
            state: None,
        });
        control.dispatch(NavCommand::Pop);
        control.dispatch(NavCommand::Replace {
            name: "b",
            url: "/b".into(),
            params: Box::new(()),
            state: None,
        });
        control.dispatch(NavCommand::Reset {
            name: "c",
            url: "/c".into(),
            params: Box::new(()),
            state: None,
        });
        control.dispatch(NavCommand::Select {
            name: "d",
            url: "/d".into(),
            params: Box::new(()),
            state: None,
        });
        control.dispatch(NavCommand::Custom(Rc::new(())));

        assert_eq!(
            count.get(),
            6,
            "every NavCommand must trigger exactly one centralized layout-pass request"
        );
    }

    #[test]
    fn no_hook_registered_is_a_safe_noop() {
        // A backend that re-layouts automatically (web reflow) never registers
        // the hook — `dispatch` must not panic when it's absent.
        let control = NavigatorControl::new();
        control.install(Box::new(|_cmd| {}));
        control.dispatch(NavCommand::Pop);
    }
}

#[cfg(test)]
mod use_can_go_back_tests {
    //! Regression: `use_can_go_back` must track the `depth`-derived
    //! `can_go_back` signal — which every backend updates on push AND pop via
    //! `depth_changed` — NOT `active_route`, which native stack handlers leave
    //! stale after a bare `pop`. The whiteboard-demo gates its capture-excluded
    //! board chrome on this: a stale read would leave the toolbar hidden forever
    //! after returning from a pushed screen ("the private layer goes missing").

    use super::*;
    use crate::reactive::{with_scope, Scope};

    fn control_with_state() -> (Rc<NavigatorControl>, NavState, Box<Scope>) {
        let control = Rc::new(NavigatorControl::new());
        let mut nav_scope = Box::new(Scope::new());
        let nav_state = with_scope(&mut nav_scope, || NavState {
            active_route: crate::Signal::new("board"),
            active_path: crate::Signal::new("/".to_string()),
            depth: crate::Signal::new(1),
            can_go_back: crate::Signal::new(false),
        });
        control.attach_nav_state(nav_state.clone());
        (control, nav_state, nav_scope)
    }

    #[test]
    fn tracks_can_go_back_across_push_and_pop() {
        let (control, nav_state, _scope) = control_with_state();
        let _guard = AmbientNavGuard::push(control.clone());

        let can_go_back = use_can_go_back();
        // At the stack root: nothing to pop back to.
        assert!(!can_go_back(), "root screen: can_go_back is false");

        // Push a screen (depth 2): now there's a back target.
        nav_state.depth.set(2);
        nav_state.can_go_back.set(true);
        assert!(can_go_back(), "after push: can_go_back is true");

        // Pop back to the root (depth 1). This is the case `active_route` would
        // read stale on native handlers — `can_go_back` must flip back.
        nav_state.depth.set(1);
        nav_state.can_go_back.set(false);
        assert!(!can_go_back(), "after pop to root: can_go_back is false again");
    }

    #[test]
    fn false_without_an_ambient_navigator() {
        // No `AmbientNavGuard` in scope → no navigator → reads false.
        let can_go_back = use_can_go_back();
        assert!(!can_go_back());
    }
}
