//! Navigator primitive.
//!
//! Core ships exactly one navigator primitive (`Element::Navigator`)
//! plus its substrate. Specific navigator kinds (stack, tabs, drawer,
//! anything third-party) live in SDK crates under `crates/sdk/` and
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

pub use host::{NavigatorHandler, NavigatorHost};
pub use registry::{NavigatorHandlerFactory, NavigatorRegistry};
pub use scroll::{ambient_scroll_context, ScrollContext};
pub use shared::{
    ambient_navigator, current_screen_state, match_pattern, AmbientNavGuard, MountResult,
    NavCommand, NavState, NavigatorConfig, NavigatorControl, NavigatorHandle, NavigatorOps,
    ParamsFromSegments, Route, RouteEntry, RouteParams, Screen, ScreenBuilder, ScreenStateGuard,
};
