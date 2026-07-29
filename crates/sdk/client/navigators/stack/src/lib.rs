//! Stack navigator on the **outlet model** — the push/pop sibling of
//! `swap-navigator`.
//!
//! A stack has depth: `push` mounts a screen on top of a back-stack, `pop`
//! removes the top and reveals the one below (whose scope stayed alive, so its
//! state is intact). The visible screen is the top of the stack, swapped into
//! the navigator's single outlet.
//!
//! Chrome is **author layout**: `.layout(|nav| …)` wraps `{nav.outlet}` and can
//! read `nav.active_route` / `nav.can_go_back` / `nav.depth` and call `nav.pop`
//! — e.g. an `idea_ui_nav::StackHeader`. The author derives the title from the
//! active route. This mirrors `swap-navigator`; the only difference is the
//! command vocabulary (Push/Pop/Replace/Reset vs Select) and that lower screens
//! stay mounted beneath the top.
//!
//! ```ignore
//! StackNavigator::new(&HOME)
//!     .screen(HOME, |_| Screen::new(/* … */))
//!     .screen(DETAIL, |p: DetailParams| Screen::new(/* … */))
//!     .layout(|nav| ui! {
//!         view {
//!             StackHeader(
//!                 title = rx!(title_for(nav.active_route.get())),
//!                 show_back = nav.can_go_back,
//!                 on_back = nav.pop.clone(),
//!             )
//!             { nav.outlet }
//!         }
//!     })
//!     .bind(nav);
//! ```
//!
//! # Sizing
//!
//! The navigator's root **fills its container by default** (width/height
//! 100% + `flex-grow: 1` — see `navigator_fill_rules` in `runtime-core`).
//! Override by styling the navigator element itself:
//! `StackNavigator::new(&home)…​.with_style(my_style)`.
//!
//! # Screen retention — what happens below the top
//!
//! Covered screens follow [`StackRetention`], resolved per platform by
//! default: on **web**, a push **disposes** the covered screen and pop
//! re-mounts it from its URL (browser semantics — nothing below the visible
//! page stays resident, and a cold deep link never mounts the parent it
//! synthesizes for Back until you actually pop to it); everywhere else,
//! covered screens stay alive (native-stack semantics — pop reveals them
//! with state intact). Force either with `.retention(...)`.

#![deny(missing_docs)]

// Lets code inside this crate refer to itself by its external name, so the
// captured `recipes` source shows the `use stack_navigator::{…}` import line
// an app author needs verbatim (mirrors `extern crate self as runtime_core`
// in core and `as idea_ui` in the component library).
extern crate self as stack_navigator;

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature — see [`recipes`].
#[cfg(all(feature = "catalog", not(feature = "new-core")))]
pub mod recipes;

// One authored surface, two cores (idea-lite migration P6) — see the
// same switch in `swap-navigator`: default = the old-core
// implementation, byte-moved into `oldcore`; `new-core` swaps in
// `newcore`, re-expressing the SAME public names over the vocabulary's
// `stack_navigator()` builder + `StackNav` world context. Mutually
// exclusive (same names), mirroring the macro-lowering switch.
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
