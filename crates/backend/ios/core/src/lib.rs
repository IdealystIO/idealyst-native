//! Shared iOS/tvOS substrate.
//!
//! This crate is currently a skeleton — it exists so that
//! `backend-ios-mobile` and `backend-ios-tv` have a common dependency
//! they can grow into. As code is extracted out of the mobile crate
//! (FFI plumbing, view-tree bookkeeping, Foundation/UIKit base
//! helpers, rendering) it lands here and the mobile/tv crates pick
//! it up via re-exports.
//!
//! Until the first extraction lands, intentionally empty.
