//! Winit native shell for the wgpu render backend.
//!
//! Translates `winit::event::WindowEvent` → the
//! [`backend_wgpu_api`] event vocabulary, owns the wgpu surface
//! (built from a winit `Window`), and drives
//! `backend_wgpu_core::Renderer` per frame.
//!
//! Variant crates (`backend-wgpu-phone`, `-tablet`, `-tv`) call
//! [`run`] with a [`DeviceProfile`].

mod app;
mod gpu;

pub use app::{run, RunError};

// The variant + user code consumes these via this crate so they
// don't need a direct dependency on `backend-wgpu-api`.
pub use backend_wgpu_api::{DeviceProfile, SimulatedPlatform};
