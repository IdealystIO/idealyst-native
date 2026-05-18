//! Shared navigator substrate — used by every navigator kind.
//!
//! This module owns the bits that don't depend on whether the
//! enclosing navigator is a stack, tabs, drawer, or anything else
//! we might ship later:
//!
//! - `Route<P>` + `RouteParams` — typed route declaration and URL
//!   <-> typed-params conversion.
//! - `ScreenBuilder` / `RouteEntry` / `ParamsFromSegments` —
//!   type-erased screen registry.
//! - `match_pattern` — pure URL-against-pattern matcher (used by
//!   web + any future SSR backend).
//! - `AmbientNavGuard` / `ambient_navigator()` — the thread-local
//!   stack of `Rc<NavigatorControl>` the `Link` primitive reads at
//!   construction time. Each navigator kind pushes onto this stack
//!   while building screens.
//! - `NavCommand` — the dispatcher's command vocabulary. Stack
//!   commands (`Push`, `Pop`, `Replace`, `Reset`) and select-style
//!   commands (`Select`) coexist; per-kind dispatchers handle the
//!   ones they understand.
//! - `NavigatorControl` — the bridge between the user-facing
//!   handle and the framework's per-navigator state. Carries the
//!   dispatcher closure, the reactive `NavState` mirror, and the
//!   depth probe.
//! - `NavigatorHandle` + `NavigatorOps` — the imperative API and
//!   its backend hook trait.
//! - `NavigatorCallbacks` — the bundle handed to
//!   `Backend::create_navigator` (and, eventually, the per-kind
//!   `create_tab_navigator` / `create_drawer_navigator` siblings).
//! - `LayoutProps` / `LayoutPlan` / `LayoutBuilder` — author-supplied
//!   chrome wrapper for backends that render through a layout (web).
//! - `NavState` — reactive bundle of "what's the active screen?"
//!   signals layout closures subscribe to.
//!
//! Per-kind builders (`Navigator` in `stack`, future `TabNavigator`
//! / `DrawerNavigator`) compose on top of these.

use crate::{Primitive, Ref};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Ambient navigator stack
// ---------------------------------------------------------------------------
//
// While a navigator's `mount_screen` is building a screen subtree,
// it pushes its `Rc<NavigatorControl>` onto this thread-local stack
// and pops on the way out. The `Link` primitive's constructor reads
// the top of the stack at build time and captures it — that's how
// `Link(route, params)` finds the right navigator without an
// explicit ref.
//
// Nested navigators stack naturally: a `Link` inside a child
// navigator's screen sees the child's control plane on top.
// Authors who want to break out can capture a parent's nav handle
// explicitly via a future `.via(ref)` override.

thread_local! {
    static AMBIENT_NAV: RefCell<Vec<Rc<NavigatorControl>>> =
        const { RefCell::new(Vec::new()) };
}

/// RAII guard that pushes a navigator control onto the ambient
/// stack at construction and pops on drop. Used by
/// `mount_screen` to wrap each per-screen build.
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

/// Read the top of the ambient-navigator stack. Returns `None`
/// when called outside any navigator's `mount_screen`. The `Link`
/// primitive captures this at build time and dispatches through
/// the captured control plane on activation.
pub fn ambient_navigator() -> Option<Rc<NavigatorControl>> {
    AMBIENT_NAV.with(|s| s.borrow().last().cloned())
}

// ---------------------------------------------------------------------------
// RouteParams — URL <-> typed params conversion
// ---------------------------------------------------------------------------

/// Convert route params to/from URL path segments. Implemented on
/// every type used as a `Route<P>` payload. Built-in impl for `()` (the
/// no-params case). For custom types, authors implement this trait
/// directly — usually a few-line affair.
///
/// # Why this trait exists
///
/// The web backend (and any future SSR backend) needs to reconcile a
/// URL like `/detail/42` with the typed `DetailParams { id: 42 }` the
/// rest of the framework speaks. The trait moves that conversion into
/// the params type itself, keeping path-pattern handling (which is
/// pure-Rust string matching) reusable across backends and SSR.
///
/// Native backends (iOS, Android) don't touch URLs at all — the param
/// payload flows as `Box<dyn Any>` directly to the receiving screen.
/// The trait is still required because the framework needs *any* path
/// rendering to work uniformly when the user wires up a web view
/// alongside native (a future use case).
pub trait RouteParams: 'static + Sized {
    /// Render `self` into URL path segments for a route whose pattern
    /// is `pattern` (e.g. `/detail/:id`). Returns the concrete URL
    /// path (e.g. `/detail/42`).
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

    /// Parse `self` from a `:placeholder` -> value map. The map is
    /// populated by the framework after matching the URL against the
    /// route's pattern. Returns `None` on parse failure (a path that
    /// matched the pattern but had unparseable values for the
    /// declared `P`).
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
// Route<P> — typed route name + phantom param type
// ---------------------------------------------------------------------------

