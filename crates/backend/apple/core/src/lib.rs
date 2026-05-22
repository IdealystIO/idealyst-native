//! Cross-Apple substrate (iOS + tvOS + macOS).
//!
//! Holds the pieces that don't care about the UI toolkit:
//!
//! - [`log`] — NSLog shim. Pure Foundation.
//! - [`scheduler`] — NSTimer + main-DispatchQueue scheduler. No
//!   UIKit or AppKit dependency.
//! - [`font`] — CoreText/CoreGraphics font registration + face
//!   matching. Returns PostScript names; UIFont/NSFont construction
//!   stays in the leaf crates.
//! - [`color`] — `framework_core::Color` → `(CGFloat, CGFloat,
//!   CGFloat, CGFloat)` parsing wrapper. UIColor/NSColor adapters
//!   stay in the leaf crates.
//!
//! Modules are gated on `cfg(any(target_os = "ios", target_os =
//! "tvos", target_os = "macos"))`; on the host target the crate
//! compiles as an empty rlib so workspace-wide `cargo check` keeps
//! working.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod color;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod font;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod log;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod scheduler;

// Convenience re-export — `apple_log!` macro callers don't need to
// path through the module. Mirrors the prior `backend_ios_core::ios_log`
// shape so the iOS-core re-export stays a one-line `pub use`.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub use log::apple_log;
