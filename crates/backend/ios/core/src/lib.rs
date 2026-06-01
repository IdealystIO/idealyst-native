//! Shared iOS/tvOS substrate.
//!
//! Houses the bits that both `backend-ios-mobile` (touch) and
//! `backend-ios-tv` (focus engine) reuse unchanged: UIKit style
//! application, UIFont resolution, and the render loop driver.
//! Higher pieces — the `IosBackend` struct, primitive construction,
//! navigator/tab-drawer chrome — stay in the leaf crates because
//! they bake in input semantics that differ between mobile and TV.
//!
//! Cross-Apple bits (CoreText font registration, color parsing,
//! NSLog, NSTimer scheduler) live one level deeper in
//! [`backend_apple_core`]; this crate re-exports them so existing
//! `backend_ios_core::{font, scheduler, ios_log}` callers stay
//! source-compatible.
//!
//! Modules are gated on `cfg(any(target_os = "ios", target_os =
//! "tvos"))`; on the host target the crate compiles as an empty
//! rlib so workspace-wide `cargo check` keeps working.

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub mod font;

// Phase-record indirection — see `phase_record.rs`. Cross-target so
// non-iOS hosts that consume `apply_style_to_view` (the macOS bridge)
// link the same scope-guard type without needing their own copy.
pub mod phase_record;

// Border-routing decision (uniform → CALayer stroke vs. asymmetric →
// per-side bars). Cross-target so the routing logic is unit-testable
// on the host, where the UIKit-only `style` module compiles to nothing.
pub mod border;

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub mod style;

#[cfg(all(any(target_os = "ios", target_os = "tvos"), feature = "async-driver"))]
pub mod render_loop;

// The scheduler is now cross-Apple. Re-export under the same path so
// `backend_ios_core::scheduler::install_scheduler()` keeps working.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub use backend_apple_core::scheduler;

/// Platform log via NSLog. Always visible in Xcode console.
///
/// Forwards to [`backend_apple_core::log::apple_log`] — the same
/// shim now backs the macOS backend too. Kept under
/// `backend_ios_core::ios_log` for source compatibility with the
/// existing `ios_log!` macro in `backend-ios-mobile`.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
#[allow(dead_code)]
pub fn ios_log(msg: &str) {
    backend_apple_core::log::apple_log(msg)
}