/// A navigation route. The `name` is the in-stack key (used by native
/// backends + framework); the `path` is the URL pattern used by web
/// (and any future SSR / pathing backend). The phantom `P` is what
/// `Navigator::push` / `.screen` type-check against so the params
/// can't drift from the route.
///
/// # Path pattern syntax
///
/// Patterns are slash-delimited segments. A segment of the form
/// `:name` is a parameter placeholder filled at push time. Everything
/// else is matched literally.
///
///   `/`              — root
///   `/settings`      — literal
///   `/detail/:id`    — single param
///   `/u/:user/p/:post` — two params
///
/// Native backends (iOS, Android) ignore `path`. The framework's
/// renderer never inspects it either — it's data the web backend (and
/// future SSR backend) reads.
#[derive(Clone)]
pub struct Route<P: RouteParams = ()> {
    name: &'static str,
    path: &'static str,
    _params: PhantomData<P>,
}

impl<P: RouteParams> Route<P> {
    /// Declare a route. `name` must be unique across the navigator's
    /// screen table; `path` is the URL pattern (see [`Route`] doc).
    /// A param-less route uses `Route::<()>::new("home", "/")`; a
    /// route with a param payload uses
    /// `Route::<MyParams>::new("detail", "/detail/:id")`.
    pub const fn new(name: &'static str, path: &'static str) -> Self {
        Self { name, path, _params: PhantomData }
    }

    /// The route's stable name. Used as the navigator's screen table
    /// key, and (on native) passed as-is through commands.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The route's URL path pattern. Used by web for matching
    /// `window.location` and constructing `history.pushState` URLs.
    /// Native backends ignore this.
    pub fn path(&self) -> &'static str {
        self.path
    }
}

// ---------------------------------------------------------------------------
// ScreenBuilder + RouteEntry — type-erased renderer + path-match data
// ---------------------------------------------------------------------------

/// Per-route builder closure. Takes the boxed params and returns the
/// screen's `Primitive` subtree.
///
/// Param downcasting happens here. If the framework dispatches the
/// wrong concrete type for a route (only possible if user code
/// fabricates a `Route<X>` at runtime with the wrong `P`), the
/// downcast panics with a clear message — same posture as any other
/// type-erased registry in the framework.
pub type ScreenBuilder = Rc<dyn Fn(Box<dyn Any>) -> Screen>;

/// Closure that parses a `:placeholder` segment map into the
/// route's typed param payload, then boxes it as `dyn Any`. Used by
/// path-matching backends (web, future SSR) to go from a matched URL
/// to the params the receiving screen expects.
pub type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

/// Per-route bookkeeping. Carries everything path-matching backends
/// need: the pattern, the typed builder, and the segment-parser. The
/// framework's screen table maps route names to these entries.
///
/// Per-screen header config (title, bar buttons, etc.) is *not* a
/// separate field — the route's render closure returns a [`Screen`]
/// which bundles both the UI tree and its options into one value.
/// This keeps screens self-contained: one route, one declaration.
pub struct RouteEntry {
    pub path: &'static str,
    pub build: ScreenBuilder,
    pub from_segments: ParamsFromSegments,
}

// ---------------------------------------------------------------------------
// ScreenOptions — per-screen header configuration
// ---------------------------------------------------------------------------

