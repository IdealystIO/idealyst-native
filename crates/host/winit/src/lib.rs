//! Winit native shell for the wgpu render backend.
//!
//! Translates `winit::event::WindowEvent` → the
//! [`render_api`] event vocabulary, owns the wgpu surface
//! (built from a winit `Window`), and drives
//! `render_wgpu::Renderer` per frame.
//!
//! Variant crates (`native-phone`, `-tablet`, `-tv`) call
//! [`run`] with a [`DeviceProfile`].

mod app;
mod gpu;

pub use app::{run, RunError};

// The variant + user code consumes these via this crate so they
// don't need a direct dependency on `render-api`.
pub use render_api::DeviceProfile;
