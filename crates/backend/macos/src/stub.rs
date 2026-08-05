//! Non-macOS stub. Lets every consumer crate type-check on any host.
//!
//! The real backend lives under `cfg(target_os = "macos")` in
//! [`crate::imp`]. Runtime v2 has no `Backend` mega-trait to stub: the
//! capability impls (`crate::newcore`) are `target_os`-gated with the
//! real backend, so off-target builds only need the type to exist.

pub struct MacosBackend;