/// Per-screen header configuration. Backends read this at mount time
/// to configure platform-native chrome (UINavigationBar on iOS,
/// MaterialToolbar on Android). All fields `Option` — `None` means
/// "inherit from navigator defaults."
///
/// Color fields are `Rc<dyn Fn() -> Color>` closures, not plain
/// `Color`. The closure is invoked at apply time, and backends
/// re-invoke it whenever the active theme changes — so if the closure
/// reads `crate::active_theme()` (directly or through a helper like
/// idea-ui's `idea_color`), the header re-tints reactively on theme
/// swap. For a static color, wrap the literal: `|| my_color.clone()`.
#[derive(Clone, Default)]
pub struct ScreenOptions {
    /// Screen title shown in the header bar.
    pub title: Option<String>,
    /// Whether the header is visible. Default `true`.
    pub header_shown: Option<bool>,
    /// Left header slot (replaces default back button if set).
    pub header_left: Option<HeaderButton>,
    /// Right header slot (action button area).
    pub header_right: Option<HeaderButton>,
    /// Header bar background fill. `None` keeps the platform default
    /// (iOS opaque white, Android opaque white).
    pub header_background: Option<Rc<dyn Fn() -> crate::Color>>,
    /// Header bar tint — applied to back chevron and any
    /// `header_left`/`header_right` icons that don't carry their own
    /// `HeaderButton::tint`. `None` keeps the platform default.
    pub header_tint: Option<Rc<dyn Fn() -> crate::Color>>,
    /// Title text color. `None` keeps the platform default.
    pub title_color: Option<Rc<dyn Fn() -> crate::Color>>,
}

impl ScreenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn header_shown(mut self, shown: bool) -> Self {
        self.header_shown = Some(shown);
        self
    }

    pub fn header_left(mut self, btn: HeaderButton) -> Self {
        self.header_left = Some(btn);
        self
    }

    pub fn header_right(mut self, btn: HeaderButton) -> Self {
        self.header_right = Some(btn);
        self
    }

    pub fn header_background<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.header_background = Some(Rc::new(f));
        self
    }

    pub fn header_tint<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.header_tint = Some(Rc::new(f));
        self
    }

    pub fn title_color<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.title_color = Some(Rc::new(f));
        self
    }

    /// Merge `other` on top of `self`: fields set in `other` override
    /// the corresponding fields in `self`. Used to layer per-screen
    /// options on top of navigator defaults.
    pub fn merge(mut self, other: &ScreenOptions) -> Self {
        if other.title.is_some() { self.title = other.title.clone(); }
        if other.header_shown.is_some() { self.header_shown = other.header_shown; }
        if other.header_left.is_some() { self.header_left = other.header_left.clone(); }
        if other.header_right.is_some() { self.header_right = other.header_right.clone(); }
        if other.header_background.is_some() { self.header_background = other.header_background.clone(); }
        if other.header_tint.is_some() { self.header_tint = other.header_tint.clone(); }
        if other.title_color.is_some() { self.title_color = other.title_color.clone(); }
        self
    }
}

// ---------------------------------------------------------------------------
// Screen — what a route's render closure returns: content + options
// ---------------------------------------------------------------------------

/// A renderable screen: the UI tree plus its per-screen
/// configuration (title, header buttons, etc.). The render closure
/// passed to `Navigator::screen(...)` (and every other navigator
/// kind) returns one of these — replacing the old pattern of a
/// separate `.options(route, fn)` call sidecared onto the builder.
///
/// `impl From<Primitive> for Screen` keeps the no-options ergonomic
/// form working: `.screen(R, |_| pages::home().into())` (or
/// `.screen(R, |_| pages::home())` when the closure signature accepts
/// `impl Into<Screen>`).
pub struct Screen {
    pub primitive: Primitive,
    pub options: ScreenOptions,
}

impl Screen {
    pub fn new(primitive: impl Into<Primitive>) -> Self {
        Self {
            primitive: primitive.into(),
            options: ScreenOptions::default(),
        }
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.options.title = Some(t.into());
        self
    }

    pub fn header_shown(mut self, shown: bool) -> Self {
        self.options.header_shown = Some(shown);
        self
    }

    pub fn header_left(mut self, btn: HeaderButton) -> Self {
        self.options.header_left = Some(btn);
        self
    }

    pub fn header_right(mut self, btn: HeaderButton) -> Self {
        self.options.header_right = Some(btn);
        self
    }

    pub fn header_background<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.options.header_background = Some(Rc::new(f));
        self
    }

    pub fn header_tint<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.options.header_tint = Some(Rc::new(f));
        self
    }

    pub fn title_color<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        self.options.title_color = Some(Rc::new(f));
        self
    }
}

impl From<Primitive> for Screen {
    fn from(p: Primitive) -> Self {
        Self::new(p)
    }
}

