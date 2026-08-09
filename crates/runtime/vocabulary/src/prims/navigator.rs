//! Navigator payloads: `swap_navigator`, `stack_navigator`, and the
//! `navigator_outlet` — the outlet-model navigation primitives, ported
//! from the old `Element::Navigator` substrate + the backend-neutral
//! `swap-navigator` / `stack-navigator` SDK handlers.
//!
//! # What moved where (vs the old core)
//!
//! - The old split — framework substrate (`walker/navigator.rs` +
//!   `primitives::navigator::shared`) vs SDK handler crates — collapses:
//!   swap and stack are ordinary vocabulary primitives mounted through
//!   the registry, exactly like `view`. The routing *data* types that are
//!   Element-free and pure (`Route`, `RouteParams`, `NavCommand`,
//!   `join_path`/`match_prefix`/`match_pattern`, the nav-base and
//!   screen-state/route thread-local guards, the fill-rule constructors)
//!   are REUSED from `runtime_shared::primitives::navigator` — the
//!   sanctioned transitional dependency; they migrate here at P7.
//! - `SwapContext`/`StackContext` become world-context values
//!   ([`SwapNav`] / [`StackNav`], `provide`d by the handler around the
//!   author layout build and read back with `inject`), and the one-shot
//!   `ctx.outlet` element is replaced by the plain
//!   [`navigator_outlet`](crate::builders::navigator_outlet) builder.
//! - `NavigatorControl` / `NavigatorHost` / `retain_scope` / the
//!   keepalive+release_navigator cycle-break machinery all disappear:
//!   nav-state signals and the dispatch driver effect are created inside
//!   the mount handler, so the enclosing `Realized` owns them — one drop
//!   tears the whole navigator down (design §4: "the retain_scope
//!   machinery simplifies").
//!
//! Screens are held as retained [`runtime_scene::Realized`] subtrees:
//! cached (persistent-policy) screens keep their nodes + reactive scope
//! while DETACHED from the tree, and are re-inserted on return without
//! re-running the route builder; dropping the `Realized` is the entire
//! screen teardown.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::navigator::{
    split_query, NavCommand, QueryParams, Route, RouteParams, ScreenState,
};
use runtime_scene::Element;
use runtime_world::Signal;

use crate::style_attach::StyleProp;

// ===========================================================================
// Routing config (the new-core `NavigatorConfig` / `RouteEntry`)
// ===========================================================================

/// Build typed params back from `:segment` captures — same alias as the
/// old `ParamsFromSegments`, re-declared here because it pairs with the
/// new-core screen builder.
pub type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

/// One registered route: its (navigator-relative) URL pattern, the
/// type-erased screen builder, and the segment→params reconstructor.
/// The old `RouteEntry`, with the builder returning a [`Screen`] (a
/// scene `Element` plus opaque per-screen options — `Into<Screen>`
/// keeps plain-`Element` builders source-compatible).
pub struct NavScreenEntry {
    pub path: &'static str,
    pub build: Rc<dyn Fn(Box<dyn Any>) -> Screen>,
    pub from_segments: ParamsFromSegments,
}

// ===========================================================================
// Screen — what a route's render closure returns (P6 header-options
// carrier: the new-core port of `runtime_shared::primitives::navigator::
// Screen`, with the body as a scene `Element`)
// ===========================================================================

/// A renderable screen: the body `Element` plus SDK-defined options.
///
/// Options are opaque to the vocabulary (`Rc<dyn Any>`): each SDK
/// defines its own typed options struct (the stack SDK's
/// `StackScreenOptions` with title + bar-button slots) and exposes
/// builder methods (via an extension trait on `Screen`) that wrap
/// [`with`](Self::with). The stack handler carries the ACTIVE screen's
/// options through [`ScreenChrome`] / [`StackNav::screen_chrome`]; the
/// SDK downcasts at read time. `impl From<Element> for Screen` keeps
/// the no-options form ergonomic — every pre-existing
/// `.screen(R, |_| element)` call site compiles unchanged.
///
/// `Rc` (not the old `Box`): the stack keeps the options on the
/// back-stack entry AND republishes them into the chrome signal on
/// every reveal — two owners.
pub struct Screen {
    /// The screen body.
    pub element: Element,
    /// Opaque, SDK-defined per-screen options (`None` = no options).
    pub options: Option<Rc<dyn Any>>,
}

impl Screen {
    /// A screen with no options.
    pub fn new(element: impl Into<Element>) -> Self {
        Self { element: element.into(), options: None }
    }

    /// Set this screen's SDK-defined options (replaces any existing).
    pub fn with<T: Any + 'static>(mut self, options: T) -> Self {
        self.options = Some(Rc::new(options));
        self
    }

    /// Downcast the options to a borrow of `T`. `None` when the screen
    /// has no options or the stored type doesn't match.
    pub fn options_as<T: Any + 'static>(&self) -> Option<&T> {
        self.options.as_ref().and_then(|o| o.downcast_ref::<T>())
    }
}

