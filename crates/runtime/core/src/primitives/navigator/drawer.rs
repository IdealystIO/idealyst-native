//! Drawer navigator — a slide-in side panel plus a switched body
//! region.
//!
//! Authors declare an initial route + screens via `.screen(...)`,
//! and render the drawer panel's contents via `.content(closure)`.
//! The user opens the drawer (hamburger button or platform
//! gesture), taps an entry rendered by the content closure, and the
//! body region swaps to that entry's screen. An imperative
//! `DrawerHandle` exposes `.select(...)`, `.open()`, `.close()`,
//! `.toggle()`, `.is_open()`.
//!
//! # Per-platform semantics
//!
//! - **iOS**: hand-rolled — UIKit has no native drawer. The backend
//!   embeds a `UINavigationController` for the body + slides a
//!   `UIView` overlay in from the requested side, with a tap-outside
//!   recognizer for dismissal. On regular-size class devices the
//!   drawer pins beside the body (a sidebar) — matches
//!   `UISplitViewController`'s posture without adopting that API's
//!   opinions.
//! - **Android**: `DrawerLayout` + `NavigationView`. Pinned-vs-modal
//!   chosen from `Configuration.screenWidthDp`.
//! - **Web**: an `<aside>` plus a body region. Pinned-vs-modal chosen
//!   from a CSS media query on the viewport width.
//!
//! Phone-vs-tablet adaptation is the backend's job, not the
//! framework's — there's no app-side knob.
//!
//! # Screens and the content panel
//!
//! Screens are declared once via `.screen(route, render)`. The
//! drawer's *panel* is rendered by the author's `.content(closure)`
//! — the closure receives a [`DrawerContentProps`] bundle with
//! navigation callbacks and reactive state so it can build whatever
//! UI the design calls for (a list of `Link`s, a brand header, a
//! settings toggle at the bottom). Per-screen header configuration
//! (title, bar buttons) goes inside the `.screen(...)` render
//! closure by returning a [`Screen`] via `Screen::new(...).title(...)`
//! instead of a bare `Primitive`.
//!
//! ```ignore
//! DrawerNavigator::new(&home)
//!     .screen(home, |_| Screen::new(home_page()).title("Home"))
//!     .screen(settings, |_| Screen::new(settings_page()).title("Settings"))
//!     .content(|props| drawer_panel(props))
//! ```

use super::shared::{
    LayoutBuilder, NavigatorCallbacks, NavigatorHandle, Route, RouteEntry, RouteParams, Screen,
    ScreenBuilder, ScreenOptions,
};
use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

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
// DrawerType — animation style
// ---------------------------------------------------------------------------

/// How the drawer animates on open/close.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DrawerType {
    /// Sidebar slides in from the edge, overlaying the body. The body
    /// stays still and is dimmed by a scrim. Default on Android;
    /// matches React Navigation's `"front"` type.
    Front,
    /// Both the sidebar and the body slide together. The body moves
    /// away to reveal the sidebar underneath. Default on iOS; matches
    /// React Navigation's `"slide"` type.
    Slide,
}


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
// Content slot — author renders the drawer's side panel content
// ---------------------------------------------------------------------------

