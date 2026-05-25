//! Navigator extension host — the contract a registered navigator
//! handler implements, and the framework-supplied affordances it
//! consumes.
//!
//! # Layering
//!
//! - **Framework-core** owns the navigation *substrate*: route
//!   registry, `NavigatorControl`, ambient capture, screen scopes,
//!   per-screen reactive lifecycle, hardware-back coordination,
//!   `NavCommand` shape.
//! - **Each backend** holds a [`super::registry::NavigatorRegistry`]
//!   field, exposes `register_navigator` / `has_navigator` methods, and
//!   implements `Backend::create_navigator` to consult the
//!   registry.
//! - **Each navigator-kind SDK crate** (`stack-navigator`,
//!   `tab-navigator`, `drawer-navigator`, or any third-party kind)
//!   implements [`NavigatorHandler`] per backend it supports and
//!   registers via the backend's `register_navigator` method.
//! - **The author-facing builders** (`Navigator::new(...)`,
//!   `TabNavigator::new(...)`, etc.) live in the SDK crates and produce
//!   `Primitive::Navigator` instances carrying the SDK's typed
//!   presentation payload.
//!
//! Compare with [`crate::external::ExternalRegistry`]: that pattern is
//! for *opaque* third-party primitives (a video, a map, a webview).
//! Navigators aren't opaque — they participate in routing, lifecycle,
//! and back-handling. The host below exposes those framework-owned
//! concerns as typed methods on a single object, instead of routing
//! everything through an opaque payload.

use super::shared::{LayoutPlan, MountResult, NavCommand, NavState, NavigatorControl};
use std::any::Any;
use std::rc::Rc;

/// Helper discriminant SDK handlers can use to tell their backend
/// "this navigator I just created is of kind X". Backends with per-kind
/// storage (iOS, Android) use this at dispatch time to route
/// `navigator_attach_initial` / `release` / etc. to the right
/// legacy method.
///
/// Built-in kinds enumerated here so backends can match on them
/// without depending on the SDK crates. Third-party kinds use
/// [`NavigatorKind::Custom`] with their own marker discriminant — the SDK
/// handles its own dispatch in that case.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavigatorKind {
    Stack,
    Tab,
    Drawer,
    /// Third-party navigator kind. The SDK is responsible for routing
    /// post-init operations (slot styles, release, attach_initial)
    /// through its own per-handler bookkeeping.
    Custom,
}

/// Affordances the framework provides to a registered navigator
/// handler. Constructed by the walker when a `Primitive::Navigator`
/// is realized; consumed by the handler's `init` plus subsequent
/// `on_command` calls (the handler stores whatever it needs from here
/// for the lifetime of the navigator).
pub struct NavigatorHost<N: Clone + 'static> {
    /// Route name for the initial screen. Always non-empty.
    pub initial_route: &'static str,

    /// Concrete URL path for the initial screen (`""` if the route's
    /// pattern has no placeholders). Used by web/SSR backends.
    pub initial_path: &'static str,

    /// When `true`, the framework will *not* explicitly mount the
    /// initial screen — the handler is expected to call `mount_screen`
    /// itself, typically after reading the current URL (web does this
    /// for deep linking). When `false`, the framework calls
    /// `mount_screen(initial_route, ())` immediately after `init`
    /// returns and feeds the result into the handler via a separate
    /// `attach_initial` hook (see [`NavigatorHandler::attach_initial`]).
    pub defer_initial_mount: bool,

    /// Realize a screen subtree. Framework allocates a fresh reactive
    /// scope, runs the route's builder closure inside it, returns the
    /// backend node + scope id + per-screen options. The handler holds
    /// onto the `scope_id` and passes it to `release_screen` when the
    /// screen leaves the navigator (popped, replaced, reset, tab
    /// changed away in `LazyDisposing` mode, etc.).
    ///
    /// The third argument is the optional opaque `state` from the
    /// originating `NavCommand`. The framework pushes it onto the
    /// per-screen state stack for the duration of the screen build, so
    /// the screen's render closure can read it via
    /// [`super::shared::current_screen_state`]. Pass `None` when the
    /// handler doesn't have state to forward (initial mount, deep-link
    /// route resolution, etc.).
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<N>>,

    /// Drop a previously-mounted screen by scope id. Runs the screen's
    /// cleanup effects. Idempotent — releasing an unknown scope id is
    /// a no-op.
    pub release_screen: Rc<dyn Fn(u64)>,

    /// Match a URL path against the navigator's route table. Returns
    /// `(route_name, typed_params_box)` for the first matching pattern.
    /// Used by web/SSR for URL-driven mounting; native handlers can
    /// ignore.
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,

    /// Build the user-supplied layout subtree (`.layout(...)` closure),
    /// if any. The result carries the layout root node + a `Ref` to the
    /// outlet view, which the framework resolves to a concrete backend
    /// node after `init`. Web backends use this to render chrome; most
    /// native backends ignore it (their chrome is supplied by the
    /// native widget — `UINavigationBar`, `UITabBar`, etc.).
    pub build_layout: Option<Rc<dyn Fn() -> LayoutPlan<N>>>,

    /// Reactive nav-state mirror. The framework updates these signals
    /// automatically when commands dispatch through
    /// [`NavigatorControl::dispatch`]. Handlers normally only *read*
    /// them (to drive their own internal state); writes belong to the
    /// `notify_*` methods below, which keep the cached
    /// [`NavigatorControl::depth`] / [`NavigatorControl::default_link_kind`]
    /// in sync alongside the signals.
    pub nav_state: NavState,

    /// Notify the framework that stack depth changed (after a push /
    /// pop / replace / reset). Updates the cached depth on
    /// [`NavigatorControl`] and the `can_go_back` signal. Tab and
    /// drawer handlers typically never call this — their depth is
    /// fixed at 1.
    pub depth_changed: Rc<dyn Fn(usize)>,

    /// Notify the framework that the active screen changed without a
    /// depth change (tab switch, drawer item select). Updates
    /// `nav_state.active_route` / `nav_state.active_path`. Stack
    /// handlers typically don't call this directly — pushing /
    /// replacing through the dispatcher already updates these signals
    /// before the command reaches the handler.
    pub active_changed: Rc<dyn Fn(&'static str, String)>,

    /// The shared control plane. Handlers store this so they can
    /// dispatch commands originating from *native* gestures (back
    /// button on Android, edge swipe on iOS, browser back button)
    /// back into the framework's dispatch path. The framework will
    /// route those commands to [`NavigatorHandler::on_command`] for
    /// consistency.
    pub control: Rc<NavigatorControl>,
}

