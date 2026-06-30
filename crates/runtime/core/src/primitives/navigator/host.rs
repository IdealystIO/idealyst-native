//! Navigator handler contract + framework affordances.
//!
//! - **Framework** owns the substrate (`shared.rs`).
//! - **SDK crates** implement `NavigatorHandler` per backend they
//!   support. Each handler owns its kind's chrome (UINavigationController,
//!   DrawerLayout, DOM router, etc.) and its kind-specific dispatcher.
//! - **Each backend** holds a [`super::registry::NavigatorRegistry`]
//!   keyed by the SDK's presentation `TypeId` and implements
//!   `Backend::create_navigator` to consult it.
//!
//! The framework never branches on navigator kind. The presentation
//! TypeId is the only thing routing handlers to instances; everything
//! else is opaque.

use super::shared::{MountResult, NavCommand, NavState, NavigatorControl, NavigatorOps};
use std::any::Any;
use std::rc::Rc;

/// No-op `NavigatorOps` used as the default reference inside
/// [`NavigatorHandler::make_handle`]. Defined at module scope so a
/// `&'static` reference is available.
struct NoopHandlerOps;
impl NavigatorOps for NoopHandlerOps {}
static NOOP_HANDLER_OPS: NoopHandlerOps = NoopHandlerOps;

