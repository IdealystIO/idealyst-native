//! Lucide icon pack for the idealyst-native framework.
//!
//! Icons are generated at build time from SVGs in the `assets/`
//! directory. Each icon is a `pub const IconData` — only icons your
//! application imports end up in the final binary (LTO drops the rest).
//!
//! # Usage
//!
//! ```ignore
//! use icons_lucide::{SEARCH, MENU, X};
//! use runtime_core::icon;
//!
//! icon(SEARCH)
//! icon(MENU).color(|| theme.primary())
//! ```
//!
//! # Adding icons
//!
//! Drop any Lucide SVG file into `crates/icons-lucide/assets/` and
//! rebuild. The `build.rs` extracts path data and generates a
//! `SCREAMING_SNAKE_CASE` constant from the filename.

include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));
