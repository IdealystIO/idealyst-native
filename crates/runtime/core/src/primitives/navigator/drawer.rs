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

use super::shared::{NavigatorCallbacks, NavigatorHandle, Route, RouteParams};
use crate::Primitive;
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
                state: None,
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
// ---------------------------------------------------------------------------
// Callbacks bundle — what backends receive
// ---------------------------------------------------------------------------
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