/// Props handed to the user's `.content(...)` closure. The closure
/// renders the drawer panel's content (entry buttons, brand,
/// footer) using these reactive signals + dispatch callbacks.
///
/// # Per-target rendering
///
/// - **Android**: the framework renders the content as a native
///   side panel inside its drawer-shell (open/close animations,
///   scrim, swipe-to-dismiss).
/// - **Web**: the content subtree is surfaced on
///   `LayoutProps::sidebar` for the author's `.layout(...)` closure
///   to place (typically in a flex row beside the outlet). Without
///   a `.layout(...)`, the content is silently dropped on web.
///
/// # State + dispatch
///
/// The closure runs inside its own reactive scope, so signal reads
/// inside it (e.g. `active_route`) re-fire dependent effects on
/// navigation without rebuilding the panel. Call `on_select(name)`
/// from a button click to swap the body — or use `Link(route,
/// params)` which auto-dispatches `Select` against the ambient
/// drawer navigator. Both work; `Link` is preferred because it
/// inherits web's hyperlink semantics (middle-click new tab,
/// right-click menu, etc.).
pub struct DrawerContentProps {
    /// Name of the currently-active route. Read this inside a
    /// reactive closure (`Text { ... active_route.get() ... }`,
    /// or a `style` closure) to drive the active highlight.
    pub active_route: crate::Signal<&'static str>,
    /// Whether the drawer is currently open. On Android this is
    /// kept in sync with the native drawer's visibility; on web
    /// the author's `.layout(...)` is responsible for reading it
    /// and showing/hiding the content (or using it for animations).
    pub is_open: crate::Signal<bool>,
    /// Programmatic body swap. Dispatches a `Select` command —
    /// equivalent to `Link(route, ())` but for cases where the
    /// author wants imperative control (e.g. firing from a custom
    /// gesture).
    pub on_select: Rc<dyn Fn(&'static str)>,
    /// Close the drawer. Useful for a "Done" footer button on
    /// mobile; on desktop where the content is pinned, this still
    /// flips the signal but the layout may ignore it.
    pub on_close: Rc<dyn Fn()>,
}

pub type ContentBuilder = Rc<dyn Fn(DrawerContentProps) -> Primitive>;

// ---------------------------------------------------------------------------
// Author-facing builder
// ---------------------------------------------------------------------------

/// Author-facing drawer navigator builder. Screens get declared
/// via `.screen(...)`; the drawer panel's contents via
/// `.content(...)`. See module-level docs for usage.
pub struct DrawerNavigator {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, RouteEntry>,
    pub layout: Option<LayoutBuilder>,
    pub content: Option<ContentBuilder>,
    pub side: DrawerSide,
    /// Animation style — `Front` (overlay) or `Slide` (push content).
    /// Default is platform-aware: `Slide` on iOS, `Front` elsewhere.
    pub drawer_type: DrawerType,
    /// Width of the drawer panel in logical points. Default 280.
    pub drawer_width: f32,
    /// Whether the drawer opens on edge-swipe (in addition to
    /// programmatic open from the handle / hamburger). On Android,
    /// this maps to `DrawerLayout.setDrawerLockMode` — `true` =
    /// `LOCK_MODE_UNLOCKED`, `false` = `LOCK_MODE_LOCKED_CLOSED`
    /// (drawer can only be opened programmatically).
    ///
    /// Default `true`. Turn off for screens with horizontal
    /// content (carousels, sliders) where edge-swipe would
    /// conflict.
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    pub default_options: Option<ScreenOptions>,
    pub style: Option<crate::StyleSource>,
    pub header_style: Option<crate::StyleSource>,
    pub title_style: Option<crate::StyleSource>,
    pub button_style: Option<crate::StyleSource>,
    pub sidebar_style: Option<crate::StyleSource>,
    pub scrim_style: Option<crate::StyleSource>,
    pub ref_fill: Option<RefFill>,
    /// Background color for the navigator's body area — the surface
    /// behind each screen's content. Backends apply this to whatever
    /// view shows through any transparent regions of the mounted
    /// screen (iOS: the nav controller's root view; Android: the
    /// DrawerLayout body). Closure shape matches the per-screen
    /// `header_background`: pass `idea_color(|c| c.background.clone())`
    /// for reactive theme tracking.
    pub background_color: Option<Rc<dyn Fn() -> crate::Color>>,
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
            screens: HashMap::new(),
            layout: None,
            content: None,
            side: DrawerSide::Start,
            // Matches React Navigation's default on every platform
            // except iOS. iOS-targeted authors should set
            // `.drawer_type(DrawerType::Slide)` explicitly.
            drawer_type: DrawerType::Front,
            drawer_width: 280.0,
            swipe_to_open: true,
            mount_policy: MountPolicy::LazyPersistent,
            default_options: None,
            style: None,
            header_style: None,
            title_style: None,
            button_style: None,
            sidebar_style: None,
            scrim_style: None,
            ref_fill: None,
            background_color: None,
        })))
    }
}

impl Bound<DrawerHandle> {
    /// Register a screen. The `render` closure returns anything
    /// convertible into a [`Screen`] — either a bare `Primitive`
    /// or a `Screen::new(...).title(...).header_left(...)` value
    /// when the route also needs per-screen header configuration.
    ///
    /// To put a route in the drawer's side panel, render the entry
    /// yourself inside the `.content(...)` closure (use `Link` for
    /// hyperlink-shaped behavior, or `on_select(...)` for a
    /// programmatic switch).
    pub fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams,
        R: Into<Screen>,
        F: Fn(P) -> R + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            let render = Rc::new(render);
            let build: ScreenBuilder = Rc::new(move |boxed: Box<dyn Any>| {
                let params: Box<P> = boxed.downcast().unwrap_or_else(|_| {
                    panic!(
                        "DrawerNavigator: screen param type mismatch for route — \
                         declared params don't match dispatched params"
                    )
                });
                render(*params).into()
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

    /// Set default header options for all screens in this drawer.
    pub fn default_screen_options(mut self, opts: ScreenOptions) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.default_options = Some(opts);
        }
        self
    }