impl From<Element> for Screen {
    fn from(element: Element) -> Self {
        Screen::new(element)
    }
}

/// The ACTIVE stack screen's chrome payload, published into
/// [`StackNav::screen_chrome`] on every navigation. `rev`-stamped so a
/// push/pop between screens with IDENTICAL options still notifies (the
/// screen underneath swapped — the old handler used `set_always` for
/// exactly this; the new kernel's guarded `set` needs an inequality,
/// which the stamp provides).
#[derive(Clone)]
pub struct ScreenChrome {
    /// Publish counter (monotonic per navigator; equality key).
    pub rev: u64,
    /// The active screen's opaque options (`None` = screen set none).
    pub options: Option<Rc<dyn Any>>,
}

impl PartialEq for ScreenChrome {
    fn eq(&self, other: &Self) -> bool {
        self.rev == other.rev
    }
}

// ===========================================================================
// LinkActivator — the ambient route-link seam (P6 link(route=…) port)
// ===========================================================================

/// The ambient link activator: how a declarative `link(route = …)`
/// resolves to the ENCLOSING navigator's dispatch. Each navigator
/// `provide`s one while building its screens and its author layout
/// (save/restore around the build window, like [`SwapNav`]): the swap
/// stages `Select`, the stack stages `Push` — the same
/// push-vs-select-by-navigator-kind contract the old
/// `NavigatorControl::install_link_activator` carried.
///
/// The vocabulary link handler captures it AT MOUNT (the new-core
/// counterpart of the old construction-time ambient capture — builders
/// are pure data here, and construction + mount are the same
/// synchronous pass). A link mounted outside any navigator captures
/// `None` and silently no-ops on activation (old contract). Same
/// documented limitation as [`ScreenNav`](crate::prims::ScreenNav):
/// a `Dyn` region rebuilt later re-captures whatever context was
/// provided last (world context has no scoping); the recapture rides
/// the P3 driver work.
#[derive(Clone)]
pub struct LinkActivator {
    activate: Rc<dyn Fn(&'static str, String, Box<dyn Any>)>,
}

impl LinkActivator {
    /// Wrap the navigator's staged dispatch. `f` receives
    /// `(route name, navigator-relative url, boxed params)` and must be
    /// handler-safe (stage, never mount).
    pub fn new(f: impl Fn(&'static str, String, Box<dyn Any>) + 'static) -> Self {
        Self { activate: Rc::new(f) }
    }

    /// Fire an activation for `(name, url, params)`.
    pub fn activate(&self, name: &'static str, url: String, params: Box<dyn Any>) {
        (self.activate)(name, url, params)
    }
}

/// The kind-agnostic routing config (old `NavigatorConfig`, minus
/// `defer_initial_mount` — web's deferred URL-driven initial mount is
/// P3-web work and rides the url_sync port, not this handler).
pub struct NavConfig {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, NavScreenEntry>,
}

// ===========================================================================
// Value types (ported from the SDK crates — the vocabulary cannot depend
// on swap-navigator/stack-navigator, so the enums are re-declared)
// ===========================================================================

/// When a swap screen's subtree is materialized and whether it survives
/// a switch away (old `swap_navigator::MountPolicy`). NOTE the old
/// handler never implemented eager mounting — `EagerPersistent` behaves
/// exactly like `LazyPersistent` there, and this port preserves that
/// behavior rather than inventing it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum MountPolicy {
    /// Reserved: behaves like `LazyPersistent` (old-handler parity).
    EagerPersistent,
    /// Mount on first activation; keep mounted (detached `Realized`)
    /// across switches — the default.
    #[default]
    LazyPersistent,
    /// Mount on first activation; DROP the screen's `Realized` when
    /// switched away — re-mounts fresh on return.
    LazyDisposing,
}

/// What happens to a stack screen when a `push` covers it (old
/// `stack_navigator::StackRetention`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StackRetention {
    /// Resolve at mount: `Rebuild` on `Platform::Web`, `Retain`
    /// everywhere else.
    #[default]
    PlatformDefault,
    /// Covered screens stay alive (retained `Realized`); pop reveals
    /// them with state intact. Native-stack semantics.
    Retain,
    /// Covered screens are disposed on push; pop re-mounts from the
    /// URL. Browser semantics.
    Rebuild,
}

/// Builds a route's `(url, typed_params)` for a bare-name selection
/// (`SwapNav::on_select`). `None` when the route needs path params a
/// bare name can't supply — the old select-args fix: dispatching
/// placeholder `""`/`()` values corrupted the path mirror and panicked
/// on non-unit-params routes.
pub type SelectArgs = Rc<dyn Fn() -> Option<(String, Box<dyn Any>)>>;

