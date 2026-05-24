//! Winit native shell for the wgpu render backend.
//!
//! Translates `winit::event::WindowEvent` → the
//! [`render_api`] event vocabulary, owns the wgpu surface
//! (built from a winit `Window`), and drives
//! `render_wgpu::Renderer` per frame.
//!
//! Variant crates (`variant-phone`, `-tablet`, `-tv`) call
//! [`run`] with a [`DeviceProfile`].

mod app;
mod gpu;
mod scheduler;

pub use app::{run, RunError};

#[cfg(feature = "runtime-server")]
pub use app::run_runtime_server;

// The variant + user code consumes these via this crate so they
// don't need a direct dependency on `render-api`.
pub use render_api::DeviceProfile;
