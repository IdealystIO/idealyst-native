//! `idea-ui-nav` — themed navigation chrome for the idealyst navigators.
//!
//! Under the outlet model, a navigator owns only its single outlet
//! (`{nav.outlet}`); everything wrapping it is ordinary author layout. This
//! crate is that layout, themed and ready-made: an [`AppShell`] (responsive
//! pinned-sidebar ⇄ drawer), a [`TabBar`], a [`Drawer`], and a
//! [`StackHeader`], wired to the
//! [`SwapContext`](runtime_core::primitives::navigator::SwapContext) /
//! `StackContext` the `.layout(|nav| …)` closure hands you.
//!
//! ```ignore
//! use swap_navigator::{SwapNavigator, SwapBuilder};
//! use idea_ui_nav::{TabBar, TabItem};
//!
//! SwapNavigator::new(&HOME)
//!     .screen(HOME, |_| Screen::new(/* … */))
//!     .screen(SETTINGS, |_| Screen::new(/* … */))
//!     .layout(|nav| ui! {
//!         view {
//!             { nav.outlet }
//!             TabBar(
//!                 items = vec![TabItem::new("home", "Home"), TabItem::new("settings", "Settings")],
//!                 active_route = nav.active_route,
//!                 on_select = nav.on_select,
//!             )
//!         }
//!     })
//! ```
//!
//! Built on the same theme as `idea-ui`, and piggybacks on its components
//! (the [`Tabs`](idea_ui::Tabs) strip) rather than reimplementing chrome.

#![deny(missing_docs)]

mod app_shell;
mod drawer;
mod stack_header;
mod tab_bar;

pub use app_shell::{sidebar_pinned, AppShell, AppShellProps};
pub use drawer::{Drawer, DrawerProps, DrawerSide};
pub use stack_header::{StackHeader, StackHeaderProps};
pub use tab_bar::{TabBar, TabBarProps, TabItem};

/// The active-tab indicator style, re-exported from `idea-ui` so callers wire
/// a [`TabBar`] without a second `use`.
pub use idea_ui::TabIndicator;
