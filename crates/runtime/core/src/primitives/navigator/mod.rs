//! Navigator primitive.
//!
//! Core ships exactly one navigator primitive (`Element::Navigator`)
//! plus its substrate. Specific navigator kinds (stack, tabs, drawer,
//! anything third-party) live in SDK crates under `crates/sdk/client/` and
//! register handlers into each backend's `NavigatorRegistry`.
//!
//! The framework never names any kind. Routing, screen scopes,
//! ambient capture, hardware-back coordination, reactive nav state —
//! all kind-agnostic. SDK crates own chrome, animations, gestures,
//! per-kind commands, typed handles, typed screen options.
//!
//! Module layout:
//! - `shared` — `Route`, `NavCommand`, `NavigatorControl`,
//!   `NavigatorHandle`, `NavState`, `NavigatorConfig`, screen-state
//!   stack, ambient-navigator stack, path matching.
//! - `host` — `NavigatorHost<N>` (the bundle handed to SDK handlers)
//!   and the `NavigatorHandler<B>` trait SDKs implement per backend.
//! - `registry` — `NavigatorRegistry<B>` keyed by presentation TypeId.

pub mod host;
pub mod registry;
pub mod scroll;
pub mod shared;
pub mod url_sync;

pub use host::{NavigatorHandler, NavigatorHost};
pub use registry::{NavigatorHandlerFactory, NavigatorRegistry, RegisterNavigator};
pub use scroll::{ambient_scroll_context, ScrollContext};
pub use url_sync::{
    handle_popstate, install_url_provider, reset_url_sync_for_tests, url_provider_installed,
    UrlProvider,
};
pub use shared::{
    ambient_navigator, capture_ambient_nav_context, current_nav_base, current_screen_route,
    consumed_prefix, current_screen_state, enable_route_collector, join_path, match_pattern,
    match_prefix,
    navigator_fill_rules, navigator_outlet, outlet_fill_rules, peek_initial_path, record_routes,
    set_initial_path, stack_container_rules, stack_screen_fill_rules, take_initial_path,
    NAV_ROOT_HYDRATION_CLASS,
    take_route_collector, use_can_go_back, use_focus, AmbientNavContext, AmbientNavContextGuard,
    AmbientNavGuard,
    HeaderButton, MountResult, NavBaseGuard, NavCommand, NavId, NavState, NavigatorConfig,
    NavigatorControl, NavigatorHandle, NavigatorOps, ParamsFromSegments, Route, RouteEntry,
    RouteParams, Screen, ScreenBuilder, ScreenNav, ScreenRouteGuard, ScreenStateGuard,
    StackHeaderState, SwapContext,
};

/// Robot-only navigator introspection surface (global registry + snapshots).
#[cfg(feature = "robot")]
pub use shared::{
    all_navigators, deregister_navigator, navigator_snapshot, register_navigator, NavSnapshot,
};

#[cfg(all(test, feature = "robot"))]
pub(crate) use shared::nav_registry_reset;
