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
//! swap_navigator::register(&mut backend);
//!
//! let home = Route::<()>::new("home", "/");
//! let nav: Ref<SwapHandle> = Ref::new();
//!
//! SwapNavigator::new(&home)
//!     .screen(home.clone(), |_| Screen::new(/* … */))
//!     .screen(settings.clone(), |_| Screen::new(/* … */))
//!     // The layout OWNS the tree and splats `{nav.outlet}`. "Tab bar" =
//!     // wrap the outlet in a bar; "drawer" = wrap it in an idea-ui Drawer.
//!     .layout(|nav| ui! {
//!         Column {
//!             { nav.outlet }
//!             TabBar(active = nav.active_route, on_select = nav.on_select) { /* … */ }
//!         }
//!     })
//!     .bind(nav.clone());
//! ```
//!
//! # One backend-neutral handler
//!
//! Because chrome is author layout, the handler drives everything through
//! the framework's [`NavigatorHost`] callbacks
//! ([`build_layout_with_outlet`](NavigatorHost::build_layout_with_outlet),
//! [`mount_screen`](NavigatorHost::mount_screen),
//! [`insert_node`](NavigatorHost::insert_node),
//! [`clear_children`](NavigatorHost::clear_children)) plus
//! [`schedule_microtask`](runtime_core::schedule_microtask). So ONE
//! [`SwapHandler`] serves every backend — there are no per-backend twins to
//! drift apart (the bug that made the old tab navigator panic on web). The
//! per-backend modules below only submit the self-registration inventory
//! entry.
//!
//! Selecting a screen dispatches `NavCommand::Select`; a `Link` inside a
//! swap screen is rewritten to `Select` by the installed link activator
//! (so links switch, never push).
//!
//! # Sizing
//!
//! The navigator's root **fills its container by default** (width/height
//! 100% + `flex-grow: 1` — see `navigator_fill_rules` in `runtime-core`), so
//! an app whose root is a navigator fills the viewport on every backend.
//! The **outlet fills too**: a style-less `{nav.outlet}` defaults to a
//! bounded, fillable flex region (`flex: 1 1 0` + `min-height: 0` — see
//! `outlet_fill_rules`), so screens that assume they can fill — and scroll
//! views that need a bounded height — work with zero configuration.
//! Override either by styling it directly: `.with_style(...)` on the
//! navigator builder, `ctx.outlet.with_style(...)` on the outlet.
//!
//! # The outlet is one-shot — keep it in one stable spot
//!
//! `ctx.outlet` is a non-`Clone` value splatted exactly once; it cannot be
//! branched into a reactive `if`/`when` (see [`SwapContext`]). Responsive
//! layouts keep the outlet pinned and reactively restyle the chrome around
//! it — or use `idea_ui_nav::AppShell`, which packages the pinned-sidebar ⇄
//! drawer shape with a single sidebar build.

#![deny(missing_docs)]

// Lets code inside this crate refer to itself by its external name, so the
// captured `recipes` source shows the `use swap_navigator::{…}` import line an
// app author needs verbatim (mirrors `extern crate self as stack_navigator` in
// the stack navigator and `as runtime_core` in core).
extern crate self as swap_navigator;

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature — see [`recipes`].
#[cfg(all(feature = "catalog", not(feature = "new-core")))]
pub mod recipes;

// One authored surface, two cores (idea-lite migration P6): the default
// build is the old-core implementation, byte-moved into `oldcore`; the
// `new-core` feature swaps in `newcore`, which re-expresses the SAME
// public names (`SwapNavigator`/`SwapBuilder`/`SwapHandle`/
// `SwapContext`/`MountPolicy`/`Screen`…) over the vocabulary's
// `swap_navigator()` builder + `SwapNav` world context — so a consuming
// app compiles the same source against either core. Mutually exclusive
// (same names), mirroring the build-graph-wide macro-lowering switch.
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
