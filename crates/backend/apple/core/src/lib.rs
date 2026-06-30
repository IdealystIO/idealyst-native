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
//! - [`color`] — `runtime_core::Color` → `(CGFloat, CGFloat,
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

/// Debug-only frame-pacing trace for diagnosing animation stutter. iOS/tvOS
/// use `CADisplayLink.displayLinkWithTarget:selector:` (UIKit); macOS uses
/// `NSScreen.displayLinkWithTarget:selector:` (AppKit, macOS 14+). Both give a
/// main-thread, common-mode vsync clock for measuring scroll-tracking stalls.
/// Self-installs from `install_scheduler`; compiled out of release builds.
#[cfg(all(any(target_os = "ios", target_os = "tvos", target_os = "macos"), debug_assertions))]
pub mod perf_trace;

/// Cooperative main-thread async executor — drives `spawn_async` futures
/// on the main run loop instead of `runtime-core`'s blocking `pollster`
/// fallback, so long-running futures (SSE / WebSocket `recv` loops) don't
/// freeze the UI. Installed by [`scheduler::install_scheduler`]. Gated on
/// `async-driver` since it needs `runtime_core::driver`.
#[cfg(all(
    any(target_os = "ios", target_os = "tvos", target_os = "macos"),
    feature = "async-driver"
))]
pub mod async_executor;

/// SVG path parser, gated on Apple targets only so the host-build
/// path stays empty. Pure-Rust logic — no platform dependencies —
/// but kept inside the cfg to match the rest of the crate's
/// posture (cross-host workspace builds shouldn't link any
/// platform-specific code from here).
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod icon_path;

// Convenience re-export — `apple_log!` macro callers don't need to
// path through the module. Mirrors the prior `backend_ios_core::ios_log`
// shape so the iOS-core re-export stays a one-line `pub use`.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub use log::apple_log;

/// Pure style decisions for native editable text controls (UITextField /
/// UITextView, NSTextField / NSTextView). NOT OS-gated — it's `runtime_core`
/// only, so it builds AND unit-tests on the host while iOS + macOS share one
/// source of truth for "what background/color does an editable control get".
pub mod text_control_style;

/// The uniform-vs-per-side border routing decision shared by the iOS and
/// macOS backends. NOT OS-gated — pure `runtime_core` logic, host-testable,
/// so both backends collapse the four CSS sides identically (Rule #7).
pub mod border;
