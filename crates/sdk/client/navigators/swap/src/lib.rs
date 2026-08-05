//! First-party **Swap** navigator SDK — a flat set of co-equal screens
//! the user switches between (`Select`), with **author-supplied chrome**.
//!
//! This replaces the separate `tab` and `drawer` navigators. There is no
//! push/pop depth: selecting a screen swaps the one visible screen. What
//! used to be a "tab bar" or a "drawer panel" is now just ordinary author
//! layout wrapped around the navigator's single **outlet** — the analog
//! of react-router's `<Outlet/>`:
//!
//! ```ignore
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<SwapHandle> = Ref::new();
//!
//! SwapNavigator::new(&home)
//!     .screen(home.clone(), |_| Screen::new(/* … */))
//!     .screen(settings.clone(), |_| Screen::new(/* … */))
//!     // The layout OWNS the tree and splats `{nav.outlet}`. "Tab bar" =
//!     // wrap the outlet in a bar; "drawer" = wrap it in an idea-ui Drawer.
//!     .layout(|nav| ui! {
//!         view {
//!             { nav.outlet }
//!             TabBar(active = nav.active_route, on_select = nav.on_select) { /* … */ }
//!         }
//!     })
//!     .bind(nav.clone());
//! ```
//!
//! # One handler, every host
//!
//! The authored surface here lowers to the vocabulary's
//! [`swap_navigator`](runtime_vocabulary::builders::swap_navigator)
//! builder, and the runtime side is the vocabulary's own swap handler —
//! installed on EVERY host by `register_builtins`. There are no
//! per-backend twins to drift apart (the bug that made the old tab
//! navigator panic on web), and no registration call for an app to
//! forget: [`register`] exists only so historical bootstrap code keeps
//! compiling.
//!
//! Construction lowers to the vocabulary builder, the layout closure
//! adapts through the [`SwapNav`] world context the navigator mount
//! provides, `.bind` rides `.on_handle` (the unified [`NavHandle`]), and
//! the outlet is the plain `navigator_outlet()` element handed back as
//! `nav.outlet`.
//!
//! Selecting a screen dispatches a `Select` command; a `link` inside a
//! swap screen is rewritten to `Select` by the installed link activator
//! (so links switch, never push).
//!
//! **URL sync** needs no opt-in: every navigator registers with the
//! host-installed `handlers::nav_url_sync::UrlSyncService` automatically
//! (backend-web installs it at boot).
//!
//! # Sizing
//!
//! The navigator's root **fills its container by default** (width/height
//! 100% + `flex-grow: 1` — `navigator_fill_rules`), so an app whose root
//! is a navigator fills the viewport on every backend. The **outlet
//! fills too**: a style-less `{nav.outlet}` defaults to a bounded,
//! fillable flex region (`flex: 1 1 0` + `min-height: 0` —
//! `outlet_fill_rules`), so screens that assume they can fill — and
//! scroll views that need a bounded height — work with zero
//! configuration. Override either by styling it directly:
//! `.with_style(...)` on the navigator builder,
//! `ctx.outlet.with_style(...)` on the outlet.
//!
//! # The outlet is one-shot — keep it in one stable spot
//!
//! `ctx.outlet` is a non-`Clone` value splatted exactly once; it cannot be
//! branched into a reactive `if`/`when` (see [`SwapContext`]). Responsive
//! layouts keep the outlet pinned and reactively restyle the chrome around
//! it — or use `idea_ui_nav::AppShell`, which packages the pinned-sidebar ⇄
//! drawer shape with a single sidebar build.

#![deny(missing_docs)]

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_vocabulary::builders::{self, navigator_outlet};
use runtime_vocabulary::glue::{inject, ChildList, Element, IntoElement, Ref, Signal};
use runtime_vocabulary::prims::{NavHandle, SwapNav};

pub use runtime_vocabulary::glue::{Route, RouteParams};
pub use runtime_vocabulary::prims::{MountPolicy, Screen};

/// Presentation-label marker. There is no typed navigator payload any
/// more; the label string below is what introspection serves as
/// `NavSnapshot::type_name`, and this ZST keeps the historical name
/// resolvable for app code that spelled it.
pub struct SwapPresentation;

/// The presentation label reported by navigator introspection (and by
/// the wire, for a recorded session) — frozen at the historical
/// payload-struct path so snapshots stay comparable.
const SWAP_LABEL: &str = "swap_navigator::SwapPresentation";

/// Per-swap-screen options. Empty today (swap screens draw no navigator
/// chrome of their own), kept as a named type so per-screen metadata
/// can be added without an API break.
#[derive(Default, Clone)]
pub struct SwapScreenOptions {}

impl SwapScreenOptions {
    /// Empty options (`Default`).
    pub fn new() -> Self {
        Self::default()
    }
}

