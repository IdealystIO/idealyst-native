//! Non-Android stub. The actual Android backend lives under
//! [`crate::imp`] behind `#[cfg(target_os = "android")]`. This stub
//! exists so the workspace compiles on host platforms (Linux, macOS)
//! without an NDK toolchain. Runtime v2 has no `Backend` mega-trait to
//! stub: the capability impls (`crate::newcore`) are `target_os`-gated
//! with the real backend, so off-target builds only need the type to
//! exist.

pub struct AndroidBackend;

impl AndroidBackend {
    pub fn new(_context: (), _root: ()) -> Self {
        AndroidBackend
    }
}
