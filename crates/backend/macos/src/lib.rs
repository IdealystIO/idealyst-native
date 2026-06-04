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

// Pure passthrough hit-test geometry for the screen_recorder PrivateLayer's
// overlay window. Kept un-gated so its regression test (the recursion +
// per-level coordinate conversion that decides whether a canvas click passes
// through to the app window beneath) runs from any host; the objc traversal
// that feeds it is macos-only. Mirrors the iOS `private_layer_hittest` module.
mod private_layer_hittest;

// Pure coalescing/gating logic behind the post-mount layout-pass scheduler.
// Un-gated so its regression tests (the recording-preview-stayed-0×0 bug) run
// from any host; the libdispatch + AppKit machinery it feeds is macos-only.
mod layout_policy;

#[cfg(target_os = "macos")]
pub use imp::{
    install_global_self, private_layer_window_ids, schedule_layout_pass, set_animated_color,
    set_animated_f32, with_global_backend, MacosBackend, MacosExternalRegistrar,
    MacosNavigatorRegistrar, MacosNode,
};

#[cfg(not(target_os = "macos"))]
pub use stub::MacosBackend;

// Optional runtime-server-client entry point. Exposes `spawn_runtime_server_shell` +
// `start_main_thread_drain_timer`, modeled on `backend-ios-mobile`'s
// `aas` module. Only compiled when `--features runtime-server` is set —
// the native-rendering build path pays zero binary cost.
//
// Unlike iOS (where Swift drives the entry via `ios_main` extern "C"),
// the macOS host is pure Rust (`host-appkit`), so we expose a Rust
// function rather than a C symbol. `host-appkit`'s `run_aas` calls
// `spawn_runtime_server_shell` from inside its own `NSApplication` setup.
#[cfg(all(target_os = "macos", feature = "runtime-server"))]
pub mod runtime_server;

/// Install the macOS scheduler (NSTimer-backed). Must be called once
/// before `runtime_core::render(...)` so timer-driven features
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
pub fn set_animated_f32<T>(_node: &T, _prop: runtime_core::animation::AnimProp, _value: f32) {}

#[cfg(not(target_os = "macos"))]
pub fn set_animated_color<T>(
    _node: &T,
    _prop: runtime_core::animation::AnimProp,
    _value: [f32; 4],
) {
}