// ===========================================================================
// World-context values (the old SwapContext/StackContext, minus the
// one-shot outlet element — authors splat `navigator_outlet()` instead)
// ===========================================================================

/// Swap-navigator context, `provide`d into the world context for the
/// duration of the author layout build (and left available to the
/// layout's own reactive regions). Read it with
/// `runtime_world::inject::<SwapNav>()`.
#[derive(Clone)]
pub struct SwapNav {
    /// Currently active route key — highlight the live tab/nav item.
    pub active_route: Signal<&'static str>,
    /// Full resolved path of the active screen. Path only — the query
    /// lives in [`query`](Self::query).
    pub active_path: Signal<String>,
    /// The active screen's query params, republished on every navigation.
    ///
    /// Read this (rather than the build-time `screen_state()` snapshot)
    /// when a screen must react to its state changing WITHOUT remounting —
    /// browser Back onto an entry that differs only in its query, or an
    /// in-place `replace_with_state`.
    pub query: Signal<QueryParams>,
    /// Switch to a sibling screen by route name (`Select`). Handler-safe:
    /// stages a command + tick, the driver effect commits on flush.
    pub on_select: Rc<dyn Fn(&'static str)>,
}

/// Stack-navigator context (`inject::<StackNav>()`).
#[derive(Clone)]
pub struct StackNav {
    /// The active (top) screen's route name.
    pub active_route: Signal<&'static str>,
    /// The active screen's full path. Path only — the query lives in
    /// [`query`](Self::query).
    pub active_path: Signal<String>,
    /// The active screen's query params — see [`SwapNav::query`].
    pub query: Signal<QueryParams>,
    /// Stack depth (1 at the root).
    pub depth: Signal<usize>,
    /// Whether a pop is possible (depth > 1).
    pub can_go_back: Signal<bool>,
    /// Pop the top screen (no-op at the root). Handler-safe.
    pub pop: Rc<dyn Fn()>,
    /// The active screen's chrome payload (its [`Screen`] options),
    /// republished on EVERY navigation — the old
    /// `NavigatorHost::screen_chrome` slot. The stack SDK's
    /// `header_state` downcasts it to `StackScreenOptions` and derives
    /// the `StackHeaderState` an author `StackHeader` renders.
    pub screen_chrome: Signal<ScreenChrome>,
}

// ===========================================================================
// NavHandle — the imperative handle (`.on_handle(...)` on the builders)
// ===========================================================================

/// Typed imperative handle to a live navigator — the new-core
/// counterpart of the SDK `SwapHandle`/`StackHandle` pair, unified
/// (the command vocabulary distinguishes the kinds; a swap ignores
/// stack verbs and vice versa, same as the old dispatchers). Cheap to
/// clone; safe to call from event handlers (dispatch only stages).
#[derive(Clone)]
pub struct NavHandle {
    dispatch: Rc<dyn Fn(NavCommand)>,
}

/// Pointer identity on the dispatcher — a `NavHandle` names ONE live
/// navigator, so clones of it are equal and handles onto two different
/// navigators never are.
///
/// The sole field is `Rc<dyn Fn(NavCommand)>`, a closure over the
/// navigator's driver. Closures have no equality, so the address is not
/// merely the best answer available — it is the only one, and it happens
/// to be the right question: "does this handle drive the same navigator?"
/// is exactly what a guarded `set` wants to know when an author swaps
/// which navigator a control targets.
///
/// Needed because `Signal<T>` is bounded on `T: PartialEq` at creation and
/// `get`, not just on the guarded `set`, and stashing the bound handle in
/// state or context is the ordinary way to drive navigation from outside
/// the navigator's subtree. Supplying it HERE (rather than on each SDK
/// wrapper) means `SwapHandle`/`StackHandle` can simply derive.
impl PartialEq for NavHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.dispatch, &other.dispatch)
    }
}

impl Eq for NavHandle {}

impl NavHandle {
    pub(crate) fn new(dispatch: Rc<dyn Fn(NavCommand)>) -> Self {
        NavHandle { dispatch }
    }

    /// Dispatch a raw command.
    pub fn dispatch(&self, cmd: NavCommand) {
        (self.dispatch)(cmd)
    }

    /// Build the `(path, query)` pair for a navigation. The route pattern
    /// may itself carry a query (`Route::new("search", "/search?sort=new")`
    /// — defaults expressed in the route); the author's state is layered on
    /// top of it, so explicit state wins over a pattern default key-by-key.
    fn route_url<P: RouteParams, S: ScreenState>(
        route: &Route<P>,
        params: &P,
        state: &S,
    ) -> (String, QueryParams) {
        let full = params.to_path(route.path());
        let (path, mut query) = split_query(&full);
        let path = path.to_string();
        for (k, v) in state.to_query().iter() {
            query.set(k, v);
        }
        (path, query)
    }

