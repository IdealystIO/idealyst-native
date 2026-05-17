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

/// No-op stub for `install_global_self` on non-iOS hosts so the
/// host-platform cross-compile of consumer code still type-checks.
#[cfg(not(target_os = "ios"))]
pub fn install_global_self(_weak: std::rc::Weak<std::cell::RefCell<IosBackend>>) {}
