//! Stack navigator — the historical `Navigator` primitive.
//!
//! Authors declare a set of `Screen` routes up-front and an initial
//! route; an imperative `NavigatorHandle` (obtained via `.bind(ref)`)
//! drives push / pop / replace / reset at runtime.
//!
//! # Per-platform semantics
//!
//! - **iOS**: the navigator is a `UINavigationController`. Each pushed
//!   screen is a child `UIViewController` whose `view` is the screen
//!   subtree's root. Back-swipe + nav bar come for free.
//! - **Android**: the navigator is a `FrameLayout` driven by a
//!   `FragmentManager`. Each push commits a new `Fragment` whose view
//!   is the screen subtree's root and adds it to the back stack so the
//!   system back button pops correctly.
//! - **Web**: the navigator is a plain container that holds the
//!   active screen inline. push/pop swap the subtree atomically.
//!   URL pathing is wired through `history.pushState` / `popstate`.
//!
//! # Lifecycles
//!
//! Each *mounted* screen runs inside its own reactive `Scope`. Popping
//! drops that scope, freeing every signal/effect/ref scoped to the
//! screen. The pattern mirrors `Virtualizer`'s per-item scopes: backends
//! get `mount_screen(idx, params)` + `release_screen(scope_id)`
//! callbacks; the framework owns the scope registry.
//!
//! # Route params
//!
//! Routes are typed via the generic param `P`:
//!
//! ```ignore
//! let home = Route::<()>::new("home");
//! let detail = Route::<DetailParams>::new("detail");
//! ```
//!
//! `nav.push(&detail, DetailParams { id: 42 })` is a compile-time check
//! that the params match the route. Inside the framework the params get
//! boxed into `Box<dyn Any>` so the navigator's screen table stays
//! non-generic; each registered screen builder downcasts back to its
//! declared param type before calling the user's render closure. A
//! mismatch (e.g. user constructs a route at runtime with the wrong
//! param) panics in the renderer with a clear message.

use super::shared::{
    LayoutBuilder, LayoutProps, Route, RouteEntry, RouteParams, ScreenBuilder, ScreenOptions,
};
use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use super::shared::NavigatorHandle;

/// Author-facing stack navigator builder. Routes get declared via
/// `.screen(...)`; the framework wires the rest. See module-level
/// docs for usage.
pub struct Navigator {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, RouteEntry>,
    pub layout: Option<LayoutBuilder>,
    pub default_options: Option<ScreenOptions>,
    pub style: Option<crate::StyleSource>,
    pub header_style: Option<crate::StyleSource>,
    pub title_style: Option<crate::StyleSource>,
    pub button_style: Option<crate::StyleSource>,
    pub ref_fill: Option<RefFill>,
}

impl Navigator {
    /// Construct a stack navigator with `initial` as the root screen.
    /// The route must be registered via `.screen(...)` before the
    /// navigator mounts; an unregistered initial route panics.
    ///
    /// The initial route's params are always `()` — the root screen
    /// is unparameterized by construction. Apps that need a
    /// parameterized "deep-link" root should rely on web's
    /// path-matching: declare the screen normally, and the web
    /// backend will mount it as the root when the URL matches.
    pub fn new(initial: &Route<()>) -> Bound<NavigatorHandle> {
        Bound::new(Primitive::Navigator(Box::new(Navigator {
            initial: initial.name(),
            initial_path: initial.path(),
            screens: HashMap::new(),
            layout: None,
            default_options: None,
            style: None,
            header_style: None,
            title_style: None,
            button_style: None,
            ref_fill: None,
        })))
    }
}

/// Builder methods. Wrapping in `Bound<NavigatorHandle>` keeps the
/// `.bind(ref)` type-check working — same pattern as every other
/// primitive's builder.
impl Bound<NavigatorHandle> {
    /// Register a screen. `route` is the typed key; `render` is the
    /// per-route subtree builder, which receives the route's typed
    /// params. The `'static` bound on `P` is required to box the
    /// params across the framework's type-erased boundary; the
    /// `RouteParams` bound is what lets web/SSR backends map URLs to
    /// typed payloads.
    pub fn screen<P: RouteParams>(
        mut self,
        route: Route<P>,
        render: impl Fn(P) -> Primitive + 'static,
    ) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            let render = Rc::new(render);
            let build: ScreenBuilder = Rc::new(move |boxed: Box<dyn Any>| {
                let params: Box<P> = boxed.downcast().unwrap_or_else(|_| {
                    panic!(
                        "Navigator: screen param type mismatch for route — \
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
                RouteEntry { path: route.path(), build, from_segments, options: None },
            );
        }
        self
    }

    /// Set per-screen header options for `route`. The closure
    /// receives the route's typed params and returns `ScreenOptions`.
    /// Called at mount time — not reactive.
    pub fn options<P: RouteParams>(
        mut self,
        route: Route<P>,
        f: impl Fn(&P) -> ScreenOptions + 'static,
    ) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            if let Some(entry) = nav.screens.get_mut(route.name()) {
                entry.options = Some(Rc::new(move |any: &dyn Any| {
                    let params = any.downcast_ref::<P>().expect(
                        "Navigator::options: param type mismatch",
                    );
                    f(params)
                }));
            }
        }
        self
    }

    /// Set default header options for all screens in this navigator.
    /// Per-screen `.options()` overrides these.
    pub fn default_screen_options(mut self, opts: ScreenOptions) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.default_options = Some(opts);
        }
        self
    }

    /// Style the navigator's header bar (background, shadow, etc.).
    /// Reactive: signals read inside a reactive closure re-fire the
    /// style on theme changes.
    pub fn header_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.header_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the navigator's title text (color, font size, weight).
    pub fn title_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.title_style = Some(s.into_style_source());
        }
        self
    }

    /// Style the navigator's bar button items (back arrow, action buttons).
    pub fn button_style(mut self, s: impl crate::IntoStyleSource) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.button_style = Some(s.into_style_source());
        }
        self
    }

    /// Install a layout component — a chrome wrapper that the
    /// framework renders around the active screen. Useful on web
    /// (and any future DOM-based backend) for things native nav
    /// controllers handle automatically: top bars, sidebars,
    /// breadcrumbs.
    ///
    /// The closure receives a [`LayoutProps`] bundle whose fields
    /// are reactive signals. Reading any of them inside the
    /// layout's `ui!` body subscribes the effect — the layout
    /// re-renders only the parts that read changed signals, not
    /// the whole subtree. `LayoutProps::outlet` is the slot the
    /// framework physically reuses on each push/pop, so the
    /// surrounding chrome doesn't rebuild when screens swap.
    ///
    /// **Native backends ignore this.** UIKit's
    /// `UINavigationController` and Android's `FragmentManager`
    /// draw their own chrome (nav bar, action bar, swipe-to-back);
    /// inserting a user layout there would just fight the platform.
    /// The layout closure is invoked only by backends that opt in.
    pub fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(LayoutProps) -> Primitive + 'static,
    {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.layout = Some(Rc::new(f));
        }
        self
    }

    /// Bind a `Ref<NavigatorHandle>` so the handle is filled at mount
    /// time. Matches the standard primitive bind shape.
    pub fn bind(mut self, r: Ref<NavigatorHandle>) -> Self {
        if let Primitive::Navigator(nav) = &mut self.primitive {
            nav.ref_fill = Some(RefFill::Navigator(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
