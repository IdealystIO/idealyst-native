//! Navigator substrate — the Element-free half of navigation.
//!
//! `shared` here carries the kind-agnostic substrate (registry,
//! routing, control plane, reactive nav state, screen-state stacks,
//! fill-rule sheets); `scroll` the per-navigator scroll persistence;
//! `url_sync` the web URL synchronization; `query` the screen-state
//! query encoding. The Element-coupled items (`Screen`, `NavConfig`,
//! `navigator_outlet`) and the navigator handlers live in
//! `runtime-vocabulary` (`prims::navigator`, `handlers::navigator`),
//! whose glue re-exports this module's data types (routes, query state,
//! header slots) under `runtime_core::primitives::navigator`.

pub mod query;
pub mod scroll;
pub mod shared;
pub mod url_sync;

pub use query::{
    split_query, strip_query, with_query, QueryParams, ScreenState,
};
pub use scroll::{ambient_scroll_context, ScrollContext};
pub use url_sync::{
    handle_popstate, install_url_provider, reset_url_sync_for_tests, url_provider_installed,
    UrlProvider,
};
pub use shared::{
    ambient_navigator, capture_ambient_nav_context, current_nav_base, current_screen_route,
    consumed_prefix, enable_route_collector, join_path, match_pattern,
    match_prefix, screen_query, screen_state,
    navigator_fill_rules, outlet_fill_rules, peek_initial_path,
    record_route_paths,
    screen_flow_fill_rules, set_initial_path, stack_container_rules, stack_screen_fill_rules,
    take_initial_path,
    NAV_OUTLET_HYDRATION_ATTR, NAV_ROOT_HYDRATION_CLASS,
    take_route_collector, use_can_go_back, use_focus, AmbientNavContext, AmbientNavContextGuard,
    AmbientNavGuard,
    HeaderButton, MountResult, NavBaseGuard, NavCommand, NavId, NavState,
    NavigatorControl, NavigatorHandle, NavigatorOps, Route,
    RouteParams, ScreenNav, ScreenRouteGuard, ScreenStateGuard,
    StackHeaderState,
};

/// Robot-only navigator introspection surface (global registry + snapshots).
#[cfg(feature = "robot")]
pub use shared::{
    all_navigators, deregister_navigator, navigator_snapshot, register_navigator, NavSnapshot,
};

/// Test-support reset for the robot nav registry. Doc-hidden `pub`
/// (not `pub(crate)`): tests in other crates reach it across the crate
/// boundary.
#[cfg(feature = "robot")]
#[doc(hidden)]
pub use shared::nav_registry_reset;