/// Bundle of header colors for a navigator-level
/// `.header(...)` call. Each field is optional — `None` keeps the
/// platform default for that slot.
///
/// Use with [`Bound::<DrawerHandle>::header`] (and the analogous
/// helper on other navigator kinds) so a single closure configures
/// the whole bar at once. For per-slot overrides at the screen
/// level, set the corresponding fields on [`Screen`] directly.
#[derive(Default, Clone)]
pub struct HeaderStyle {
    pub background: Option<crate::Color>,
    pub title: Option<crate::Color>,
    pub tint: Option<crate::Color>,
    pub body_background: Option<crate::Color>,
}

/// An icon button for a header slot (left or right).
#[derive(Clone)]
pub struct HeaderButton {
    /// Icon name — SF Symbol on iOS (e.g. "line.3.horizontal"),
    /// Material icon name on Android (e.g. "menu").
    pub icon: String,
    /// Action when the button is pressed.
    pub on_press: Rc<dyn Fn()>,
    /// Override tint for this specific button. `None` = use the
    /// header style's `tint_color`.
    pub tint: Option<crate::Color>,
}

impl HeaderButton {
    pub fn new(icon: impl Into<String>, on_press: impl Fn() + 'static) -> Self {
        Self {
            icon: icon.into(),
            on_press: Rc::new(on_press),
            tint: None,
        }
    }

    pub fn tint(mut self, color: crate::Color) -> Self {
        self.tint = Some(color);
        self
    }
}

/// Result of mounting a screen. Returned by
/// `NavigatorCallbacks.mount_screen` to give backends both the
/// native node and the resolved header options.
pub struct MountResult<N> {
    pub node: N,
    pub scope_id: u64,
    pub options: ScreenOptions,
}

// ---------------------------------------------------------------------------
// Path matching — pure-Rust, used by web + future SSR backends
// ---------------------------------------------------------------------------

/// Match `path` against `pattern`. Returns `Some(map)` if the segment
/// counts agree and every literal segment matches (case-sensitively);
/// `:placeholder` segments end up as entries in the returned map.
/// Returns `None` on a mismatch.
///
/// Trailing slashes are tolerated on both sides (treated as
/// equivalent). Empty path is treated as `/`.
///
/// Pure function — no DOM access, no JS APIs. Ports unchanged to a
/// future SSR backend.
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
// NavigatorHandle — the imperative API exposed via .bind(...)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NavigatorHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn NavigatorOps,
    /// Shared with the running navigator's state. Cloning the handle
    /// re-uses the same control plane — multiple owners drive the same
    /// stack. None when the handle is a no-op (the trait default).
    control: Option<Rc<NavigatorControl>>,
}

impl NavigatorHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn NavigatorOps) -> Self {
        Self { node, ops, control: None }
    }

    /// Construct a handle wired to a control plane. Used by backends
    /// that actually drive the navigator (web, iOS, Android impls).
    pub fn with_control(
        node: Rc<dyn Any>,
        ops: &'static dyn NavigatorOps,
        control: Rc<NavigatorControl>,
    ) -> Self {
        Self { node, ops, control: Some(control) }
    }

    /// Access the underlying control plane. Used by the per-kind
    /// handle wrappers (`StackHandle`, `TabsHandle`, `DrawerHandle`)
    /// to dispatch their kind-specific commands without re-implementing
    /// the wiring.
    pub fn control(&self) -> Option<&Rc<NavigatorControl>> {
        self.control.as_ref()
    }

    /// Push a new screen onto the stack. `route` is what was declared
    /// in `.screen(...)`; `params` must match the route's `P`. If the
    /// route is not registered, panics — declaring routes up-front is
    /// part of the contract.
    ///
    /// On web, this also calls `history.pushState` with the URL
    /// produced by `params.to_path(route.path())`. On native backends,
    /// the URL is computed but unused.
    pub fn push<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Push {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_pushed(&*self.node, route.name);
        }
    }

    /// Pop the top screen. No-op when the stack has only the root
    /// screen (matches platform behavior — iOS won't pop the root VC,
    /// Android's FragmentManager won't pop an empty back stack).
    ///
    /// On web, this calls `history.back()`, which fires `popstate`;
    /// the web backend's popstate handler then performs the stack
    /// pop. Native backends pop via their native API immediately.
    pub fn pop(&self) {
        if let Some(c) = &self.control {
            c.dispatch(NavCommand::Pop);
            self.ops.notify_popped(&*self.node);
        }
    }

    /// Replace the top screen without changing stack depth. Equivalent
    /// to pop + push for state but skips the platform's push/pop
    /// animation. On web, uses `history.replaceState`.
    pub fn replace<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Replace {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_replaced(&*self.node, route.name);
        }
    }

    /// Clear the entire stack and mount `route` as the new root.
    /// Useful for post-login redirects. On web, this is a single
    /// `history.replaceState` (we don't `pushState` because there's
    /// nothing above to navigate back to).
    pub fn reset<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = &self.control {
            let url = params.to_path(route.path);
            c.dispatch(NavCommand::Reset {
                name: route.name,
                url,
                params: Box::new(params),
            });
            self.ops.notify_reset(&*self.node, route.name);
        }
    }

    /// Current stack depth (1 = only root). Cheap.
    pub fn depth(&self) -> usize {
        self.control.as_ref().map(|c| c.depth()).unwrap_or(0)
    }
}

