//! Navigator primitives.
//!
//! Core ships exactly one navigator primitive — `Primitive::Navigator`.
//! Specific navigator kinds (stack / tabs / drawer / third-party) live
//! in SDK crates under `crates/sdk/` and register handlers into each
//! backend's `NavigatorRegistry`.
//!
//! Module layout:
//!
//! - `shared` — the substrate. `Route<P>`, `RouteParams`, `NavCommand`,
//!   `NavigatorControl`, `NavigatorHandle`, `NavigatorCallbacks`,
//!   `NavState`, layout machinery, ambient-navigator stack, path-matching,
//!   per-screen state stack. Every navigator kind uses these.
//! - `host` — `NavigatorHost` (framework affordances handed to the
//!   handler), `NavigatorHandler` trait, `NavigatorKind`.
//! - `registry` — `NavigatorRegistry<B>` keyed by presentation TypeId.
//! - `tabs` — kind-specific substrate types (TabSpec, TabPlacement,
//!   TabRegistration, MountPolicy, TabsHandle, TabNavigatorCallbacks).
//!   Used by backend inherent helpers + SDK adapters.
//! - `drawer` — kind-specific substrate types (DrawerSide, DrawerType,
//!   DrawerContentProps, ContentBuilder, DrawerHandle,
//!   DrawerNavigatorCallbacks). Same pattern as `tabs`.
//!
//! The author-facing builder structs (`Navigator`, `TabNavigator`,
//! `DrawerNavigator`) used to live here in `stack.rs` / `tabs.rs` /
//! `drawer.rs` but have been moved to their respective SDK crates.

pub mod drawer;
pub mod host;
pub mod registry;
pub mod shared;
pub mod tabs;

pub use drawer::{
    ContentBuilder, DrawerContentProps, DrawerHandle, DrawerNavigatorCallbacks, DrawerSide,
    DrawerType,
};
pub use host::{NavigatorKind, NavigatorHandler, NavigatorHost};
pub use registry::{NavigatorHandlerFactory, NavigatorRegistry};
pub use shared::{
    ambient_navigator, current_screen_state, match_pattern, AmbientNavGuard, DefaultLinkKind,
    HeaderButton, HeaderStyle, LayoutBuilder, LayoutPlan, LayoutProps, MountResult, NavCommand,
    NavState, NavigatorCallbacks, NavigatorControl, NavigatorConfig, NavigatorHandle,
    NavigatorOps, ParamsFromSegments, Route, RouteEntry, RouteParams, Screen, ScreenBuilder,
    ScreenOptions, ScreenStateGuard,
};
pub use tabs::{
    MountPolicy, TabNavigatorCallbacks, TabPlacement, TabRegistration, TabSpec, TabsHandle,
};
