//! Drawer navigator — a slide-in side panel plus a switched body
//! region.
//!
//! Authors declare drawer entries (each a route + presentation
//! metadata) and an initial selection. The user opens the drawer
//! (hamburger button or platform gesture), taps an entry, and the
//! body region swaps to that entry's screen. An imperative
//! `DrawerHandle` exposes `.select(...)`, `.open()`, `.close()`,
//! `.toggle()`, `.is_open()`.
//!
//! # Per-platform semantics
//!
//! - **iOS**: hand-rolled — UIKit has no native drawer. The backend
//!   slides a `UIView` overlay in from the requested side, with a
//!   tap-outside recognizer for dismissal. Above the `pinned_above`
//!   width breakpoint the drawer is pinned beside the body (a
//!   sidebar) — matches `UISplitViewController`'s posture without
//!   adopting that API's opinions.
//! - **Android**: `DrawerLayout` + `NavigationView`. Standard.
//! - **Web**: an `<aside>` plus a body region. Above
//!   `pinned_above`, the aside is always visible; below, it slides
//!   on/off via CSS transform with a focus trap while open.
//!
//! Phase-4 status: this module defines the public surface (primitive
//! variant, builder, handle, callbacks bundle) but backend
//! implementations have not landed yet. Calling `create_drawer_navigator`
//! on a backend that hasn't implemented it panics (trait default).
//!
//! # Item vs screen registration
//!
//! Unlike tabs (where each tab *always* has a screen), drawers
//! support an asymmetry: routes can be deep-linkable without being
//! drawer entries, and a drawer entry can dispatch to a route that's
//! also reachable elsewhere. The builder splits `.item(...)` from
//! `.screen(...)`:
//!
//! ```ignore
//! DrawerNavigator::new(&home)
//!     .item(home,     DrawerItem::new("Home", "home"))
//!     .item(settings, DrawerItem::new("Settings", "settings"))
//!     .screen(home,     |_| ui! { HomeBody() })
//!     .screen(library,  |_| ui! { LibraryBody() })  // reachable via Link, not drawer
//!     .screen(settings, |_| ui! { SettingsBody() })
//! ```

use super::shared::{
    LayoutBuilder, NavigatorCallbacks, NavigatorHandle, Route, RouteEntry, RouteParams,
    ScreenBuilder,
};
use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Presentation metadata — DrawerItem
// ---------------------------------------------------------------------------

/// Per-drawer-entry presentation metadata. Label + optional icon.
/// Screens are registered separately via `.screen(...)` — see the
/// module docs for the rationale.
pub struct DrawerItem {
    pub label: String,
    pub icon: Option<String>,
}

impl DrawerItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
        }
    }

    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }
}

// ---------------------------------------------------------------------------
// DrawerSide + MountPolicy
// ---------------------------------------------------------------------------

/// Which edge the drawer slides in from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DrawerSide {
    /// Locale-aware leading edge (left in LTR, right in RTL). The
    /// default — matches Material's standard navigation drawer.
    Start,
    /// Locale-aware trailing edge.
    End,
}

impl Default for DrawerSide {
    fn default() -> Self {
        DrawerSide::Start
    }
}

// Re-export `MountPolicy` from `tabs` so drawer authors don't need to
// know there's a `tabs` module. Same semantics either way.
pub use super::tabs::MountPolicy;

// ---------------------------------------------------------------------------
// Handle — the imperative API exposed via .bind(...)
// ---------------------------------------------------------------------------

/// Imperative handle for a drawer navigator. Backed by the same
/// `NavigatorControl` as every other navigator kind. Exposes only the
/// methods that make sense for a drawer.
#[derive(Clone)]
pub struct DrawerHandle {
    inner: NavigatorHandle,
    /// Mirror of the drawer's open-state signal. Updated by the
    /// drawer's dispatcher; `is_open()` reads through it without a
    /// signal subscription (callers who want reactivity should
    /// subscribe to the signal directly via `LayoutProps` or a
    /// reactive helper TBD).
    is_open: Rc<std::cell::Cell<bool>>,
}

