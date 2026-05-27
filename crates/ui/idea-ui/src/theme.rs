//! Shim — re-exports `idea_theme::theme::*` under the `idea_ui::theme`
//! path. The actual implementation moved to the `idea-theme` crate.
//! This module exists so that internal idea-ui source (components,
//! stylesheets) — and external consumers using `idea_ui::theme::X` —
//! keep working unchanged.

pub use idea_theme::theme::*;
