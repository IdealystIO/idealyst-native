//! iOS backend: builds UIKit views via objc2.
//!
//! Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

#[cfg(target_os = "ios")]
mod imp;

#[cfg(not(target_os = "ios"))]
mod stub;

#[cfg(target_os = "ios")]
pub use imp::{install_global_self, IosBackend};

#[cfg(not(target_os = "ios"))]
pub use stub::IosBackend;

// Optional AAS-client entry point. Exposes `ios_main` /
// `ios_teardown` C symbols the Swift host calls to run the iOS app
// as a thin client of an AAS dev-server. Only compiled when
// `--features aas-shell` is set — the native-rendering build path
// pays zero binary cost.
#[cfg(all(target_os = "ios", feature = "aas-shell"))]
mod aas;

// Re-export the C entry points at Rust-path level so the consuming
// staticlib crate can write a linker-anchor that references them.
// Without an anchor, Rust DCEs the symbols from the final .a even
// though they're `#[no_mangle] pub extern "C"` — staticlib output
// only retains symbols that are reachable from the crate's own
// items, and the consumer crate is otherwise empty.
#[cfg(all(target_os = "ios", feature = "aas-shell"))]
pub use aas::{ios_main, ios_teardown};

/// No-op stub for `install_global_self` on non-iOS hosts so the
/// host-platform cross-compile of consumer code still type-checks.
#[cfg(not(target_os = "ios"))]
pub fn install_global_self(_weak: std::rc::Weak<std::cell::RefCell<IosBackend>>) {}