impl DrawerHandle {
    pub fn from_inner(inner: NavigatorHandle, is_open: Rc<std::cell::Cell<bool>>) -> Self {
        Self { inner, is_open }
    }

    /// Switch the active drawer screen to `route` with `params`.
    /// If `route` is not registered, panics.
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

    /// Open the drawer.
    pub fn open(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(super::shared::NavCommand::OpenDrawer);
        }
    }

    /// Close the drawer.
    pub fn close(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(super::shared::NavCommand::CloseDrawer);
        }
    }

    /// Toggle the drawer's open state.
    pub fn toggle(&self) {
        if let Some(c) = self.inner.control() {
            c.dispatch(super::shared::NavCommand::ToggleDrawer);
        }
    }

    /// Read the drawer's open state. Non-reactive; subscribe to
    /// the drawer's open-state signal via `DrawerLayoutProps` for
    /// reactive reads inside effects.
    pub fn is_open(&self) -> bool {
        self.is_open.get()
    }

    pub fn inner(&self) -> &NavigatorHandle {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// Sidebar slot — author renders the drawer's side panel
// ---------------------------------------------------------------------------

/// Props handed to the user's `.sidebar(...)` closure. The closure
/// renders the side panel content (entry buttons, brand, footer)
/// using these reactive signals + dispatch callbacks.
///
/// # Per-target rendering
///
/// - **Android**: the framework renders the sidebar as a native
///   side panel inside its drawer-shell (open/close animations,
///   scrim, swipe-to-dismiss).
/// - **Web**: the sidebar is built into a `Primitive` and surfaced
///   on `LayoutProps::sidebar` for the author's `.layout(...)` to
///   place wherever it wants (typically in a flex row beside the
///   outlet). Without a `.layout(...)`, the sidebar is silently
///   dropped on web — see `DrawerNavigator::sidebar` docs.
///
/// # State + dispatch
///
/// Sidebars are reactive: read `active_route` inside the closure
/// to flip the highlighted item without rebuilding the panel.
/// Call `on_select(name)` from a button click to swap the body —
/// or use `Link(route, params)` which auto-dispatches `Select`
/// against the ambient drawer navigator. Both work; `Link` is
/// preferred because it inherits web's hyperlink semantics
/// (middle-click new tab, right-click menu, etc.).
pub struct DrawerSidebarProps {
    /// The drawer's registered items, in declaration order. The
    /// sidebar typically iterates this to render one entry button
    /// per item, but is free to ignore it (e.g. for a sidebar
    /// whose contents don't track the drawer's items 1:1).
    pub items: Vec<DrawerItemRegistration>,
    /// Name of the currently-active route. Read this inside a
    /// reactive closure (`Text { ... active_route.get() ... }`,
    /// or a `style` closure) to drive the active highlight.
    pub active_route: crate::Signal<&'static str>,
    /// Whether the drawer is currently open. On Android this is
    /// kept in sync with the native drawer's visibility; on web
    /// the author's `.layout(...)` is responsible for reading it
    /// and showing/hiding the sidebar (or using it for animations).
    pub is_open: crate::Signal<bool>,
    /// Programmatic body swap. Dispatches a `Select` command —
    /// equivalent to `Link(route, ())` but for cases where the
    /// author wants imperative control (e.g. firing from a custom
    /// gesture).
    pub on_select: Rc<dyn Fn(&'static str)>,
    /// Close the drawer. Useful for a "Done" footer button on
    /// mobile; on desktop where the sidebar is pinned, this still
    /// flips the signal but the layout may ignore it.
    pub on_close: Rc<dyn Fn()>,
}

pub type SidebarBuilder = Rc<dyn Fn(DrawerSidebarProps) -> Primitive>;

// ---------------------------------------------------------------------------
// Author-facing builder
// ---------------------------------------------------------------------------

/// Author-facing drawer navigator builder. Drawer entries get
/// declared via `.item(...)`; screens via `.screen(...)`. See
/// module-level docs for usage.
pub struct DrawerNavigator {
    pub initial: &'static str,
    pub initial_path: &'static str,
    /// Ordered list of (route name, item) — preserves declaration
    /// order, which the backend uses to render the drawer list.
    pub item_order: Vec<(&'static str, DrawerItem)>,
    pub screens: HashMap<&'static str, RouteEntry>,
    pub layout: Option<LayoutBuilder>,
    pub sidebar: Option<SidebarBuilder>,
    pub side: DrawerSide,
    /// Width breakpoint in CSS pixels above which the drawer is
    /// pinned beside the body region (becomes a sidebar). `None`
    /// (default) keeps the drawer as an overlay at all widths.
    pub pinned_above: Option<u32>,
    pub mount_policy: MountPolicy,
    pub style: Option<crate::StyleSource>,
    pub ref_fill: Option<RefFill>,
}

impl DrawerNavigator {
    /// Construct a drawer navigator with `initial` as the active
    /// screen. The route must be registered via `.screen(...)`
    /// before the navigator mounts; an unregistered initial route
    /// panics.
    pub fn new(initial: &Route<()>) -> Bound<DrawerHandle> {
        Bound::new(Primitive::DrawerNavigator(Box::new(DrawerNavigator {
            initial: initial.name(),
            initial_path: initial.path(),
            item_order: Vec::new(),
            screens: HashMap::new(),
            layout: None,
            sidebar: None,
            side: DrawerSide::Start,
            pinned_above: None,
            mount_policy: MountPolicy::LazyPersistent,
            style: None,
            ref_fill: None,
        })))
    }
}

impl Bound<DrawerHandle> {
    /// Register a drawer entry. Adds an item to the drawer list
    /// for the given route. The route's *screen* is registered
    /// separately via `.screen(...)` — see module docs for the
    /// rationale.
    pub fn item<P: RouteParams>(mut self, route: Route<P>, item: DrawerItem) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.item_order.push((route.name(), item));
        }
        self
    }

    /// Register a screen. Same shape as the stack navigator's
    /// `.screen(...)`. Routes that appear in `.screen(...)` but
    /// not in `.item(...)` are reachable via `Link` or
    /// programmatic `select(...)` but won't appear in the drawer
    /// list.
    pub fn screen<P: RouteParams>(
        mut self,
        route: Route<P>,
        render: impl Fn(P) -> Primitive + 'static,
    ) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            let render = Rc::new(render);
            let build: ScreenBuilder = Rc::new(move |boxed: Box<dyn Any>| {
                let params: Box<P> = boxed.downcast().unwrap_or_else(|_| {
                    panic!(
                        "DrawerNavigator: screen param type mismatch for route — \
                         declared params don't match dispatched params"
                    )
                });
                render(*params)
            });
            let from_segments = Rc::new(|segs: &HashMap<String, String>| {
                P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
            });
            nav.screens.insert(
                route.name(),
                RouteEntry { path: route.path(), build, from_segments },
            );
        }
        self
    }

    /// Set which edge the drawer slides in from. Default is
    /// `DrawerSide::Start` (locale-aware leading edge).
    pub fn side(mut self, side: DrawerSide) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.side = side;
        }
        self
    }

    /// Pin the drawer beside the body region above the given
    /// viewport width in CSS pixels. Below the breakpoint, the
    /// drawer behaves as an overlay.
    pub fn pinned_above(mut self, px: u32) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.pinned_above = Some(px);
        }
        self
    }

    /// Override when drawer screens are mounted and disposed.
    /// Default is `MountPolicy::LazyPersistent`.
    pub fn mount_policy(mut self, policy: MountPolicy) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.mount_policy = policy;
        }
        self
    }

    /// Install a layout wrapper. Drawer layouts typically render a
    /// top app bar containing the hamburger trigger; the
    /// `LayoutProps` bundle exposes the open-state signal (via
    /// `can_go_back` — no, see `DrawerSidebarProps` for direct
    /// access) and the toggle callback for that purpose.
    ///
    /// **Web only.** Native backends (Android, iOS) draw their own
    /// drawer shell and ignore this slot. To define the *sidebar*
    /// content portably across web and Android, use
    /// [`Bound::sidebar`] instead — the sidebar Primitive flows to
    /// the layout closure on web via `LayoutProps::sidebar`, and is
    /// rendered natively on Android.
    pub fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(super::shared::LayoutProps) -> Primitive + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.layout = Some(Rc::new(f));
        }
        self
    }

    /// Install a sidebar closure. The closure renders the drawer's
    /// side panel content (typically an entry list + brand/footer)
    /// and runs in its own reactive scope so signal reads inside
    /// it update without rebuilding the panel.
    ///
    /// # Per-target behavior
    ///
    /// - **Android**: the framework renders the sidebar as a real
    ///   native drawer side panel — open/close animations, scrim,
    ///   edge-swipe to dismiss.
    /// - **Web**: the framework builds the sidebar Primitive and
    ///   surfaces it on `LayoutProps::sidebar` for the author's
    ///   `.layout(...)` closure to place. Without a `.layout(...)`,
    ///   the sidebar is silently dropped (web has no automatic
    ///   drawer shell to receive it).
    /// - **iOS** (when implemented): same as Android.
    pub fn sidebar<F>(mut self, f: F) -> Self
    where
        F: Fn(DrawerSidebarProps) -> Primitive + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.sidebar = Some(Rc::new(f));
        }
        self
    }

    /// Bind a `Ref<DrawerHandle>`.
    pub fn bind(mut self, r: Ref<DrawerHandle>) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.ref_fill = Some(RefFill::DrawerNavigator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Callbacks bundle — what backends receive
// ---------------------------------------------------------------------------

/// Per-drawer-item registration data handed to the backend so it can
/// render the drawer list. Same shape as `DrawerItem` plus the route
/// name.
#[derive(Clone)]
pub struct DrawerItemRegistration {
    pub route: &'static str,
    pub label: String,
    pub icon: Option<String>,
}

/// Bundle the framework hands to `Backend::create_drawer_navigator`.
/// Reuses the shared `NavigatorCallbacks<N>` for the screen/scope
/// machinery and adds drawer-specific data.
pub struct DrawerNavigatorCallbacks<N: Clone + 'static> {
    pub navigator: NavigatorCallbacks<N>,
    pub items: Vec<DrawerItemRegistration>,
    pub side: DrawerSide,
    pub pinned_above: Option<u32>,
    pub mount_policy: MountPolicy,
    /// Reactive open-state signal. The backend's dispatcher flips
    /// this when `OpenDrawer` / `CloseDrawer` / `ToggleDrawer`
    /// commands fire; layouts subscribed to it re-render
    /// (hamburger icon state, focus trap, etc.).
    pub is_open: crate::Signal<bool>,
    /// Build the sidebar subtree, if one was registered via
    /// `.sidebar(...)`. Mirrors `NavigatorCallbacks::build_layout`:
    /// the framework runs the author's closure inside a dedicated
    /// reactive scope (so signal-reads in the sidebar keep firing
    /// across drawer state changes) and returns the freshly-built
    /// native node. `None` ⇒ the author didn't register a sidebar;
    /// backends that need *something* for the slot use an empty
    /// View.
    pub build_sidebar: Option<Rc<dyn Fn() -> N>>,
    /// Called by the backend after `select`/`open`/`close`
    /// commands commit (e.g. for analytics).
    pub active_changed: Rc<dyn Fn(&'static str)>,
    pub open_changed: Rc<dyn Fn(bool)>,
}