/// Affordances the framework hands to a registered SDK handler at
/// `init` time. Carries everything the handler needs from the
/// substrate — mount/release callbacks, the control plane, reactive
/// nav state, plus two scope-aware Element→Node builders for SDK
/// chrome.
///
/// The handler stores whatever it needs from this bundle for the
/// navigator's lifetime; the rest can be dropped after `init`
/// returns.
pub struct NavigatorHost<N: Clone + 'static> {
    /// Route name for the initial screen.
    pub initial_route: &'static str,

    /// Concrete URL path for the initial screen.
    pub initial_path: &'static str,

    /// When `true`, the framework does NOT auto-mount the initial
    /// screen — the handler is expected to call `mount_screen` itself
    /// (typically web reading the current URL for deep linking).
    /// When `false`, the framework calls `mount_screen` immediately
    /// after `init` returns and feeds the result via `attach_initial`.
    pub defer_initial_mount: bool,

    /// Realize a screen subtree. Framework allocates a fresh scope,
    /// runs the route's builder closure inside it, returns the
    /// backend node + scope id + opaque options.
    ///
    /// The third argument is the optional opaque `state` payload from
    /// the originating `NavCommand`. The framework pushes it onto the
    /// per-screen state stack for the duration of the screen build,
    /// so the screen's render closure can read it via
    /// [`super::shared::current_screen_state`].
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<N>>,

    /// Release a previously-mounted screen by scope id. Drops the
    /// screen's reactive scope (runs cleanup effects). Idempotent.
    pub release_screen: Rc<dyn Fn(u64)>,

    /// Match a URL path against the navigator's route table. Returns
    /// `(route_name, typed_params_box)` for the first FULL-match (the
    /// path equals a route's pattern after stripping this navigator's
    /// base). Used by web/SSR; native handlers can ignore.
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,

    /// Resolve a URL path against the route table by PREFIX matching
    /// (after stripping this navigator's [`base`](Self::base)): returns
    /// the best-matching `(route_name, typed_params, remainder)`, where
    /// `remainder` is the unconsumed tail after the matched route's
    /// pattern. "Best" = the route whose pattern consumes the most
    /// segments, so a specific route beats an index (`""` pattern).
    ///
    /// This is the hierarchical / deep-link entry point: a navigator
    /// prefix-selects the screen for an incoming URL even when the URL
    /// continues into a NESTED navigator (which resolves the remainder
    /// against its own routes). `match_path` is the full-match special
    /// case. Web/native handlers use this on cold-load / deep-link.
    pub resolve_entry:
        Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>,

    /// This navigator's hierarchy base prefix — the URL it's mounted
    /// under (empty for the root, e.g. `/encounters` for a stack nested
    /// in that drawer screen). Web/native handlers strip it from the
    /// platform URL before resolving. Route patterns are relative to it.
    pub base: String,

    /// Reactive nav-state mirror. Updated automatically by the
    /// substrate's `NavigatorControl::dispatch` for commands that
    /// change the active route. Handlers update it via the
    /// `depth_changed` / `active_changed` callbacks below when state
    /// changes asynchronously (native back gesture, popstate).
    pub nav_state: NavState,

    /// Notify the framework that stack depth changed (push / pop /
    /// reset). Updates the cached depth on `NavigatorControl` and the
    /// `can_go_back` signal. Tabs/drawers typically don't call this.
    pub depth_changed: Rc<dyn Fn(usize)>,

    /// Notify the framework that the active screen changed without a
    /// depth change (tab switch, drawer item select). Updates
    /// `nav_state.active_route` / `nav_state.active_path`. Stack
    /// handlers typically don't call this directly.
    pub active_changed: Rc<dyn Fn(&'static str, String)>,

    /// The shared control plane. Handlers store this so they can
    /// dispatch commands originating from native gestures (back
    /// button, edge swipe, browser back) back through the substrate.
    pub control: Rc<NavigatorControl>,

    /// Materialize a Element into a backend Node. Used for SDK
    /// chrome that lives the navigator's full lifetime (sidebar,
    /// custom bars). The framework wraps the call in a fresh reactive
    /// scope retained on the navigator — effects inside the built
    /// subtree die when the navigator's enclosing scope drops.
    ///
    /// **Must be called outside the outer `backend.borrow_mut()`**
    /// (i.e. not synchronously from `init`). Defer via
    /// `runtime_core::schedule_microtask` or your platform's
    /// equivalent.
    pub build_node: Rc<dyn Fn(crate::Element) -> N>,

    /// Like [`build_node`](Self::build_node) but takes the **builder closure**
    /// instead of an already-constructed `Element`, and runs that closure
    /// INSIDE the retained chrome scope.
    ///
    /// This matters for chrome whose `Element` is produced by a `#[component]`
    /// body containing a standalone `Effect`/`AnimatedValue` (e.g. idea-ui's
    /// animated `Switch`). A component body runs its `Effect::new` the moment
    /// the `Element` is *constructed* — so if a handler builds the `Element`
    /// (`sidebar_builder(props)`) and only then calls `build_node`, those
    /// effects were created with NO active scope: their handle owns them and
    /// frees them when the body returns, so they run once and never re-fire
    /// (the macOS drawer's Switch thumb froze for exactly this reason). Passing
    /// the builder here defers construction into the scope so the effects are
    /// owned by it and stay reactive. Same must-run-outside-the-outer-borrow
    /// rule as `build_node`.
    pub build_node_scoped: Rc<dyn Fn(Box<dyn FnOnce() -> crate::Element>) -> N>,

    /// [`build_node`](Self::build_node) plus insert-into-parent, so a
    /// closure with no backend reference can attach chrome into an
    /// existing slot. Lets a handler defer building an author `Element`
    /// (e.g. a drawer sidebar) to a microtask that runs *after* the
    /// `create_navigator` borrow releases, then splice it in — without
    /// reaching into a backend's node internals. Same
    /// must-run-outside-the-outer-borrow rule as `build_node`.
    pub build_node_into: Rc<dyn Fn(N /* parent */, crate::Element)>,

    /// Materialize a Element into a Node, scoped to a specific
    /// screen's lifetime. Pass the `scope_id` from a `MountResult`.
    /// Used for per-screen SDK chrome (custom title view, custom
    /// button content) — when the SDK calls `release_screen(scope_id)`,
    /// anything built here drops alongside the screen.
    ///
    /// Same defer-via-microtask rule as `build_node` — must be called
    /// outside the outer borrow window.
    pub build_in_screen: Rc<dyn Fn(u64, crate::Element) -> N>,

    /// Insert an already-built `child` node as the last child of
    /// `parent`. The backend `Rc` is captured internally (so the
    /// backend type stays erased from this node-typed host), letting a
    /// **backend-neutral** handler attach a node it already holds — e.g.
    /// a drawer's `Select` dispatcher splicing the freshly
    /// `mount_screen`'d screen into its outlet. Per-backend handlers do
    /// this through their backend's global-self
    /// (`with_global_backend`); this is the portable equivalent, the
    /// enabler for one generic native handler across every backend.
    ///
    /// **Must be called outside the outer `backend.borrow_mut()`**
    /// (i.e. from a dispatcher/microtask, not synchronously from
    /// `init`) — it re-borrows the backend, same rule as `build_node`.
    pub insert_node: Rc<dyn Fn(N /* parent */, N /* child */)>,

    /// Detach every child of `parent` (the backend's `clear_children`).
    /// Companion to [`insert_node`](Self::insert_node) for a generic
    /// handler's outlet swap: clear the outgoing screen before inserting
    /// the incoming one. Same backend-erased, must-run-outside-the-outer-
    /// borrow contract.
    pub clear_children: Rc<dyn Fn(N /* parent */)>,
}