    /// Switch to `route` (swap navigators). Selecting the already-active
    /// URL is a no-op at the driver.
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.select_with_state(route, params, ())
    }

    /// [`select`](Self::select), carrying `state` as the screen's query
    /// params. See [`ScreenState`].
    pub fn select_with_state<P: RouteParams, S: ScreenState>(
        &self,
        route: &Route<P>,
        params: P,
        state: S,
    ) {
        let (url, query) = Self::route_url(route, &params, &state);
        self.dispatch(NavCommand::Select {
            name: route.name(),
            url,
            params: Box::new(params),
            query,
        });
    }

    /// Push `route` onto the stack.
    pub fn push<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.push_with_state(route, params, ())
    }

    /// [`push`](Self::push), carrying `state` as the screen's query params.
    ///
    /// The screen reads it back with `screen_state::<S>()`, and on a
    /// URL-bearing backend the state lands in the address bar — so a reload
    /// or a shared link reconstructs the same screen. See [`ScreenState`].
    pub fn push_with_state<P: RouteParams, S: ScreenState>(
        &self,
        route: &Route<P>,
        params: P,
        state: S,
    ) {
        let (url, query) = Self::route_url(route, &params, &state);
        self.dispatch(NavCommand::Push {
            name: route.name(),
            url,
            params: Box::new(params),
            query,
        });
    }

    /// Pop the top screen (no-op at the root).
    pub fn pop(&self) {
        self.dispatch(NavCommand::Pop);
    }

    /// Replace the top screen with `route`.
    pub fn replace<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.replace_with_state(route, params, ())
    }

    /// [`replace`](Self::replace), carrying `state` as the screen's query
    /// params.
    ///
    /// This is the verb for "the user changed a filter": it rewrites the
    /// current entry's query in place rather than growing the back stack,
    /// so Back still leaves the screen instead of stepping through every
    /// intermediate filter value.
    pub fn replace_with_state<P: RouteParams, S: ScreenState>(
        &self,
        route: &Route<P>,
        params: P,
        state: S,
    ) {
        let (url, query) = Self::route_url(route, &params, &state);
        self.dispatch(NavCommand::Replace {
            name: route.name(),
            url,
            params: Box::new(params),
            query,
        });
    }

    /// Reset the whole stack to a single `route`.
    pub fn reset<P: RouteParams>(&self, route: &Route<P>, params: P) {
        self.reset_with_state(route, params, ())
    }

    /// [`reset`](Self::reset), carrying `state` as the screen's query
    /// params.
    pub fn reset_with_state<P: RouteParams, S: ScreenState>(
        &self,
        route: &Route<P>,
        params: P,
        state: S,
    ) {
        let (url, query) = Self::route_url(route, &params, &state);
        self.dispatch(NavCommand::Reset {
            name: route.name(),
            url,
            params: Box::new(params),
            query,
        });
    }
}

// ===========================================================================
// Prim payloads
// ===========================================================================

/// The `swap_navigator` primitive — a flat set of co-equal screens
/// switched via `Select`, chrome as author layout around the outlet.
pub struct SwapNavigatorPrim {
    pub config: NavConfig,
    /// The author layout closure (`None` ⇒ a bare outlet fills the
    /// navigator). Reads [`SwapNav`] via `inject`; splats
    /// `navigator_outlet()` exactly once.
    pub layout: Option<Rc<dyn Fn() -> Element>>,
    pub mount_policy: MountPolicy,
    /// Per-route bare-name select recipes (see [`SelectArgs`]).
    pub select_args: HashMap<&'static str, SelectArgs>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub on_handle: Option<Box<dyn FnOnce(NavHandle)>>,
    /// Presentation label for introspection (`NavSnapshot::type_name`).
    /// `None` ⇒ the builder name (`"swap_navigator"`). The SDK sets its
    /// old presentation type name for wire parity with the old bridge.
    pub nav_label: Option<&'static str>,
}

/// The `stack_navigator` primitive — push/pop over a back-stack, chrome
/// as author layout around the outlet.
pub struct StackNavigatorPrim {
    pub config: NavConfig,
    /// The author layout closure (reads [`StackNav`] via `inject`).
    pub layout: Option<Rc<dyn Fn() -> Element>>,
    pub retention: StackRetention,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub on_handle: Option<Box<dyn FnOnce(NavHandle)>>,
    /// Presentation label for introspection — see
    /// [`SwapNavigatorPrim::nav_label`].
    pub nav_label: Option<&'static str>,
}

/// The `navigator_outlet` primitive — the single hole an author layout
/// splats to mark where the active screen renders. Style-less outlets
/// get the fill default (`outlet_fill_rules`); an author style replaces
/// it entirely (old `dispatch_navigator_outlet` contract).
pub struct NavigatorOutletPrim {
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
}
