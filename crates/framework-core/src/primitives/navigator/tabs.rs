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

use super::shared::{
    LayoutBuilder, NavigatorCallbacks, NavigatorHandle, Route, RouteEntry, RouteParams,
    ScreenBuilder,
};
use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::collections::HashMap;
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
// Author-facing builder
// ---------------------------------------------------------------------------

/// Author-facing tab navigator builder. Tabs get declared via
/// `.tab(...)`; the framework wires the rest. See module-level
/// docs for usage.
pub struct TabNavigator {
    pub initial: &'static str,
    pub initial_path: &'static str,
    /// Ordered list of (route name, spec) — preserves the order tabs
    /// were declared in, which the backend uses to render the bar.
    pub tab_order: Vec<(&'static str, TabSpec)>,
    pub screens: HashMap<&'static str, RouteEntry>,
    pub layout: Option<LayoutBuilder>,
    pub placement: TabPlacement,
    pub mount_policy: MountPolicy,
    pub style: Option<crate::StyleSource>,
    pub ref_fill: Option<RefFill>,
}

impl TabNavigator {
    /// Construct a tab navigator with `initial` as the active tab.
    /// The route must be registered via `.tab(...)` before the
    /// navigator mounts; an unregistered initial tab panics.
    pub fn new(initial: &Route<()>) -> Bound<TabsHandle> {
        Bound::new(Primitive::TabNavigator(Box::new(TabNavigator {
            initial: initial.name(),
            initial_path: initial.path(),
            tab_order: Vec::new(),
            screens: HashMap::new(),
            layout: None,
            placement: TabPlacement::Auto,
            mount_policy: MountPolicy::LazyPersistent,
            style: None,
            ref_fill: None,
        })))
    }
}

impl Bound<TabsHandle> {
    /// Register a tab. Rolls `.screen(...)` and presentation metadata
    /// into one call — the typical case for a tab navigator. For
    /// edge cases where the same route should be reachable from
    /// multiple tabs or also deep-linkable independently, split into
    /// `.tab_spec(...)` + `.screen(...)` (todo: not yet exposed).
    pub fn tab<P: RouteParams>(
        mut self,
        route: Route<P>,
        spec: TabSpec,
        render: impl Fn(P) -> Primitive + 'static,
    ) -> Self {
        if let Primitive::TabNavigator(nav) = &mut self.primitive {
            let render = Rc::new(render);
            let build: ScreenBuilder = Rc::new(move |boxed: Box<dyn Any>| {
                let params: Box<P> = boxed.downcast().unwrap_or_else(|_| {
                    panic!(
                        "TabNavigator: screen param type mismatch for route — \
                         declared params don't match dispatched params"
                    )
                });
                render(*params)
            });
            let from_segments = Rc::new(|segs: &HashMap<String, String>| {
                P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
            });
            nav.tab_order.push((route.name(), spec));
            nav.screens.insert(
                route.name(),
                RouteEntry { path: route.path(), build, from_segments },
            );
        }
        self
    }

    /// Override the tab bar's placement. Default is
    /// `TabPlacement::Auto` — backends pick based on platform
    /// conventions (bottom on phones, top on web).
    pub fn placement(mut self, placement: TabPlacement) -> Self {
        if let Primitive::TabNavigator(nav) = &mut self.primitive {
            nav.placement = placement;
        }
        self
    }

    /// Override when tab screens are mounted and disposed.
    /// Default is `MountPolicy::LazyPersistent`.
    pub fn mount_policy(mut self, policy: MountPolicy) -> Self {
        if let Primitive::TabNavigator(nav) = &mut self.primitive {
            nav.mount_policy = policy;
        }
        self
    }

    /// Install a layout wrapper around the tab navigator. Useful on
    /// web for adding a top app bar that spans tabs. Native backends
    /// ignore this — the tab bar controller draws its own chrome.
    pub fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(super::shared::LayoutProps) -> Primitive + 'static,
    {
        if let Primitive::TabNavigator(nav) = &mut self.primitive {
            nav.layout = Some(Rc::new(f));
        }
        self
    }

    /// Bind a `Ref<TabsHandle>` so the handle is filled at mount
    /// time.
    pub fn bind(mut self, r: Ref<TabsHandle>) -> Self {
        if let Primitive::TabNavigator(nav) = &mut self.primitive {
            nav.ref_fill = Some(RefFill::TabNavigator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

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

