//! macOS backend: builds AppKit views via objc2.
//!
//! Real `objc2-app-kit` calls under `target_os = "macos"`; a stub on
//! other hosts so the crate type-checks during cross-compile and
//! workspace-wide `cargo check` works from any platform.
//!
//! Design notes live in `docs/macos-backend-plan.md`.

#[cfg(target_os = "macos")]
mod imp;

#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(target_os = "macos")]
pub use imp::{
    install_global_self, set_animated_color, set_animated_f32, MacosBackend, MacosNode,
};

#[cfg(not(target_os = "macos"))]
pub use stub::MacosBackend;

/// Install the macOS scheduler (NSTimer-backed). Must be called once
/// before `framework_core::render(...)` so timer-driven features
/// (presence animations, anything calling `after_ms` /
/// `schedule_microtask`) delay correctly instead of firing
/// synchronously.
///
/// On macOS this forwards to the shared
/// [`backend_apple_core::scheduler::install_scheduler`] — same code
/// the iOS backend uses.
#[cfg(target_os = "macos")]
pub use backend_apple_core::scheduler::install_scheduler;

/// Non-macOS no-op so cross-compile of host code still type-checks.
#[cfg(not(target_os = "macos"))]
pub fn install_scheduler() {}

/// No-op stub for `install_global_self` on non-macOS hosts so host-
/// platform cross-compile of consumer code still type-checks.
#[cfg(not(target_os = "macos"))]
pub fn install_global_self(_weak: std::rc::Weak<std::cell::RefCell<MacosBackend>>) {}

/// Non-macOS no-op stub for the animation property helpers. The
/// matching `MacosNode` is exposed only on macOS, so consumer code
/// that reaches these on a non-macOS host is necessarily host-only.
#[cfg(not(target_os = "macos"))]
pub fn set_animated_f32<T>(_node: &T, _prop: framework_core::animation::AnimProp, _value: f32) {}

#[cfg(not(target_os = "macos"))]
pub fn set_animated_color<T>(
    _node: &T,
    _prop: framework_core::animation::AnimProp,
    _value: [f32; 4],
) {
}
