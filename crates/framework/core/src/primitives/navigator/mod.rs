//! Navigator primitives.
//!
//! A *navigator* is a primitive that owns one-or-more registered
//! screens and decides which one is active. The framework ships
//! multiple navigator kinds — stack, tabs, drawer — that share a
//! large substrate (typed routes, per-screen reactive scopes,
//! ambient-navigator capture for the `Link` primitive, URL
//! path-matching) and differ only in their *active-screen-selection
//! UI* and *command shape*.
//!
//! Module layout:
//!
//! - `shared` — the substrate. `Route<P>`, `RouteParams`,
//!   `NavCommand`, `NavigatorControl`, `NavigatorHandle`,
//!   `NavigatorCallbacks`, `NavState`, layout machinery,
//!   ambient-navigator stack, path-matching, screen-entry
//!   bookkeeping. Every navigator kind uses these.
//! - `stack` — the `Navigator` builder. Push/pop/replace/reset
//!   semantics; backed by `UINavigationController` on iOS,
//!   `FragmentManager` on Android, inline subtree swap on web.
//!
//! New navigator kinds (tabs, drawer) will land as additional
//! submodules alongside `stack`, each one a thin builder over the
//! shared substrate.
//!
//! For API continuity, the historical `primitives::navigator::Foo`
//! paths still resolve — everything is re-exported at this module's
//! root.

pub mod drawer;
pub mod shared;
pub mod stack;
pub mod tabs;

pub use drawer::{
    ContentBuilder, DrawerContentProps, DrawerHandle, DrawerNavigator, DrawerNavigatorCallbacks,
    DrawerSide, DrawerType,
};
pub use shared::{
    ambient_navigator, match_pattern, AmbientNavGuard, DefaultLinkKind, HeaderButton, HeaderStyle,
    LayoutBuilder, LayoutPlan, LayoutProps, MountResult, NavCommand, NavState, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, NavigatorOps, ParamsFromSegments, Route, RouteEntry,
    RouteParams, Screen, ScreenBuilder, ScreenOptions,
};
pub use stack::Navigator;
pub use tabs::{
    MountPolicy, TabNavigator, TabNavigatorCallbacks, TabPlacement, TabRegistration, TabSpec,
    TabsHandle,
};