/// Implementation contract for a registered navigator kind. Each SDK
/// crate implements this trait once per backend it supports.
///
/// The handler owns the **presentation** (native chrome, transitions,
/// gestures). The framework owns the **substrate** (routing, screen
/// scopes, ambient capture, hardware-back coordination, the
/// `NavigatorControl` handle exposed to user code).
///
/// # Lifecycle
///
/// 1. Framework calls [`Self::init`] with the host + the SDK's
///    presentation payload. Handler builds its native root view and
///    returns it. The framework inserts that view into the parent.
/// 2. Framework calls [`Self::attach_initial`] with the realized
///    initial screen (unless `defer_initial_mount` was set). Handler
///    inserts the screen into its native container.
/// 3. Framework calls [`Self::on_command`] for every `NavCommand`
///    dispatched through the navigator's control plane. Handler
///    interprets per its kind.
/// 4. On system back gestures the framework calls
///    [`Self::on_system_back`]; the handler returns whether it
///    consumed the gesture.
/// 5. When the navigator's reactive scope drops, the framework calls
///    [`Self::release`] for handler-owned native resource cleanup.
pub trait NavigatorHandler<B: crate::Backend + 'static>: 'static {
    /// Construct the native root view. `presentation` is the typed
    /// payload the SDK chose for its `Primitive::Navigator`
    /// (e.g. `stack_navigator::StackPresentation`). The handler
    /// downcasts to its expected type — payload-to-handler type
    /// matching is enforced by `TypeId` at registration time.
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node;

    /// Insert the framework-realized initial screen into the native
    /// container. Skipped when `host.defer_initial_mount` was `true` at
    /// init time. Default impl panics — handlers that defer must
    /// override and either implement this as a no-op or self-mount in
    /// `init`.
    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        scope_id: u64,
        options: super::shared::ScreenOptions,
    );

    /// Dispatch a `NavCommand` against the handler. The framework
    /// forwards every command from [`NavigatorControl::dispatch`] here;
    /// the handler interprets commands it understands and panics (or
    /// no-ops, by kind contract) on commands it doesn't.
    ///
    /// **No `&mut B`** — this method is invoked through the dispatch
    /// closure installed on `NavigatorControl::install`, whose
    /// signature is `Box<dyn Fn(NavCommand)>`. The handler must
    /// internalize any backend state it needs during `init` (typically
    /// as `Rc<RefCell<…>>` clones of the backend's internal handles).
    /// This mirrors how the per-kind backend impls already work today
    /// — their dispatch closures capture state, not the backend
    /// itself.
    fn on_command(&mut self, cmd: NavCommand);

    /// System back (Android back-button press, iOS edge-swipe-completed,
    /// browser-back). Return `true` to consume; `false` to let the
    /// platform handle it (which usually means closing the app on
    /// Android root, replaying history on web, no-op on iOS).
    ///
    /// Default returns `false`.
    #[allow(unused_variables)]
    fn on_system_back(&mut self, backend: &mut B) -> bool {
        false
    }

    /// Called when the navigator's enclosing scope drops. Handler
    /// releases its native resources. Default is a no-op so handlers
    /// that hold only `B::Node` (the framework will release the node
    /// itself) don't need to override.
    #[allow(unused_variables)]
    fn release(&mut self, backend: &mut B) {}

    /// Apply a slot style update (e.g. header bar background,
    /// tab bar tint, drawer scrim color). `slot` is an SDK-defined
    /// identifier string — the framework hands through opaque strings
    /// the SDK's builder emits via the navigator's `.with_style(...)`
    /// chain. Handlers no-op on unknown slots.
    ///
    /// Default is a no-op so handlers that don't support per-slot
    /// styling don't need to override.
    #[allow(unused_variables)]
    fn apply_slot_style(
        &mut self,
        backend: &mut B,
        slot: &'static str,
        style: &Rc<crate::style::StyleRules>,
    ) {
    }
}
