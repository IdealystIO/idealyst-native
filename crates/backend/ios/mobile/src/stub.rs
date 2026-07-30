//! Non-iOS stub. Lets every consumer crate type-check on any host.
//!
//! The real backend lives under `cfg(target_os = "ios")` in
//! [`crate::imp`]. Runtime v2 has no `Backend` mega-trait to stub: the
//! capability impls (`crate::newcore`) are `target_os`-gated with the
//! real backend, so off-target builds only need the type to exist.

pub struct IosBackend;

impl IosBackend {
    /// Stub for the iOS-only `run_layout` so non-iOS hosts that
    /// reference it (e.g. a shared crate that calls it under cfg)
    /// link cleanly.
    pub fn run_layout(&mut self) {}
}
