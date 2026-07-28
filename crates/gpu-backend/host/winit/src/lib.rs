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

// idea-lite core migration (P5): `newcore::run`/`run_with` — the
// new-core windowed boot (World + Registry + realize + flush driver via
// `render_wgpu::newcore`). Off by default so the local-render build
// path is unchanged.
#[cfg(feature = "new-core")]
pub mod newcore;

pub use app::{run, run_with, RunError};

#[cfg(feature = "runtime-server")]
pub use app::run_runtime_server;

// The variant + user code consumes these via this crate so they
// don't need a direct dependency on `render-api`.
pub use render_api::DeviceProfile;