/// The value a swap navigator hands its `.layout(|nav| …)` closure.
/// Splat [`outlet`](Self::outlet) exactly once where the active screen
/// renders. Not `Clone` (the outlet is a one-shot element).
pub struct SwapContext {
    /// Splat into the layout (`{nav.outlet}`) where the active screen
    /// mounts.
    pub outlet: Element,
    /// Currently active route key — highlight the live tab/nav item.
    pub active_route: Signal<&'static str>,
    /// Full resolved path of the active screen.
    pub active_path: Signal<String>,
    /// Switch to a sibling screen by route name (`Select`).
    pub on_select: Rc<dyn Fn(&'static str)>,
}

/// Typed runtime handle to a live swap navigator, filled into the
/// [`Ref`] passed to [`SwapBuilder::bind`]. Wraps the vocabulary's
/// unified [`NavHandle`]. Cheap to clone.
/// Equal exactly when the two handles drive the same navigator —
/// DERIVED, because the wrapper adds no state and `NavHandle` already
/// answers the identity question (pointer equality on its dispatch
/// closure). A hand-written impl here would be a second copy of the same
/// rule, free to drift from the vocabulary's.
#[derive(Clone, PartialEq, Eq)]
pub struct SwapHandle {
    inner: NavHandle,
}

impl SwapHandle {
    /// Wrap the vocabulary handle (called by the `.bind` glue; authors
    /// get one from [`SwapBuilder::bind`]).
    pub fn from_inner(inner: NavHandle) -> Self {
        Self { inner }
    }

    /// Switch to `route`, building its URL from typed `params`.
    /// Selecting the already-active screen is a no-op at the driver.
    pub fn select<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        self.inner.select(route, params);
    }

    /// Borrow the underlying kind-agnostic [`NavHandle`].
    pub fn inner(&self) -> &NavHandle {
        &self.inner
    }
}

/// The swap-navigator builder. [`SwapNavigator::new`] starts one; the
/// fluent [`SwapBuilder`] methods register screens, set the author
/// layout, and bind the `Ref`. The result drops into a `ui!` tree
/// (it coerces to an `Element`).
pub struct SwapNavigator {
    b: builders::SwapNavigatorBuilder,
}

impl SwapNavigator {
    /// Start a swap navigator whose initial (selected) screen is
    /// `initial`.
    pub fn new(initial: &Route<()>) -> Self {
        Self {
            b: builders::swap_navigator(initial).nav_label(SWAP_LABEL),
        }
    }
}

/// Fluent builder methods for the swap navigator. A trait (not inherent
/// methods) so the builder surface stays swappable underneath.
pub trait SwapBuilder: Sized {
    /// Register a screen: its route and the closure that builds the
    /// screen from typed params.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set the author layout — the closure owns the chrome tree and
    /// splats `{nav.outlet}` where the active screen renders.
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static;
    /// Set the screen mount lifecycle — see [`MountPolicy`].
    fn mount_policy(self, policy: MountPolicy) -> Self;
    /// Bind a [`Ref<SwapHandle>`] so the app can switch screens
    /// imperatively.
    fn bind(self, r: Ref<SwapHandle>) -> Self;
}

impl SwapBuilder for SwapNavigator {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        self.b = self.b.screen(route, render);
        self
    }

    fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(SwapContext) -> Element + 'static,
    {
        // The vocabulary layout closure takes no argument; the mount
        // provides `SwapNav` for the build window — adapt it back into
        // the old context-parameter shape.
        self.b = self.b.layout(move || {
            let nav = inject::<SwapNav>()
                .expect("swap-navigator: SwapNav provided by the navigator mount");
            f(SwapContext {
                outlet: navigator_outlet().build(),
                active_route: nav.active_route,
                active_path: nav.active_path,
                on_select: nav.on_select,
            })
        });
        self
    }

    fn mount_policy(mut self, policy: MountPolicy) -> Self {
        self.b = self.b.mount_policy(policy);
        self
    }

    fn bind(mut self, r: Ref<SwapHandle>) -> Self {
        self.b = self
            .b
            .on_handle(move |handle| r.fill(SwapHandle::from_inner(handle)));
        self
    }
}

impl IntoElement for SwapNavigator {
    fn into_element(self) -> Element {
        self.b.build()
    }
}

impl ChildList for SwapNavigator {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into_element());
    }
}

// NOTE: this SDK intentionally exposes NO registration seam. The swap
// handler is a vocabulary built-in installed by `register_builtins` on
// every host — including generic-registry (SSR / test) hosts — so there
// is nothing for an app to register. The 1.0-era no-op `register` /
// `register_generic` shims were removed in 1.1.0: an unconstrained
// `fn register<B>(_: &mut B) {}` accepts a `&mut Registry<H>` happily,
// so a caller who assumed the usual seam convention got a call that
// compiled, did nothing, and sent them debugging a registration they
// had already "fixed".

/// Convenience re-exports — glob-import to bring the builder, handle,
/// screen options, and value types into scope, including the shared data
/// surface (`Route`, `Screen`, `SwapContext`), so an app imports
/// everything from here.
pub mod prelude {
    pub use super::{
        MountPolicy, Route, Screen, SwapBuilder, SwapContext, SwapHandle,
        SwapNavigator, SwapPresentation, SwapScreenOptions,
    };
}
