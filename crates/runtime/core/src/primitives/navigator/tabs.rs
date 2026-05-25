//! Tab navigator — a tab bar plus a switched content region.
//!
//! Authors declare a set of tabs up-front (each tab being a route +
//! presentation metadata + a render closure) and an initial tab.
//! At runtime, an imperative `TabsHandle` (obtained via `.bind(ref)`)
//! switches the active tab; users tap the tab bar to do the same.
//!
//! # Per-platform semantics
//!
//! - **iOS**: backed by `UITabBarController`. Each tab is a child
//!   view controller; the bar is iOS-rendered.
//! - **Android**: backed by `BottomNavigationView` (bottom placement)
//!   or `TabLayout` (top placement) hosting child fragments.
//! - **Web**: a `<nav role="tablist">` rendered alongside a content
//!   region. Inactive tabs are kept in the DOM (per `MountPolicy`)
//!   so state is preserved on switch.
//!
//! Phase-3 status: this module defines the public surface (primitive
//! variant, builder, handle, callbacks bundle) but backend
//! implementations have not landed yet. Calling `.create_tab_navigator`
//! on a backend that hasn't implemented it panics (the trait default).
//!
//! # State preservation
//!
//! Each tab's screen runs inside its own reactive `Scope`. The
//! `MountPolicy` controls when that scope is created and destroyed:
//!
//! - `LazyPersistent` (default): mount the screen the first time its
//!   tab is activated, then keep it mounted forever. Matches React
//!   Navigation; preserves stack depth across tab switches when each
//!   tab's screen is itself a nested stack.
//! - `EagerPersistent`: mount every tab's screen at navigator
//!   creation. Higher memory; switch is pure visibility toggle.
//! - `LazyDisposing`: drop the inactive tab's scope on switch.
//!   Cheap memory; state-losing.

use super::shared::{NavigatorCallbacks, NavigatorHandle, Route, RouteParams};
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Presentation metadata — TabSpec
// ---------------------------------------------------------------------------

/// Per-tab presentation metadata. Wraps the label, optional icon
/// name (resolved by the backend's icon set), and an optional
/// reactive badge.
///
/// The screen *itself* (the render closure that produces the tab's
/// content subtree) is supplied alongside via the navigator's
/// `.tab(route, spec, render)` builder; `TabSpec` only carries the
/// chrome metadata, mirroring how `.layout(...)` separates chrome
/// from screen logic for the stack navigator.
pub struct TabSpec {
    pub label: String,
    /// Optional icon name. Resolved against the backend's icon set
    /// (web pulls from idea-ui's icon SVGs; iOS pulls from SF
    /// Symbols by default; Android pulls from Material icons).
    /// `None` ⇒ label-only tab.
    pub icon: Option<String>,
    /// Optional reactive badge content. Returning an empty string
    /// hides the badge. Reading signals inside the closure
    /// subscribes the badge effect — toggle visibility or
    /// increment a count without rebuilding the tab.
    pub badge: Option<Rc<dyn Fn() -> String>>,
}

impl TabSpec {
    /// Construct a tab spec with a label and no icon / badge.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            badge: None,
        }
    }

    /// Attach an icon name. The backend resolves the name against
    /// its icon set.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// Attach a reactive badge. The closure runs inside a backend
    /// effect; signals it reads drive updates.
    pub fn badge<F>(mut self, f: F) -> Self
    where
        F: Fn() -> String + 'static,
    {
        self.badge = Some(Rc::new(f));
        self
    }
}

// ---------------------------------------------------------------------------
// TabPlacement + MountPolicy
// ---------------------------------------------------------------------------

/// Where the tab bar sits relative to the content region.
///
/// Default is `Bottom` on phones, `Top` on web/tablet. The builder's
/// `.placement(...)` overrides; backends pick based on
/// `placement = TabPlacement::Auto` (the default).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TabPlacement {
    Auto,
    Top,
    Bottom,
    /// Vertical sidebar — common on tablets and web. Backends that
    /// don't support a sidebar layout (mobile) fall back to `Top`.
    Sidebar,
}

impl Default for TabPlacement {
    fn default() -> Self {
        TabPlacement::Auto
    }
}

/// When to mount and dispose tab screens.
///
/// See module-level docs. Default is `LazyPersistent` — matches user
/// expectations on mobile (state is preserved across tab switches
/// after first visit).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MountPolicy {
    /// Mount every tab's screen at navigator creation. Switch is
    /// pure visibility toggle. Highest memory.
    EagerPersistent,
    /// Mount on first activation; keep mounted forever after.
    /// Memory grows monotonically with explored tabs.
    LazyPersistent,
    /// Mount on activation; drop the previous tab's scope on
    /// switch. Cheap memory; state-losing.
    LazyDisposing,
}

impl Default for MountPolicy {
    fn default() -> Self {
        MountPolicy::LazyPersistent
    }
}

// ---------------------------------------------------------------------------
// Handle — the imperative API exposed via .bind(...)
// ---------------------------------------------------------------------------

/// Imperative handle for a tab navigator. Backed by the same
/// `NavigatorControl` as every other navigator kind; this wrapper
/// only exposes the methods that make sense for tabs (`select`,
/// `active`) so authors can't accidentally call `.push(...)` against
/// a tab nav.
#[derive(Clone)]
pub struct TabsHandle {
    inner: NavigatorHandle,
}

impl TabsHandle {
    pub fn from_inner(inner: NavigatorHandle) -> Self {
        Self { inner }
    }

    /// Switch the active tab to `route` with `params`. If `route`
    /// is not registered as a tab, panics — same contract as
    /// `NavigatorHandle::push`.
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P) {
        if let Some(c) = self.inner.control() {
            let url = params.to_path(route.path());
            c.dispatch(super::shared::NavCommand::Select {
                name: route.name(),
                url,
                params: Box::new(params),
                state: None,
            });
        }
    }

    /// Access the underlying generic navigator handle. Useful for
    /// passing to APIs that work uniformly across navigator kinds.
    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// Author-facing builder REMOVED — was `pub struct TabNavigator` +
// `impl TabNavigator { fn new }` + `impl Bound<TabsHandle>`. The
// builder now lives in `crates/sdk/tab-navigator/` and produces
// `Primitive::Navigator` instead of the dropped
// `Primitive::TabNavigator`. The substrate types (TabSpec /
// TabPlacement / MountPolicy / TabsHandle / TabRegistration /
// TabNavigatorCallbacks) below remain — backends and SDK adapters
// reference them.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Callbacks bundle — what backends receive
// ---------------------------------------------------------------------------

/// Per-tab registration data handed to the backend so it can build
/// the tab bar. Same shape as `TabSpec` but with the route name
/// baked in.
pub struct TabRegistration {
    pub route: &'static str,
    pub label: String,
    pub icon: Option<String>,
    pub badge: Option<Rc<dyn Fn() -> String>>,
}

/// Bundle the framework hands to `Backend::create_tab_navigator`.
/// Reuses the shared `NavigatorCallbacks<N>` for the screen/scope
/// machinery and adds tab-specific data.
pub struct TabNavigatorCallbacks<N: Clone + 'static> {
    pub navigator: NavigatorCallbacks<N>,
    pub tabs: Vec<TabRegistration>,
    pub placement: TabPlacement,
    pub mount_policy: MountPolicy,
    /// Called by the backend when the user taps a tab. The
    /// framework's dispatcher already mounts the new screen + updates
    /// `nav_state.active_route`; this callback exists for backends
    /// that need a hook after the commit (e.g. analytics).
    pub active_changed: Rc<dyn Fn(&'static str)>,
}

