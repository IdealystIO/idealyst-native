//! Border-routing decision for the iOS backend.
//!
//! The decision itself lives in [`backend_apple_core::border`] so the
//! iOS and macOS backends share one source of truth and converge byte for
//! byte (Rule #7). This module re-exports it as `crate::border::uniform_border`.

pub use backend_apple_core::border::uniform_border;