/// Optional per-backend operations on a navigator node. Backends that
/// only need the command stream (web, iOS, Android) leave most of
/// these as no-ops — the real work happens inside the control plane's
/// dispatch closure, which the backend installs when building the
/// navigator. The notifiers are here for backends that want to know
/// the operation happened without re-implementing the command queue.
pub trait NavigatorOps {
    fn notify_pushed(&self, _node: &dyn Any, _route: &str) {}
    fn notify_popped(&self, _node: &dyn Any) {}
    fn notify_replaced(&self, _node: &dyn Any, _route: &str) {}
    fn notify_reset(&self, _node: &dyn Any, _route: &str) {}
}

// ---------------------------------------------------------------------------
// Control plane — shared state between handle + framework + backend
// ---------------------------------------------------------------------------

/// Default activation shape exposed to the `Link` primitive. Stack
/// navigators expose `Push`; tabs and drawer expose `Select`. The
/// `Link` constructor reads this off the ambient navigator's
/// control plane when no explicit `.kind(...)` was specified.
///
/// Kept here (in the shared substrate, not in `link.rs`) so the
/// link module stays free of per-kind knowledge — it just queries
/// the ambient navigator's default and dispatches.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DefaultLinkKind {
    Push,
    Select,
}

impl Default for DefaultLinkKind {
    fn default() -> Self {
        DefaultLinkKind::Push
    }
}

/// The bridge between the user-facing handle and the framework's
/// per-navigator state. Carries:
/// - the command dispatcher the handle uses (set by the framework
///   during `build_navigator`),
/// - a read-only depth probe so handles can answer `.depth()` without
///   reaching into the backend,
/// - the reactive nav-state mirror (active route/path/depth/can_go_back),
/// - the navigator's default `Link` activation kind, so a `Link`
///   inside a tab/drawer navigator's screen dispatches `Select`
///   instead of `Push` by default.
///
/// Wrapped in `Rc` so handle clones share one control plane.
pub struct NavigatorControl {
    dispatch: RefCell<Option<Box<dyn Fn(NavCommand)>>>,
    depth: RefCell<usize>,
    /// Reactive nav-state mirror. Updated *before* the backend's
    /// dispatcher runs, so by the time effects re-fire the new
    /// route name + path are already observable. `None` until
    /// `attach_nav_state` is called from `build_navigator`.
    nav_state: RefCell<Option<NavState>>,
    /// Default dispatch shape for `Link` primitives whose ambient
    /// navigator is this one. Stack navigators leave it at `Push`
    /// (the default); tabs and drawer set it to `Select` via
    /// `set_default_link_kind` during their build phase.
    default_link_kind: RefCell<DefaultLinkKind>,
}

impl NavigatorControl {
    pub fn new() -> Self {
        Self {
            dispatch: RefCell::new(None),
            depth: RefCell::new(1),
            nav_state: RefCell::new(None),
            default_link_kind: RefCell::new(DefaultLinkKind::Push),
        }
    }

    /// Wire the nav-state signals. Called once from `build_navigator`
    /// before [`install`] so the dispatch path can update signals
    /// from the very first command.
    pub fn attach_nav_state(&self, nav_state: NavState) {
        *self.nav_state.borrow_mut() = Some(nav_state);
    }

