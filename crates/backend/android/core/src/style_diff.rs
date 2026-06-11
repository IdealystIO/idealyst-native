//! The unstyled-`text()` color decision for the Android backend.
//!
//! This decision (and its regression test) now lives once in
//! `runtime_core::text_defaults` — it must be byte-identical across every
//! native backend (CLAUDE.md §7), so a single framework-level definition
//! replaces what used to be a copy-pasted, hand-synced duplicate of the
//! iOS backend. Re-exported here under the historical names so
//! `backend-android-mobile`'s JNI style path is unchanged.

pub use runtime_core::text_defaults::{
    effective_text_color, THEME_TEXT_COLOR_FALLBACK, THEME_TEXT_COLOR_TOKEN,
};
