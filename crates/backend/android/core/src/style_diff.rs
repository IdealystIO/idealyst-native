//! The unstyled-`text()` color decision for the Android backend.
//!
//! This decision (and its regression test) lives once in
//! `runtime_shared::text_defaults` — it must be byte-identical across every
//! native backend (CLAUDE.md §7). Re-exported here so
//! `backend-android-mobile`'s JNI style path can name it locally.

pub use runtime_shared::text_defaults::{
    effective_text_color, THEME_TEXT_COLOR_FALLBACK, THEME_TEXT_COLOR_TOKEN,
};