    /// Set the default activation shape exposed to `Link` primitives.
    /// Called once from each per-kind builder's `build_*` walker.
    pub fn set_default_link_kind(&self, kind: DefaultLinkKind) {
        *self.default_link_kind.borrow_mut() = kind;
    }

    /// Read the default activation shape. The `Link` primitive
    /// constructor calls this to pick its initial `NavKind` based on
    /// the ambient navigator.
    pub fn default_link_kind(&self) -> DefaultLinkKind {
        *self.default_link_kind.borrow()
    }

    /// Install the dispatcher. Called once from `build_navigator` after
    /// the backend's command-execution closure is wired up.
    pub fn install(&self, dispatch: Box<dyn Fn(NavCommand)>) {
        *self.dispatch.borrow_mut() = Some(dispatch);
    }

    /// Update the cached depth. Called by the framework when commands
    /// commit so `handle.depth()` stays in sync.
    pub fn set_depth(&self, d: usize) {
        *self.depth.borrow_mut() = d;
    }

    pub fn depth(&self) -> usize {
        *self.depth.borrow()
    }

    /// Programmatic pop, equivalent to `handle.pop()`. Exposed
    /// because layout closures hold an `Rc<NavigatorControl>` (via
    /// `LayoutProps::on_back`) but don't have direct access to a
    /// `NavigatorHandle` — they need a way to invoke pop without
    /// passing a handle around separately.
    pub fn pop(&self) {
        self.dispatch(NavCommand::Pop);
    }

