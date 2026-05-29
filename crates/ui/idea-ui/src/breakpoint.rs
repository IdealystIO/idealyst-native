//! Responsive breakpoints — re-exported from `runtime-core`.
//!
//! The breakpoint primitive (the [`Breakpoint`] enum, the
//! [`Breakpoints`] threshold table, [`install_breakpoints`], and the
//! reactive [`current_breakpoint`] signal) **moved down into
//! `runtime-core`** so the style system, the `Backend` trait, and the
//! `css` crate can all reason about breakpoints — those layers can't
//! depend on `idea-ui`. See `runtime_core::breakpoint` for the full
//! documentation and the rationale.
//!
//! This module is a thin re-export kept so existing
//! `idea_ui::breakpoint::*` / `idea_ui::{Breakpoint, …}` paths keep
//! compiling unchanged.
//!
//! Most author code shouldn't read [`current_breakpoint`] directly —
//! declare `breakpoint md { … }` overlays in a `stylesheet!` and let
//! the framework realize them (CSS `@media` on web, reactive merge on
//! native). The signal is the escape hatch for imperative layout
//! switches.

pub use runtime_core::breakpoint::{
    breakpoints, current_breakpoint, install_breakpoints, Breakpoint, Breakpoints,
};
