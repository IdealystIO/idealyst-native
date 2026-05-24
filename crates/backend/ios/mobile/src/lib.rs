//! iOS backend: builds UIKit views via objc2.
//!
//! Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

#[cfg(target_os = "ios")]
mod imp;

#[cfg(not(target_os = "ios"))]
mod stub;

#[cfg(target_os = "ios")]
pub use imp::{install_global_self, set_animated_color, set_animated_f32, IosBackend, IosNode};

#[cfg(all(target_os = "ios", feature = "async-driver"))]
pub use backend_ios_core::render_loop::install_render_loop;

/// Install the iOS scheduler (NSTimer-backed). Must be called once
/// before `runtime_core::render(...)` so timer-driven features
/// (long-press recognizer, presence animations, anything calling
/// `after_ms` / `schedule_microtask`) delay correctly instead of
/// firing synchronously.
#[cfg(target_os = "ios")]
pub use backend_ios_core::scheduler::install_scheduler;

#[cfg(not(target_os = "ios"))]
pub use stub::IosBackend;

/// Non-iOS no-op so cross-compile of host code still type-checks.
#[cfg(all(not(target_os = "ios"), feature = "async-driver"))]
pub fn install_render_loop() {}

/// Non-iOS no-op so cross-compile of host code still type-checks.
#[cfg(not(target_os = "ios"))]
pub fn install_scheduler() {}

// Optional runtime-server-client entry point. Exposes `ios_main` /
// `ios_teardown` C symbols the Swift host calls to run the iOS app
// as a thin client of an runtime-server dev-server. Only compiled when
// `--features runtime-server` is set — the native-rendering build path
// pays zero binary cost.
#[cfg(all(target_os = "ios", feature = "runtime-server"))]
mod runtime_server;

// Re-export the C entry points at Rust-path level so the consuming
// staticlib crate can write a linker-anchor that references them.
// Without an anchor, Rust DCEs the symbols from the final .a even
// though they're `#[no_mangle] pub extern "C"` — staticlib output
// only retains symbols that are reachable from the crate's own
// items, and the consumer crate is otherwise empty.
#[cfg(all(target_os = "ios", feature = "runtime-server"))]
pub use runtime_server::{ios_main, ios_teardown};

/// No-op stub for `install_global_self` on non-iOS hosts so the
/// host-platform cross-compile of consumer code still type-checks.
#[cfg(not(target_os = "ios"))]
pub fn install_global_self(_weak: std::rc::Weak<std::cell::RefCell<IosBackend>>) {}

/// Non-iOS no-op stub for the animation property helper. The
/// matching `IosNode` is exposed only on iOS, so consumer code that
/// reaches this path is necessarily host-target only.
#[cfg(not(target_os = "ios"))]
pub fn set_animated_f32<T>(_node: &T, _prop: runtime_core::animation::AnimProp, _value: f32) {}

#[cfg(not(target_os = "ios"))]
pub fn set_animated_color<T>(
    _node: &T,
    _prop: runtime_core::animation::AnimProp,
    _value: [f32; 4],
) {
}