    /// Dispatch a `NavCommand` against this navigator. Public so
    /// the `Link` primitive (which captures an `Rc<NavigatorControl>`
    /// from the ambient stack at construction time) can route
    /// activations through here without going through a
    /// `NavigatorHandle`.
    pub fn dispatch(&self, cmd: NavCommand) {
        // Update the reactive nav-state mirror *before* the backend
        // sees the command. Effects that read these signals will
        // re-fire as the backend processes the command, so by the
        // time animations / fragment transactions finish, any
        // layout chrome that reads active_route/path is already
        // pointed at the new screen.
        if let Some(state) = self.nav_state.borrow().as_ref() {
            match &cmd {
                NavCommand::Push { name, url, .. }
                | NavCommand::Replace { name, url, .. }
                | NavCommand::Reset { name, url, .. }
                | NavCommand::Select { name, url, .. } => {
                    state.active_route.set(name);
                    state.active_path.set(url.clone());
                }
                NavCommand::Pop => {
                    // Pop's new active route/path is the entry one
                    // below the popped one — but the framework
                    // doesn't track per-entry route/path here. The
                    // backend, after committing the pop, updates
                    // active_route/path via depth_changed +
                    // (TODO) a separate "active_changed" callback.
                }
                NavCommand::OpenDrawer
                | NavCommand::CloseDrawer
                | NavCommand::ToggleDrawer => {
                    // Drawer open/close is transient UI state, not
                    // navigation — leave the active-route signals
                    // alone. The drawer navigator's dispatcher
                    // tracks the open-state signal separately.
                }
            }
        }

        // If the handle is somehow called before the navigator has
        // been built, just drop the command — same posture as a `Ref`
        // that's been used before the matching primitive has mounted.
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

/// Commands that flow from the handle into the framework's dispatcher.
/// Boxed params survive the channel hop and are downcast at the
/// builder boundary. `url` is the concrete URL string produced by
/// `RouteParams::to_path` — web pushes it; native backends ignore it.
///
/// Not every navigator kind handles every command:
///
/// - Stack handles `Push` / `Pop` / `Replace` / `Reset`. `Select` and
///   drawer commands are programmer errors against a stack nav.
/// - Tabs handles `Select` (and `Reset` to "go back to initial tab").
///   `Push` / `Pop` against a tab nav is a programmer error.
/// - Drawer handles `Select` + `OpenDrawer` / `CloseDrawer` /
///   `ToggleDrawer`.
///
/// Per-kind dispatchers panic on unsupported commands so the mismatch
/// surfaces at the call site, not as silent no-op.
pub enum NavCommand {
    // ----- stack-shaped -----
    Push {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    Pop,
    Replace {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    Reset {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    // ----- select-shaped (tabs + drawer) -----
    /// Switch the active screen to `name`. Used by tabs and drawer.
    /// Idempotent: selecting the already-active route is a no-op
    /// (matches React Navigation's tab tap-to-pop behavior, modulo
    /// the optional "reset stack on re-tap" we may add later).
    Select {
        name: &'static str,
        url: String,
        params: Box<dyn Any>,
    },
    // ----- drawer-shaped -----
    OpenDrawer,
    CloseDrawer,
    ToggleDrawer,
}

// ---------------------------------------------------------------------------
// NavigatorCallbacks — what the framework hands to the backend
// ---------------------------------------------------------------------------

/// Bundle the framework hands to `Backend::create_navigator`. Same
/// shape philosophy as `VirtualizerCallbacks`: typed where possible
/// (`mount_screen` returns the backend's actual `N`), `Rc`'d so
/// per-event handlers can clone freely.
pub struct NavigatorCallbacks<N: Clone + 'static> {
    /// The declared initial route (name + path). Backends that don't
    /// do path-matching (iOS, Android) mount this directly. The web
    /// backend (and any SSR backend) may instead use [`match_path`] to
    /// resolve the current URL against the registered routes; if the
    /// URL matches a non-initial route, the web backend mounts that
    /// route as the root and the initial route is bypassed.
    pub initial_route: &'static str,
    /// Initial route's path pattern (always param-less, see
    /// [`Navigator::new`]). Web uses this to determine whether the
    /// current URL "is" the initial route.
    pub initial_path: &'static str,
    /// Mount a screen for `name` with `params`. The framework builds
    /// the subtree inside a fresh per-screen `Scope` and returns the
    /// resulting native node plus the scope id so the backend can
    /// later release the same screen by id.
    pub mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<N>>,
    /// Release a previously-mounted screen by scope id. Drops the
    /// screen's `Scope`, freeing every signal/effect/ref inside. The
    /// backend should *not* use the node after this and should also
    /// detach it from its parent.
    pub release_screen: Rc<dyn Fn(u64)>,
    /// Match a URL path against the registered routes. Returns
    /// `Some((name, boxed_params))` if `path` matches one of the
    /// declared routes' patterns AND the matched segments can be
    /// parsed into the route's typed params; `None` otherwise.
    ///
    /// Used by the web backend on mount (deep linking) and on
    /// `popstate` (forward/back button); future SSR backends use it
    /// to map an HTTP request path to a screen subtree without ever
    /// running a JS dispatcher.
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    /// Build the layout subtree, if one was registered via
    /// `.layout(...)`. The framework constructs the outlet
    /// placeholder + reactive signal bundle (see `LayoutProps`),
    /// invokes the user's closure, *and runs the build walker* —
    /// so the returned `LayoutPlan::root` is already a native node
    /// the backend can insert into its container. The `outlet_ref`
    /// resolves to the outlet's native node via
    /// `Ref::get().node()`-style probes once the build completes.
    ///
    /// Backends that want to render through a layout (web) call
    /// this once at create time; native backends never call it.
    /// `None` means the author didn't supply a layout — backends
    /// fall back to their default "stack the screens directly in
    /// the navigator container" behavior.
    pub build_layout: Option<Rc<dyn Fn() -> LayoutPlan<N>>>,
    /// Reactive signals the dispatcher updates on every commit.
    /// Backends that build the layout subtree pass these into the
    /// layout closure via `LayoutProps`; the layout can subscribe
    /// to whichever it cares about. Signals exposed here are the
    /// same instances `LayoutProps` carries, so the user can
    /// `.set(...)` from layout effects if they need to (advanced).
    pub nav_state: NavState,
    /// Subscribe to commands from the handle. The backend installs a
    /// dispatcher here (see `Backend::create_navigator` doc); when the
    /// user's code calls `handle.push(...)`, that dispatcher fires.
    /// The backend's dispatcher must:
    ///   1. Call `mount_screen(name, params)` to get the new node +
    ///      scope id (for push / replace / reset).
    ///   2. Insert the node into its native container (push child VC,
    ///      replace fragment, etc.).
    ///   3. Call `release_screen` for any screen it pops.
    ///   4. Notify `depth_changed(new_depth)` so the handle's depth
    ///      probe stays in sync.
    pub depth_changed: Rc<dyn Fn(usize)>,
    /// When `true`, backends that would normally auto-mount the
    /// initial route at `create_navigator` time MUST NOT call
    /// `mount_screen` themselves. Initial mounting comes
    /// exclusively through [`Backend::navigator_attach_initial`]
    /// with a pre-built screen node.
    ///
    /// Set by the AAS dev-client's stub callbacks (the wire's
    /// `NavigatorAttachInitial` carries the canonical screen + scope,
    /// and re-mounting locally via URL match would produce a
    /// duplicate / wrong tree). Left `false` by the framework's
    /// normal navigator builder so local-mode backends keep their
    /// existing deep-link auto-mount behavior unchanged.
    pub defer_initial_mount: bool,
}

// ---------------------------------------------------------------------------
// Layout — author-supplied chrome wrapper, used by web (and any future
// SSR / DOM-based backend). Native backends ignore it.
// ---------------------------------------------------------------------------

/// Props handed to the user's layout closure. The author renders a
/// chrome subtree (top bar / sidebar / breadcrumbs / whatever) and
/// drops `outlet` somewhere in it — the framework physically swaps
/// the outlet's child to the active screen on each push/pop.
///
/// All state fields are reactive `Signal`s, so reading them inside a
/// `derived(...)` or directly via `.get()` subscribes the enclosing
/// effect — the layout (or just parts of it) re-renders on
/// navigation without the framework rebuilding the whole layout.
///
/// Backends that don't render through the layout (iOS, Android)
/// never invoke the layout closure; it's a pure web/DOM concept.
/// Putting layout logic in user code lets the author choose chrome
/// — a fixed top bar, a sidebar with breadcrumbs, a centered card
/// stack — without the framework dictating any of it.
pub struct LayoutProps {
    /// Where the active screen renders. Embed this in your tree
    /// exactly once — the framework owns the underlying node and
    /// swaps its child on push/pop.
    pub outlet: Primitive,
    /// Pre-built sidebar subtree.
    ///
    /// - For a `DrawerNavigator` with a `.sidebar(...)` registered,
    ///   this is the rendered sidebar Primitive. Drop it somewhere
    ///   in your layout subtree (typically a flex row beside the
    ///   `outlet`).
    /// - For a stack/tab navigator, or a drawer without
    ///   `.sidebar(...)`, this is an empty `View` — embed it
    ///   anyway (or ignore it). Always `Some`-equivalent so layout
    ///   authors don't have to write a None-case branch.
    pub sidebar: Primitive,
    /// Name of the currently-active route (matches `Route::name()`).
    /// Useful for a top-bar title or to highlight a tab.
    pub active_route: crate::Signal<&'static str>,
    /// Concrete URL path of the currently-active screen (e.g.
    /// `/detail/42`). Distinct from `active_route` — that's the
    /// route's stable name, this is the rendered URL.
    pub active_path: crate::Signal<String>,
    /// Current stack depth (1 = only root). Useful for hiding the
    /// back button when there's nowhere to go.
    pub depth: crate::Signal<usize>,
    /// `depth > 1`, exposed separately so the back button's reactive
    /// visibility doesn't need a `derived(...)` wrapper.
    pub can_go_back: crate::Signal<bool>,
    /// Pop the top screen. Same as calling `handle.pop()` on the
    /// navigator's handle. Lives here for convenience so layout
    /// code doesn't need to hold a separate handle.
    pub on_back: Rc<dyn Fn()>,
}

pub type LayoutBuilder = Rc<dyn Fn(LayoutProps) -> Primitive>;

/// Reactive nav-state bundle exposed to layout closures + held by
/// the framework. The dispatcher mutates these on every push/pop;
/// any effect reading them re-runs to reflect the new state.
///
/// Cloning is cheap — every field is a `Signal<T>` which is a
/// numeric arena handle.
#[derive(Clone)]
pub struct NavState {
    pub active_route: crate::Signal<&'static str>,
    pub active_path: crate::Signal<String>,
    pub depth: crate::Signal<usize>,
    pub can_go_back: crate::Signal<bool>,
}

/// Output of [`NavigatorCallbacks::build_layout`]. The framework
/// already built the layout's subtree into the backend's native
/// node type; the backend just needs to (1) insert `root` into the
/// navigator's container, and (2) hold onto `outlet_ref` so it can
/// look up the outlet's native node and append screens into it.
///
/// Generic over `N` (the backend's `Node` type) so the framework
/// can return the actual node, not a type-erased one.
pub struct LayoutPlan<N: Clone + 'static> {
    pub root: N,
    pub outlet_ref: Ref<crate::ViewHandle>,
}