/// Implementation contract for a registered navigator kind. SDK
/// crates implement this once per backend they support.
///
/// # Lifecycle
///
/// 1. Framework calls [`Self::init`] with host + opaque presentation.
///    Handler builds its native root view, installs its dispatcher
///    on `host.control`, returns the root node.
/// 2. Framework calls [`Self::attach_initial`] with the framework-
///    realized initial screen (unless `defer_initial_mount` was set).
/// 3. Framework dispatches `NavCommand`s through the installed
///    dispatcher; [`Self::on_command`] is the catch-all for any
///    command the dispatcher closure didn't handle.
/// 4. Native back gestures trigger [`Self::on_system_back`].
/// 5. When the enclosing scope drops, the framework calls
///    [`Self::release`].
pub trait NavigatorHandler<B: crate::Backend + 'static>: 'static {
    /// Construct the navigator's root native view. `presentation` is
    /// the typed payload the SDK chose for its `Element::Navigator`;
    /// downcast to the expected type (registration uses TypeId so the
    /// type matches by construction).
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node;

    /// Insert the framework-realized initial screen into the native
    /// container. `options` is the screen's opaque SDK options
    /// (downcast to your SDK's options type). Skipped when
    /// `defer_initial_mount` was `true` at init time.
    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    );

    /// Catch-all command dispatch. The framework routes every
    /// `NavCommand` here AFTER the dispatcher closure installed on
    /// `host.control` runs. Most SDKs implement all dispatch in the
    /// closure and leave this as the trait default (no-op).
    #[allow(unused_variables)]
    fn on_command(&mut self, cmd: NavCommand) {}

    /// Native back gesture (Android back press, iOS edge-swipe,
    /// browser back). Return `true` to consume; `false` to let the
    /// platform handle it. Default: `false`.
    #[allow(unused_variables)]
    fn on_system_back(&mut self, backend: &mut B) -> bool {
        false
    }

    /// Called when the navigator's enclosing scope drops. SDK
    /// releases its native resources. Default: no-op.
    #[allow(unused_variables)]
    fn release(&mut self, backend: &mut B) {}

    /// Build the `NavigatorHandle` exposed to author code via
    /// `Ref<H>::bind(...)`. Default returns an inert handle (no
    /// control wired) — SDKs that stored `host.control` at init
    /// should override and call `NavigatorHandle::with_control(...)`.
    fn make_handle(&self) -> super::shared::NavigatorHandle {
        super::shared::NavigatorHandle::new(Rc::new(()), &NOOP_HANDLER_OPS)
    }

    /// Apply a slot style update. `slot` is an SDK-defined identifier
    /// string (e.g. `"header"`, `"tab_bar"`, `"sidebar"`); SDKs no-op
    /// on unknown slots. Default: no-op.
    #[allow(unused_variables)]
    fn apply_slot_style(
        &mut self,
        backend: &mut B,
        slot: &'static str,
        style: &Rc<crate::style::StyleRules>,
    ) {
    }
}
