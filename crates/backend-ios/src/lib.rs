//! iOS backend: builds UIKit views via objc2.
//!
//! Real `objc2-ui-kit` calls under `target_os = "ios"`;
//! a stub on other hosts so the crate type-checks during cross-compile.

#[cfg(target_os = "ios")]
mod imp;

#[cfg(not(target_os = "ios"))]
mod stub;

#[cfg(target_os = "ios")]
pub use imp::IosBackend;

#[cfg(not(target_os = "ios"))]
pub use stub::IosBackend;