    /// Navigator-level default for the header bar's background fill.
    /// Applies to every screen unless that screen's own
    /// `.header_background(...)` overrides it. The closure shape
    /// matches `Screen::header_background` — pass `idea_color(|c|
    /// c.surface.clone())` for reactive theme tracking, or
    /// `|| my_color.clone()` for a static fill.
    pub fn header_background<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.default_options
                .get_or_insert_with(ScreenOptions::default)
                .header_background = Some(Rc::new(f));
        }
        self
    }

    /// Navigator-level default for header tint (back chevron + bar
    /// button icons). See [`Self::header_background`] for the
    /// closure shape.
    pub fn header_tint<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.default_options
                .get_or_insert_with(ScreenOptions::default)
                .header_tint = Some(Rc::new(f));
        }
        self
    }

    /// Navigator-level default for title text color. See
    /// [`Self::header_background`] for the closure shape.
    pub fn title_color<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.default_options
                .get_or_insert_with(ScreenOptions::default)
                .title_color = Some(Rc::new(f));
        }
        self
    }

    /// Background fill for the navigator's body area — shows
    /// through any transparent regions of the mounted screen. See
    /// [`Self::header_background`] for the closure shape.
    pub fn background_color<F>(mut self, f: F) -> Self
    where
        F: Fn() -> crate::Color + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.background_color = Some(Rc::new(f));
        }
        self
    }

    /// Bundled header style. The closure returns a
    /// [`crate::HeaderStyle`] populated with whichever fields the
    /// author wants to drive — each `None` field falls through to
    /// the platform default. The closure is re-invoked on every
    /// theme swap (when its body reads `active_theme()` directly or
    /// via a helper like idea-ui's `idea_color`), so the bar and
    /// body retint reactively without per-screen wiring.
    ///
    /// Equivalent to calling [`Self::header_background`],
    /// [`Self::title_color`], [`Self::header_tint`], and
    /// [`Self::background_color`] with closures that each project a
    /// field off the same `HeaderStyle`. Use the granular setters
    /// when you only need one slot or want them driven by different
    /// closures.
    ///
    /// The closure is probed once at build time to decide which
    /// slots to wire — fields that are `None` on the initial probe
    /// stay platform-default and aren't re-evaluated. Toggling a
    /// field between `Some` and `None` at runtime isn't supported;
    /// always return the same set of `Some` fields.
    pub fn header<F>(self, f: F) -> Self
    where
        F: Fn() -> crate::HeaderStyle + 'static,
    {
        let f = Rc::new(f);
        let probe = f();
        let mut s = self;
        if probe.background.is_some() {
            let f = f.clone();
            s = s.header_background(move || {
                f().background.expect("HeaderStyle.background must stay Some after the initial probe")
            });
        }
        if probe.title.is_some() {
            let f = f.clone();
            s = s.title_color(move || {
                f().title.expect("HeaderStyle.title must stay Some after the initial probe")
            });
        }
        if probe.tint.is_some() {
            let f = f.clone();
            s = s.header_tint(move || {
                f().tint.expect("HeaderStyle.tint must stay Some after the initial probe")
            });
        }
        if probe.body_background.is_some() {
            let f = f.clone();
            s = s.background_color(move || {
                f().body_background.expect("HeaderStyle.body_background must stay Some after the initial probe")
            });
        }
        s
    }

    /// Style the drawer navigator's header bar.
    pub fn header_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.header_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the drawer navigator's title text.
    pub fn title_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.title_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the drawer navigator's bar button items.
    pub fn button_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.button_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the drawer's sidebar panel.
    pub fn sidebar_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.sidebar_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the drawer's scrim (background overlay when open).
    pub fn scrim_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.scrim_style = Some(s.into_style_source());
        }
        self
    }

    /// Set the drawer animation type. Default is platform-aware:
    /// `Slide` on iOS, `Front` on Android/web.
    pub fn drawer_type(mut self, dt: DrawerType) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.drawer_type = dt;
        }
        self
    }

    /// Set the drawer panel width in logical points. Default 280.
    pub fn drawer_width(mut self, width: f32) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.drawer_width = width;
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

    /// Toggle the swipe-from-edge gesture. Default is on.
    ///
    /// On Android, this controls
    /// `DrawerLayout.setDrawerLockMode(LOCK_MODE_*)`: when off, the
    /// drawer can only be opened programmatically (via the handle
    /// or a hamburger button calling `OpenDrawer`). Turn off when
    /// the drawer's content includes horizontal-swipe surfaces
    /// (carousels, sliders) that would conflict with edge-swipe.
    ///
    /// On web this setting has no effect — there's no native
    /// gesture to lock.
    pub fn swipe_to_open(mut self, enabled: bool) -> Self {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.swipe_to_open = enabled;
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
    /// `LayoutProps` bundle exposes the active route, the back
    /// callback (which toggles the drawer), and the pre-built
    /// drawer-content `Primitive` (see [`DrawerContentProps`] for the
    /// reactive state passed to the content closure).
    ///
    /// **Web only.** Native backends (Android, iOS) draw their own
    /// drawer shell and ignore this slot. To define the drawer
    /// panel's contents portably across web and native, use
    /// [`Bound::content`] — the content Primitive flows to the
    /// layout closure on web via `LayoutProps::sidebar` and is
    /// rendered natively on Android/iOS.
    pub fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(super::shared::LayoutProps) -> Primitive + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.layout = Some(Rc::new(f));
        }
        self
    }

    /// Install a content closure. The closure renders the drawer
    /// panel's contents (typically an entry list + brand + footer)
    /// and runs in its own reactive scope so signal reads inside
    /// it update without rebuilding the panel.
    ///
    /// # Per-target behavior
    ///
    /// - **Android / iOS**: the framework renders the content as
    ///   a native drawer side panel — open/close animations, scrim,
    ///   edge-swipe to dismiss.
    /// - **Web**: the framework builds the content Primitive and
    ///   surfaces it on `LayoutProps::sidebar` for the author's
    ///   `.layout(...)` closure to place. Without a `.layout(...)`,
    ///   the content is silently dropped (web has no automatic
    ///   drawer shell to receive it).
    pub fn content<F>(mut self, f: F) -> Self
    where
        F: Fn(DrawerContentProps) -> Primitive + 'static,
    {
        if let Primitive::DrawerNavigator(nav) = &mut self.primitive {
            nav.content = Some(Rc::new(f));
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

/// Bundle the framework hands to `Backend::create_drawer_navigator`.
/// Reuses the shared `NavigatorCallbacks<N>` for the screen/scope
/// machinery and adds drawer-specific data.
pub struct DrawerNavigatorCallbacks<N: Clone + 'static> {
    pub navigator: NavigatorCallbacks<N>,
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub swipe_to_open: bool,
    pub mount_policy: MountPolicy,
    /// Reactive open-state signal. The backend's dispatcher flips
    /// this when `OpenDrawer` / `CloseDrawer` / `ToggleDrawer`
    /// commands fire; layouts subscribed to it re-render
    /// (hamburger icon state, focus trap, etc.).
    pub is_open: crate::Signal<bool>,
    /// Build the drawer-panel content subtree, if one was registered
    /// via `.content(...)`. Mirrors `NavigatorCallbacks::build_layout`:
    /// the framework runs the author's closure inside a dedicated
    /// reactive scope (so signal-reads inside the panel keep firing
    /// across drawer state changes) and returns the freshly-built
    /// native node. `None` ⇒ the author didn't register a content
    /// closure; backends that need *something* for the slot use an
    /// empty View.
    pub build_content: Option<Rc<dyn Fn() -> N>>,
    /// Called by the backend after `select`/`open`/`close`
    /// commands commit (e.g. for analytics).
    pub active_changed: Rc<dyn Fn(&'static str)>,
    pub open_changed: Rc<dyn Fn(bool)>,
    /// Reactive background-color closure for the navigator's body
    /// area. Backends that apply it (iOS sets `nav_view.backgroundColor`,
    /// Android sets the DrawerLayout body's background) wrap a call
    /// to this in a per-drawer Effect, so the body re-tints on
    /// theme swap. `None` ⇒ keep the platform default.
    pub background_color: Option<Rc<dyn Fn() -> crate::Color>>,
}
